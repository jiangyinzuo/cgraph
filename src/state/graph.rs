//! Directed, globally deduplicated hierarchy state.
//!
//! This module deliberately contains no terminal geometry or LSP protocol
//! types. `RelationGraph` owns semantic nodes, canonical edges, branch query
//! state, anchors, and the cycle-safe projection of currently visible data.

use std::collections::{HashMap, HashSet, VecDeque};

use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use petgraph::visit::{EdgeRef, IntoEdgeReferences};

use super::{HierarchyDirection, HierarchyKind, LoadState, NodeId, SourceLocation, SymbolIdentity};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ResolvedSymbolKey {
    kind: HierarchyKind,
    location: SourceLocation,
}

impl ResolvedSymbolKey {
    fn from_identity(identity: &SymbolIdentity) -> Option<Self> {
        Some(Self {
            kind: identity.kind,
            location: identity.location.clone()?,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BranchKey {
    pub node_id: NodeId,
    pub direction: HierarchyDirection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphBranch {
    pub expanded: bool,
    pub load_state: LoadState,
    pub neighbors: Vec<NodeId>,
    pub failure: Option<String>,
    pub(crate) active_request_id: Option<u64>,
}

impl GraphBranch {
    pub fn can_toggle(&self) -> bool {
        !self.neighbors.is_empty()
    }

    pub fn toggle(&mut self) -> bool {
        if !self.can_toggle() {
            return false;
        }
        self.expanded = !self.expanded;
        true
    }

    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
}

impl Default for GraphBranch {
    fn default() -> Self {
        Self {
            expanded: false,
            load_state: LoadState::NotLoaded,
            neighbors: Vec::new(),
            failure: None,
            active_request_id: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphNode {
    pub id: NodeId,
    pub symbol: String,
    pub kind: HierarchyKind,
    pub location: Option<SourceLocation>,
    pub incoming: GraphBranch,
    pub outgoing: GraphBranch,
}

impl GraphNode {
    fn new(identity: SymbolIdentity) -> Self {
        Self {
            id: NodeId::next(),
            symbol: identity.symbol,
            kind: identity.kind,
            location: identity.location,
            incoming: GraphBranch::default(),
            outgoing: GraphBranch::default(),
        }
    }

    pub fn identity(&self) -> SymbolIdentity {
        SymbolIdentity {
            symbol: self.symbol.clone(),
            kind: self.kind,
            location: self.location.clone(),
        }
    }

    pub fn branch(&self, direction: HierarchyDirection) -> &GraphBranch {
        match direction {
            HierarchyDirection::Incoming => &self.incoming,
            HierarchyDirection::Outgoing => &self.outgoing,
        }
    }

    pub fn branch_mut(&mut self, direction: HierarchyDirection) -> &mut GraphBranch {
        match direction {
            HierarchyDirection::Incoming => &mut self.incoming,
            HierarchyDirection::Outgoing => &mut self.outgoing,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelationEdge {
    pub observed_by: Vec<BranchKey>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VisibleEdge {
    pub source: NodeId,
    pub target: NodeId,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VisibleGraph {
    pub nodes: Vec<NodeId>,
    pub edges: Vec<VisibleEdge>,
}

#[derive(Debug, Default)]
pub struct RelationGraph {
    graph: StableDiGraph<GraphNode, RelationEdge>,
    by_id: HashMap<NodeId, NodeIndex>,
    by_identity: HashMap<ResolvedSymbolKey, NodeId>,
    redirects: HashMap<NodeId, NodeId>,
    anchors: Vec<NodeId>,
}

impl RelationGraph {
    pub fn insert_symbol(&mut self, identity: SymbolIdentity) -> NodeId {
        if let Some(key) = ResolvedSymbolKey::from_identity(&identity)
            && let Some(existing) = self.by_identity.get(&key)
        {
            return *existing;
        }

        let node = GraphNode::new(identity);
        let node_id = node.id;
        let key = ResolvedSymbolKey::from_identity(&node.identity());
        let index = self.graph.add_node(node);
        self.by_id.insert(node_id, index);
        if let Some(key) = key {
            self.by_identity.insert(key, node_id);
        }
        node_id
    }

    pub fn pin_symbol(&mut self, identity: SymbolIdentity) -> NodeId {
        let node_id = self.insert_symbol(identity);
        self.pin(node_id);
        node_id
    }

    pub fn pin(&mut self, node_id: NodeId) -> bool {
        let Some(node_id) = self.resolve_id(node_id) else {
            return false;
        };
        if !self.anchors.contains(&node_id) {
            self.anchors.push(node_id);
        }
        true
    }

    pub fn unpin(&mut self, node_id: NodeId) -> bool {
        let Some(node_id) = self.resolve_id(node_id) else {
            return false;
        };
        let Some(index) = self.anchors.iter().position(|anchor| *anchor == node_id) else {
            return false;
        };
        self.anchors.remove(index);
        true
    }

    pub fn anchors(&self) -> &[NodeId] {
        &self.anchors
    }

    pub fn is_anchor(&self, node_id: NodeId) -> bool {
        self.resolve_id(node_id)
            .is_some_and(|node_id| self.anchors.contains(&node_id))
    }

    pub fn contains(&self, node_id: NodeId) -> bool {
        self.resolve_id(node_id).is_some()
    }

    pub fn node(&self, node_id: NodeId) -> Option<&GraphNode> {
        let node_id = self.resolve_id(node_id)?;
        self.by_id
            .get(&node_id)
            .and_then(|index| self.graph.node_weight(*index))
    }

    pub fn node_mut(&mut self, node_id: NodeId) -> Option<&mut GraphNode> {
        let node_id = self.resolve_id(node_id)?;
        let index = *self.by_id.get(&node_id)?;
        self.graph.node_weight_mut(index)
    }

    pub fn resolve_id(&self, mut node_id: NodeId) -> Option<NodeId> {
        let mut visited = HashSet::new();
        while let Some(next) = self.redirects.get(&node_id) {
            if !visited.insert(node_id) {
                return None;
            }
            node_id = *next;
        }
        self.by_id.contains_key(&node_id).then_some(node_id)
    }

    pub fn resolve_symbol(&mut self, node_id: NodeId, identity: SymbolIdentity) -> Option<NodeId> {
        let node_id = self.resolve_id(node_id)?;
        let new_key = ResolvedSymbolKey::from_identity(&identity);
        if let Some(existing) = new_key
            .as_ref()
            .and_then(|key| self.by_identity.get(key))
            .copied()
            && existing != node_id
        {
            return Some(self.merge_nodes(node_id, existing, identity));
        }

        if let Some(old_key) = self
            .node(node_id)
            .and_then(|node| ResolvedSymbolKey::from_identity(&node.identity()))
        {
            self.by_identity.remove(&old_key);
        }
        let node = self.node_mut(node_id)?;
        node.symbol = identity.symbol;
        node.kind = identity.kind;
        node.location = identity.location;
        if let Some(key) = new_key {
            self.by_identity.insert(key, node_id);
        }
        Some(node_id)
    }

    pub fn replace_branch_neighbors(
        &mut self,
        node_id: NodeId,
        direction: HierarchyDirection,
        children: Vec<SymbolIdentity>,
    ) -> Option<Vec<NodeId>> {
        let node_id = self.resolve_id(node_id)?;
        let owner = BranchKey { node_id, direction };
        let old_neighbors = self.node(node_id)?.branch(direction).neighbors.clone();
        for neighbor in old_neighbors {
            self.remove_observation(node_id, neighbor, owner);
        }

        let mut seen = HashSet::new();
        let mut neighbors = Vec::new();
        for child in children {
            let child_id = self.insert_symbol(child);
            if !seen.insert(child_id) {
                continue;
            }
            self.observe_relation(node_id, child_id, owner);
            neighbors.push(child_id);
        }
        self.node_mut(node_id)?.branch_mut(direction).neighbors = neighbors.clone();
        Some(neighbors)
    }

    pub fn clear_branch(&mut self, node_id: NodeId, direction: HierarchyDirection) -> bool {
        let Some(node_id) = self.resolve_id(node_id) else {
            return false;
        };
        let owner = BranchKey { node_id, direction };
        let neighbors = self
            .node(node_id)
            .map(|node| node.branch(direction).neighbors.clone())
            .unwrap_or_default();
        for neighbor in neighbors {
            self.remove_observation(node_id, neighbor, owner);
        }
        let Some(node) = self.node_mut(node_id) else {
            return false;
        };
        *node.branch_mut(direction) = GraphBranch::default();
        true
    }

    pub fn visible_graph(&self) -> VisibleGraph {
        let mut visible = VisibleGraph::default();
        let mut seen_nodes = HashSet::new();
        let mut seen_edges = HashSet::new();
        let mut queue = VecDeque::new();

        for anchor in &self.anchors {
            if self.contains(*anchor) && seen_nodes.insert(*anchor) {
                visible.nodes.push(*anchor);
                queue.push_back(*anchor);
            }
        }

        while let Some(node_id) = queue.pop_front() {
            let Some(node) = self.node(node_id) else {
                continue;
            };
            for direction in [HierarchyDirection::Incoming, HierarchyDirection::Outgoing] {
                let branch = node.branch(direction);
                if !branch.expanded {
                    continue;
                }
                for neighbor in &branch.neighbors {
                    let Some(neighbor) = self.resolve_id(*neighbor) else {
                        continue;
                    };
                    let edge = canonical_edge(node_id, neighbor, direction);
                    if self.has_edge(edge.source, edge.target) && seen_edges.insert(edge) {
                        visible.edges.push(edge);
                    }
                    if seen_nodes.insert(neighbor) {
                        visible.nodes.push(neighbor);
                        queue.push_back(neighbor);
                    }
                }
            }
        }
        visible
    }

    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    fn observe_relation(&mut self, node_id: NodeId, neighbor: NodeId, owner: BranchKey) {
        let edge = canonical_edge(node_id, neighbor, owner.direction);
        let Some(source) = self.resolve_id(edge.source) else {
            return;
        };
        let Some(target) = self.resolve_id(edge.target) else {
            return;
        };
        let source_index = self.by_id[&source];
        let target_index = self.by_id[&target];
        if let Some(edge_index) = self.graph.find_edge(source_index, target_index) {
            let relation = self
                .graph
                .edge_weight_mut(edge_index)
                .expect("found edges have weights");
            if !relation.observed_by.contains(&owner) {
                relation.observed_by.push(owner);
            }
        } else {
            self.graph.add_edge(
                source_index,
                target_index,
                RelationEdge {
                    observed_by: vec![owner],
                },
            );
        }
    }

    fn remove_observation(&mut self, node_id: NodeId, neighbor: NodeId, owner: BranchKey) {
        let edge = canonical_edge(node_id, neighbor, owner.direction);
        let Some(source) = self.resolve_id(edge.source) else {
            return;
        };
        let Some(target) = self.resolve_id(edge.target) else {
            return;
        };
        let Some(edge_index) = self
            .graph
            .find_edge(self.by_id[&source], self.by_id[&target])
        else {
            return;
        };
        let relation = self
            .graph
            .edge_weight_mut(edge_index)
            .expect("found edges have weights");
        relation.observed_by.retain(|candidate| *candidate != owner);
        if relation.observed_by.is_empty() {
            self.graph.remove_edge(edge_index);
        }
    }

    fn has_edge(&self, source: NodeId, target: NodeId) -> bool {
        let (Some(source), Some(target)) = (self.resolve_id(source), self.resolve_id(target))
        else {
            return false;
        };
        self.graph
            .find_edge(self.by_id[&source], self.by_id[&target])
            .is_some()
    }

    fn merge_nodes(
        &mut self,
        source_id: NodeId,
        target_id: NodeId,
        identity: SymbolIdentity,
    ) -> NodeId {
        let identity_key = ResolvedSymbolKey::from_identity(&identity);
        let source_index = self.by_id[&source_id];
        let target_index = self.by_id[&target_id];
        let incident = self
            .graph
            .edge_references()
            .filter(|edge| edge.source() == source_index || edge.target() == source_index)
            .map(|edge| {
                let source = self.graph[edge.source()].id;
                let target = self.graph[edge.target()].id;
                (source, target, edge.weight().clone())
            })
            .collect::<Vec<_>>();
        let source_node = self
            .graph
            .remove_node(source_index)
            .expect("source node exists while merging");
        self.by_id.remove(&source_id);
        self.redirects.insert(source_id, target_id);
        self.by_identity.retain(|_, value| *value != source_id);

        for node_index in self.graph.node_indices().collect::<Vec<_>>() {
            let node = &mut self.graph[node_index];
            replace_neighbor(&mut node.incoming.neighbors, source_id, target_id);
            replace_neighbor(&mut node.outgoing.neighbors, source_id, target_id);
        }
        for relation in self.graph.edge_weights_mut() {
            for owner in &mut relation.observed_by {
                if owner.node_id == source_id {
                    owner.node_id = target_id;
                }
            }
            relation.observed_by.sort_by_key(|owner| {
                (
                    owner.node_id.0,
                    match owner.direction {
                        HierarchyDirection::Incoming => 0,
                        HierarchyDirection::Outgoing => 1,
                    },
                )
            });
            relation.observed_by.dedup();
        }

        {
            let target = self
                .graph
                .node_weight_mut(target_index)
                .expect("target node remains while merging");
            merge_branch(&mut target.incoming, source_node.incoming);
            merge_branch(&mut target.outgoing, source_node.outgoing);
            target.symbol = identity.symbol;
            target.kind = identity.kind;
            target.location = identity.location.clone();
        }

        for (source, target, mut relation) in incident {
            let source = if source == source_id {
                target_id
            } else {
                source
            };
            let target = if target == source_id {
                target_id
            } else {
                target
            };
            for owner in &mut relation.observed_by {
                if owner.node_id == source_id {
                    owner.node_id = target_id;
                }
            }
            let source_index = self.by_id[&source];
            let target_index = self.by_id[&target];
            if let Some(edge_index) = self.graph.find_edge(source_index, target_index) {
                let existing = self
                    .graph
                    .edge_weight_mut(edge_index)
                    .expect("found edges have weights");
                for owner in relation.observed_by {
                    if !existing.observed_by.contains(&owner) {
                        existing.observed_by.push(owner);
                    }
                }
            } else {
                self.graph.add_edge(source_index, target_index, relation);
            }
        }

        for anchor in &mut self.anchors {
            if *anchor == source_id {
                *anchor = target_id;
            }
        }
        let mut seen = HashSet::new();
        self.anchors.retain(|anchor| seen.insert(*anchor));
        if let Some(key) = identity_key {
            self.by_identity.insert(key, target_id);
        }
        target_id
    }
}

fn canonical_edge(node_id: NodeId, neighbor: NodeId, direction: HierarchyDirection) -> VisibleEdge {
    match direction {
        HierarchyDirection::Incoming => VisibleEdge {
            source: neighbor,
            target: node_id,
        },
        HierarchyDirection::Outgoing => VisibleEdge {
            source: node_id,
            target: neighbor,
        },
    }
}

fn replace_neighbor(neighbors: &mut Vec<NodeId>, source: NodeId, target: NodeId) {
    for neighbor in neighbors.iter_mut() {
        if *neighbor == source {
            *neighbor = target;
        }
    }
    let mut seen = HashSet::new();
    neighbors.retain(|neighbor| seen.insert(*neighbor));
}

fn merge_branch(target: &mut GraphBranch, source: GraphBranch) {
    target.expanded |= source.expanded;
    for neighbor in source.neighbors {
        if !target.neighbors.contains(&neighbor) {
            target.neighbors.push(neighbor);
        }
    }
    if load_state_priority(source.load_state) > load_state_priority(target.load_state) {
        target.load_state = source.load_state;
        target.failure = source.failure;
    } else if target.failure.is_none() {
        target.failure = source.failure;
    }
    if target.active_request_id.is_none() {
        target.active_request_id = source.active_request_id;
    }
}

fn load_state_priority(state: LoadState) -> u8 {
    match state {
        LoadState::NotLoaded => 0,
        LoadState::Failed => 1,
        LoadState::Loading => 2,
        LoadState::Loaded => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::{RelationGraph, VisibleEdge};
    use crate::state::{
        HierarchyDirection, HierarchyKind, LoadState, SourceLocation, SymbolIdentity,
    };

    fn symbol(name: &str, line: u32) -> SymbolIdentity {
        SymbolIdentity {
            symbol: name.to_owned(),
            kind: HierarchyKind::Call,
            location: Some(SourceLocation {
                uri: "file:///workspace/src/lib.rs".to_owned(),
                line: Some(line),
                character: Some(0),
            }),
        }
    }

    #[test]
    fn globally_deduplicates_diamond_nodes_without_losing_edges() {
        let mut graph = RelationGraph::default();
        let root = graph.pin_symbol(symbol("root", 0));
        graph
            .replace_branch_neighbors(
                root,
                HierarchyDirection::Outgoing,
                vec![symbol("left", 1), symbol("right", 2)],
            )
            .unwrap();
        graph.node_mut(root).unwrap().outgoing.expanded = true;
        let left = graph.node(root).unwrap().outgoing.neighbors[0];
        let right = graph.node(root).unwrap().outgoing.neighbors[1];
        let shared_from_left = graph
            .replace_branch_neighbors(
                left,
                HierarchyDirection::Outgoing,
                vec![symbol("shared", 3)],
            )
            .unwrap()[0];
        let shared_from_right = graph
            .replace_branch_neighbors(
                right,
                HierarchyDirection::Outgoing,
                vec![symbol("shared", 3)],
            )
            .unwrap()[0];
        graph.node_mut(left).unwrap().outgoing.expanded = true;
        graph.node_mut(right).unwrap().outgoing.expanded = true;

        assert_eq!(shared_from_left, shared_from_right);
        assert_eq!(graph.node_count(), 4);
        assert_eq!(graph.edge_count(), 4);
        let visible = graph.visible_graph();
        assert_eq!(visible.nodes.len(), 4);
        assert_eq!(visible.edges.len(), 4);
    }

    #[test]
    fn merges_the_same_edge_observed_from_both_directions() {
        let mut graph = RelationGraph::default();
        let caller = graph.pin_symbol(symbol("caller", 0));
        let callee = graph.insert_symbol(symbol("callee", 1));
        graph
            .replace_branch_neighbors(
                caller,
                HierarchyDirection::Outgoing,
                vec![symbol("callee", 1)],
            )
            .unwrap();
        graph
            .replace_branch_neighbors(
                callee,
                HierarchyDirection::Incoming,
                vec![symbol("caller", 0)],
            )
            .unwrap();

        assert_eq!(graph.edge_count(), 1);
    }

    #[test]
    fn traverses_cycles_once_and_preserves_the_closing_edge() {
        let mut graph = RelationGraph::default();
        let first = graph.pin_symbol(symbol("first", 0));
        let second = graph
            .replace_branch_neighbors(
                first,
                HierarchyDirection::Outgoing,
                vec![symbol("second", 1)],
            )
            .unwrap()[0];
        let third = graph
            .replace_branch_neighbors(
                second,
                HierarchyDirection::Outgoing,
                vec![symbol("third", 2)],
            )
            .unwrap()[0];
        graph
            .replace_branch_neighbors(
                third,
                HierarchyDirection::Outgoing,
                vec![symbol("first", 0)],
            )
            .unwrap();
        for node_id in [first, second, third] {
            graph.node_mut(node_id).unwrap().outgoing.expanded = true;
        }

        let visible = graph.visible_graph();
        assert_eq!(visible.nodes.len(), 3);
        assert_eq!(visible.edges.len(), 3);
        assert!(visible.edges.contains(&VisibleEdge {
            source: third,
            target: first,
        }));
    }

    #[test]
    fn represents_a_self_loop_without_creating_an_extra_node() {
        let mut graph = RelationGraph::default();
        let node = graph.pin_symbol(symbol("recursive", 0));
        graph
            .replace_branch_neighbors(
                node,
                HierarchyDirection::Outgoing,
                vec![symbol("recursive", 0)],
            )
            .unwrap();
        graph.node_mut(node).unwrap().outgoing.expanded = true;

        assert_eq!(graph.node_count(), 1);
        assert_eq!(graph.edge_count(), 1);
        assert_eq!(
            graph.visible_graph().edges,
            [VisibleEdge {
                source: node,
                target: node,
            }]
        );
    }

    #[test]
    fn keeps_same_named_symbols_at_different_locations_separate() {
        let mut graph = RelationGraph::default();
        let first = graph.insert_symbol(symbol("same", 1));
        let second = graph.insert_symbol(symbol("same", 2));

        assert_ne!(first, second);
        assert_eq!(graph.node_count(), 2);
    }

    #[test]
    fn resolves_a_provisional_anchor_into_an_existing_semantic_node() {
        let mut graph = RelationGraph::default();
        let provisional = graph.pin_symbol(SymbolIdentity {
            symbol: "target".to_owned(),
            kind: HierarchyKind::Call,
            location: None,
        });
        let existing = graph.insert_symbol(symbol("Module::target", 4));

        let resolved = graph
            .resolve_symbol(provisional, symbol("Module::target", 4))
            .unwrap();

        assert_eq!(resolved, existing);
        assert_eq!(graph.resolve_id(provisional), Some(existing));
        assert_eq!(graph.anchors(), [existing]);
        assert_eq!(graph.node_count(), 1);
    }

    #[test]
    fn clearing_one_branch_keeps_an_edge_observed_by_the_other_endpoint() {
        let mut graph = RelationGraph::default();
        let caller = graph.pin_symbol(symbol("caller", 0));
        let callee = graph.insert_symbol(symbol("callee", 1));
        graph
            .replace_branch_neighbors(
                caller,
                HierarchyDirection::Outgoing,
                vec![symbol("callee", 1)],
            )
            .unwrap();
        graph
            .replace_branch_neighbors(
                callee,
                HierarchyDirection::Incoming,
                vec![symbol("caller", 0)],
            )
            .unwrap();

        assert!(graph.clear_branch(caller, HierarchyDirection::Outgoing));
        assert_eq!(graph.edge_count(), 1);
        assert_eq!(
            graph.node(caller).unwrap().outgoing.load_state,
            LoadState::NotLoaded
        );
        assert!(graph.clear_branch(callee, HierarchyDirection::Incoming));
        assert_eq!(graph.edge_count(), 0);
    }
}
