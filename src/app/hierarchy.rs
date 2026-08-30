use super::{App, filtering::candidate_is_visible};
use crate::{
    fetch::{CachePolicy, FetchSource, HierarchyQuery, HierarchyResponse},
    state::{HierarchyDirection, LoadState, NodeId, SymbolIdentity},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HierarchyLoadRequest {
    pub request_id: u64,
    pub node_id: NodeId,
    pub query: HierarchyQuery,
    pub cache_policy: CachePolicy,
    previous_load_state: LoadState,
}

impl App {
    pub fn delete_selected_branch(&mut self, direction: HierarchyDirection) -> bool {
        let Some(selected) = self.selected else {
            return false;
        };
        let cleared = self.graph.clear_branch(selected, direction);
        if cleared {
            self.clear_canvas_notice();
        }
        cleared
    }

    pub fn toggle_selected_branch(
        &mut self,
        direction: HierarchyDirection,
        hierarchy_available: bool,
    ) -> Option<HierarchyLoadRequest> {
        let selected = self.selected?;
        self.toggle_node_branch(selected, direction, hierarchy_available)
    }

    pub fn toggle_node_branch(
        &mut self,
        node_id: NodeId,
        direction: HierarchyDirection,
        hierarchy_available: bool,
    ) -> Option<HierarchyLoadRequest> {
        let node_id = self.graph.resolve_id(node_id)?;
        let node = self.graph.node_mut(node_id)?;
        self.selected = Some(node_id);
        let identity = node.identity();
        let branch = node.branch_mut(direction);

        if branch.can_toggle() {
            branch.toggle();
            return None;
        }
        if branch.load_state == LoadState::Loading {
            branch.expanded = !branch.expanded;
            return None;
        }
        if branch.load_state == LoadState::Loaded {
            return None;
        }
        if !hierarchy_available {
            let message = format!(
                "Hierarchy query unavailable for {}: no analysis provider",
                identity.symbol
            );
            branch.load_state = LoadState::Failed;
            branch.expanded = false;
            branch.failure = Some("Hierarchy requires an available LSP server".to_owned());
            branch.active_request_id = None;
            self.set_canvas_error(message);
            return None;
        }

        self.begin_hierarchy_load(node_id, identity, direction, CachePolicy::UseCache, true)
    }

    pub fn refresh_selected_branches(
        &mut self,
        hierarchy_available: bool,
    ) -> Vec<HierarchyLoadRequest> {
        let Some(selected) = self
            .selected
            .and_then(|node_id| self.graph.resolve_id(node_id))
        else {
            return Vec::new();
        };
        self.selected = Some(selected);
        if !hierarchy_available {
            self.set_canvas_notice("Refresh requires an available LSP server");
            return Vec::new();
        }

        self.clear_canvas_notice();
        let identity = self
            .graph
            .node(selected)
            .expect("resolved graph nodes exist")
            .identity();
        [HierarchyDirection::Incoming, HierarchyDirection::Outgoing]
            .into_iter()
            .filter_map(|direction| {
                self.begin_hierarchy_load(
                    selected,
                    identity.clone(),
                    direction,
                    CachePolicy::Refresh,
                    false,
                )
            })
            .collect()
    }

    pub fn finish_hierarchy(
        &mut self,
        request: &HierarchyLoadRequest,
        result: Result<HierarchyResponse, String>,
    ) -> bool {
        let Some(node_id) = self.graph.resolve_id(request.node_id) else {
            return false;
        };
        let branch = self
            .graph
            .node_mut(node_id)
            .expect("resolved graph nodes exist")
            .branch_mut(request.query.direction);
        if branch.active_request_id != Some(request.request_id) {
            return false;
        }
        match result {
            Ok(response) => {
                let source = response.source;
                let was_selected = self.selected == Some(node_id);
                let Some(node_id) = self
                    .graph
                    .resolve_symbol(node_id, response.query.symbol.clone())
                else {
                    return false;
                };
                if was_selected {
                    self.selected = Some(node_id);
                }
                if self
                    .graph
                    .node(node_id)
                    .expect("resolved graph nodes exist")
                    .branch(request.query.direction)
                    .active_request_id
                    != Some(request.request_id)
                {
                    return false;
                }
                let children = response
                    .children
                    .into_iter()
                    .filter(|child| {
                        !self.symbol_filter.is_ignored(&child.symbol)
                            && candidate_is_visible(
                                &child.symbol,
                                child.location.as_ref(),
                                &self.filters,
                                &self.workspace,
                            )
                    })
                    .collect();
                self.graph
                    .replace_branch_neighbors(node_id, request.query.direction, children);
                let branch = self
                    .graph
                    .node_mut(node_id)
                    .expect("resolved graph nodes exist")
                    .branch_mut(request.query.direction);
                branch.active_request_id = None;
                branch.load_state = LoadState::Loaded;
                branch.failure = None;
                if branch.neighbors.is_empty() {
                    branch.expanded = false;
                }
                if source == FetchSource::TreeSitter {
                    self.set_canvas_notice(
                        "Tree-sitter: syntactic relations only; dynamic dispatch may be omitted"
                            .to_owned(),
                    );
                }
            }
            Err(error) => {
                let direction = match request.query.direction {
                    HierarchyDirection::Incoming => "incoming/parent",
                    HierarchyDirection::Outgoing => "outgoing/child",
                };
                self.set_canvas_error(format!(
                    "Hierarchy query failed for {} ({direction}): {error}",
                    request.query.symbol.symbol
                ));
                let branch = self
                    .graph
                    .node_mut(node_id)
                    .expect("resolved graph nodes exist")
                    .branch_mut(request.query.direction);
                branch.active_request_id = None;
                if request.cache_policy == CachePolicy::Refresh {
                    branch.load_state = request.previous_load_state;
                } else {
                    branch.load_state = LoadState::Failed;
                    branch.expanded = false;
                }
                branch.failure = Some(error);
            }
        }
        true
    }

    pub(super) fn begin_hierarchy_load(
        &mut self,
        node_id: NodeId,
        identity: SymbolIdentity,
        direction: HierarchyDirection,
        cache_policy: CachePolicy,
        expand: bool,
    ) -> Option<HierarchyLoadRequest> {
        let request_id = self.next_hierarchy_request_id;
        self.next_hierarchy_request_id = self.next_hierarchy_request_id.wrapping_add(1);
        let branch = self.graph.node_mut(node_id)?.branch_mut(direction);
        let previous_load_state = match branch.load_state {
            LoadState::Loading if branch.neighbors.is_empty() => LoadState::NotLoaded,
            LoadState::Loading => LoadState::Loaded,
            state => state,
        };
        branch.load_state = LoadState::Loading;
        if expand {
            branch.expanded = true;
        }
        branch.failure = None;
        branch.active_request_id = Some(request_id);
        Some(HierarchyLoadRequest {
            request_id,
            node_id,
            query: HierarchyQuery {
                symbol: identity,
                direction,
            },
            cache_policy,
            previous_load_state,
        })
    }
}
