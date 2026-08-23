use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use petgraph::{algo::kosaraju_scc, graph::DiGraph};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::state::{
    LoadState, NodeId, Viewport,
    graph::{GraphBranch, GraphNode, RelationGraph},
};

mod connections;

pub(super) use connections::{CanvasConnections, CanvasEdge};

const ROOT_NODE_WIDTH: u16 = 28;
const ROOT_NODE_HEIGHT: u16 = 3;
const ROOT_COLUMN_GAP: u16 = 4;
const ROOT_ROW_GAP: u16 = 1;
const NODE_CHROME_WIDTH: u16 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CanvasNodePlacement {
    pub(super) node_id: NodeId,
    pub(super) slot: ProjectedRect,
    pub(super) visible_slot: Rect,
    pub(super) area: Rect,
    pub(super) incoming_button: Rect,
    pub(super) outgoing_button: Rect,
    pub(super) local_area: Rect,
    pub(super) local_incoming_button: Rect,
    pub(super) local_outgoing_button: Rect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ProjectedRect {
    pub(super) x: i64,
    pub(super) y: i64,
    pub(super) width: u16,
    pub(super) height: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct WorldRect {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: u16,
    pub(super) height: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct WorldNodePlacement {
    pub(super) node_id: NodeId,
    pub(super) slot: WorldRect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProjectedNodePlacement {
    node_id: NodeId,
    slot: ProjectedRect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EdgeVisualKind {
    Forward,
    BackOrCycle,
    SelfLoop,
}

impl EdgeVisualKind {
    fn is_special(self) -> bool {
        self != Self::Forward
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LayoutEdge {
    pub(super) source_id: NodeId,
    pub(super) target_id: NodeId,
    pub(super) visual_kind: EdgeVisualKind,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct WorldLayoutSnapshot {
    pub(super) nodes: Vec<WorldNodePlacement>,
    pub(super) edges: Vec<LayoutEdge>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct CanvasLayoutSnapshot {
    pub(super) nodes: Vec<CanvasNodePlacement>,
    pub(super) edges: Vec<CanvasEdge>,
}

pub(super) struct CanvasNodeWidget<'a> {
    pub(super) node: &'a GraphNode,
    pub(super) placement: CanvasNodePlacement,
    pub(super) selected: bool,
}

impl Widget for CanvasNodeWidget<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let placement = self.placement;
        let mut local = Buffer::empty(Rect::new(0, 0, placement.slot.width, placement.slot.height));
        let border_style = if self.selected {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan)
        };
        let content_style = if self.selected {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Paragraph::new(self.node.symbol.as_str())
            .alignment(Alignment::Center)
            .style(content_style)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style),
            )
            .render(placement.local_area, &mut local);
        branch_button(&self.node.incoming, Color::Blue)
            .render(placement.local_incoming_button, &mut local);
        branch_button(&self.node.outgoing, Color::Blue)
            .render(placement.local_outgoing_button, &mut local);

        for y in placement.visible_slot.y..placement.visible_slot.bottom() {
            for x in placement.visible_slot.x..placement.visible_slot.right() {
                if !area.contains((x, y).into()) {
                    continue;
                }
                let local_x = i64::from(x).saturating_sub(placement.slot.x);
                let local_y = i64::from(y).saturating_sub(placement.slot.y);
                let Ok(local_x) = u16::try_from(local_x) else {
                    continue;
                };
                let Ok(local_y) = u16::try_from(local_y) else {
                    continue;
                };
                let local_position = (local_x, local_y).into();
                if !placement.local_area.contains(local_position)
                    && !placement.local_incoming_button.contains(local_position)
                    && !placement.local_outgoing_button.contains(local_position)
                {
                    continue;
                }
                let Some(source) = local.cell((local_x, local_y)) else {
                    continue;
                };
                let Some(target) = buffer.cell_mut((x, y)) else {
                    continue;
                };
                *target = source.clone();
            }
        }
    }
}

fn branch_button(branch: &GraphBranch, color: Color) -> Paragraph<'static> {
    let (label, style) = match branch.load_state {
        LoadState::NotLoaded if branch.neighbors.is_empty() => {
            ("[+]", Style::default().fg(Color::Yellow))
        }
        LoadState::Loading => ("[~]", Style::default().fg(Color::Yellow)),
        LoadState::Failed => ("[!]", Style::default().fg(Color::Red)),
        LoadState::Loaded if branch.neighbors.is_empty() => {
            (" · ", Style::default().fg(Color::DarkGray))
        }
        _ if branch.expanded => ("[-]", Style::default().fg(color)),
        _ => ("[+]", Style::default().fg(color)),
    };
    Paragraph::new(label).style(style)
}

pub(super) fn canvas_layout(
    area: Rect,
    graph: &RelationGraph,
    selected: Option<NodeId>,
    viewport: Viewport,
) -> CanvasLayoutSnapshot {
    let world = world_canvas_layout(graph, selected);
    let projected = world
        .nodes
        .iter()
        .map(|placement| ProjectedNodePlacement {
            node_id: placement.node_id,
            slot: project_world_slot(area, placement.slot, viewport),
        })
        .collect::<Vec<_>>();
    let nodes = projected
        .iter()
        .filter_map(|placement| {
            let visible_slot = intersect_projected_rect(area, placement.slot)?;
            Some(node_placement(
                placement.node_id,
                placement.slot,
                visible_slot,
                area,
            ))
        })
        .collect::<Vec<_>>();
    let mut edges = world
        .edges
        .iter()
        .filter_map(|edge| connections::route_edge(*edge, &projected, area))
        .collect::<Vec<_>>();
    edges.sort_by_key(|edge| edge.visual_kind.is_special());
    CanvasLayoutSnapshot { nodes, edges }
}

pub(super) fn world_canvas_layout(
    graph: &RelationGraph,
    selected: Option<NodeId>,
) -> WorldLayoutSnapshot {
    let visible = graph.visible_graph();
    if visible.nodes.is_empty() {
        return WorldLayoutSnapshot::default();
    }

    let mut topology = DiGraph::<NodeId, ()>::new();
    let mut topology_index = HashMap::new();
    for node_id in &visible.nodes {
        topology_index.insert(*node_id, topology.add_node(*node_id));
    }
    for edge in &visible.edges {
        topology.add_edge(
            topology_index[&edge.source],
            topology_index[&edge.target],
            (),
        );
    }

    let components = kosaraju_scc(&topology);
    let mut component_by_node = HashMap::new();
    for (component, members) in components.iter().enumerate() {
        for member in members {
            component_by_node.insert(topology[*member], component);
        }
    }
    let component_ranks = component_ranks(&visible.nodes, &visible.edges, &component_by_node);
    let selected = selected
        .and_then(|node_id| graph.resolve_id(node_id))
        .filter(|node_id| visible.nodes.contains(node_id))
        .unwrap_or(visible.nodes[0]);
    let mut nodes_by_rank = BTreeMap::<i32, Vec<NodeId>>::new();
    for node_id in &visible.nodes {
        let rank = component_ranks[&component_by_node[node_id]];
        nodes_by_rank.entry(rank).or_default().push(*node_id);
    }

    let column_widths = nodes_by_rank
        .iter()
        .map(|(rank, nodes)| {
            let width = nodes
                .iter()
                .map(|node_id| node_slot_width(graph, *node_id))
                .max()
                .unwrap_or(ROOT_NODE_WIDTH);
            (*rank, width)
        })
        .collect::<BTreeMap<_, _>>();
    let column_x = column_positions(&column_widths);
    let mut placements = Vec::with_capacity(visible.nodes.len());
    for (rank, nodes) in &nodes_by_rank {
        for (index, node_id) in nodes.iter().enumerate() {
            let width = node_slot_width(graph, *node_id);
            let row = if *rank == 0 {
                alternating_row(index)
            } else {
                index as i32 - nodes.len().saturating_sub(1) as i32 / 2
            };
            placements.push(WorldNodePlacement {
                node_id: *node_id,
                slot: WorldRect {
                    x: column_x[rank],
                    y: row
                        .saturating_mul(i32::from(ROOT_NODE_HEIGHT + ROOT_ROW_GAP))
                        .saturating_sub(i32::from(ROOT_NODE_HEIGHT / 2)),
                    width,
                    height: ROOT_NODE_HEIGHT,
                },
            });
        }
    }
    let selected_slot = placements
        .iter()
        .find(|placement| placement.node_id == selected)
        .expect("the selected visible node has a placement")
        .slot;
    let selected_center_x = selected_slot
        .x
        .saturating_add(i32::from(selected_slot.width) / 2);
    let selected_center_y = selected_slot
        .y
        .saturating_add(i32::from(selected_slot.height) / 2);
    for placement in &mut placements {
        placement.slot.x = placement.slot.x.saturating_sub(selected_center_x);
        placement.slot.y = placement.slot.y.saturating_sub(selected_center_y);
    }

    let edges = visible
        .edges
        .iter()
        .map(|edge| {
            let source_component = component_by_node[&edge.source];
            let target_component = component_by_node[&edge.target];
            let visual_kind = if edge.source == edge.target {
                EdgeVisualKind::SelfLoop
            } else if source_component == target_component
                || component_ranks[&target_component] <= component_ranks[&source_component]
            {
                EdgeVisualKind::BackOrCycle
            } else {
                EdgeVisualKind::Forward
            };
            LayoutEdge {
                source_id: edge.source,
                target_id: edge.target,
                visual_kind,
            }
        })
        .collect();

    WorldLayoutSnapshot {
        nodes: placements,
        edges,
    }
}

fn component_ranks(
    nodes: &[NodeId],
    edges: &[crate::state::graph::VisibleEdge],
    component_by_node: &HashMap<NodeId, usize>,
) -> HashMap<usize, i32> {
    let component_count = component_by_node
        .values()
        .copied()
        .max()
        .map_or(0, |max| max + 1);
    let mut outgoing = vec![HashSet::new(); component_count];
    let mut indegree = vec![0_usize; component_count];
    for edge in edges {
        let source = component_by_node[&edge.source];
        let target = component_by_node[&edge.target];
        if source != target && outgoing[source].insert(target) {
            indegree[target] += 1;
        }
    }
    let component_order = nodes.iter().enumerate().fold(
        HashMap::<usize, usize>::new(),
        |mut order, (index, node_id)| {
            order.entry(component_by_node[node_id]).or_insert(index);
            order
        },
    );
    let mut ready = indegree
        .iter()
        .enumerate()
        .filter_map(|(component, indegree)| (*indegree == 0).then_some(component))
        .collect::<Vec<_>>();
    ready.sort_by_key(|component| component_order[component]);
    let mut ready = VecDeque::from(ready);
    let mut ranks = (0..component_count)
        .map(|component| (component, 0_i32))
        .collect::<HashMap<_, _>>();

    while let Some(component) = ready.pop_front() {
        let next_rank = ranks[&component].saturating_add(1);
        let mut targets = outgoing[component].iter().copied().collect::<Vec<_>>();
        targets.sort_by_key(|target| component_order[target]);
        for target in targets {
            ranks
                .entry(target)
                .and_modify(|rank| *rank = (*rank).max(next_rank));
            indegree[target] -= 1;
            if indegree[target] == 0 {
                ready.push_back(target);
            }
        }
    }
    ranks
}

fn column_positions(widths: &BTreeMap<i32, u16>) -> BTreeMap<i32, i32> {
    let mut positions = BTreeMap::new();
    let zero_width = i32::from(*widths.get(&0).unwrap_or(&ROOT_NODE_WIDTH));
    positions.insert(0, -(zero_width / 2));

    let mut previous_right = zero_width - zero_width / 2;
    for (rank, width) in widths.range(1..) {
        let x = previous_right.saturating_add(i32::from(ROOT_COLUMN_GAP));
        positions.insert(*rank, x);
        previous_right = x.saturating_add(i32::from(*width));
    }

    let mut next_left = -(zero_width / 2);
    for (rank, width) in widths.range(..0).rev() {
        let x = next_left
            .saturating_sub(i32::from(ROOT_COLUMN_GAP))
            .saturating_sub(i32::from(*width));
        positions.insert(*rank, x);
        next_left = x;
    }
    positions
}

fn alternating_row(index: usize) -> i32 {
    if index == 0 {
        0
    } else if index % 2 == 1 {
        index.div_ceil(2) as i32
    } else {
        -(index as i32 / 2)
    }
}

fn project_world_slot(area: Rect, slot: WorldRect, viewport: Viewport) -> ProjectedRect {
    let anchor_x = area.x + area.width / 2;
    let anchor_y = area.y + area.height / 2;
    let screen_x = i64::from(anchor_x) + i64::from(slot.x) + i64::from(viewport.offset_x);
    let screen_y = i64::from(anchor_y) + i64::from(slot.y) + i64::from(viewport.offset_y);
    ProjectedRect {
        x: screen_x,
        y: screen_y,
        width: slot.width,
        height: slot.height,
    }
}

fn intersect_projected_rect(area: Rect, projected: ProjectedRect) -> Option<Rect> {
    let left = projected.x.max(i64::from(area.x));
    let top = projected.y.max(i64::from(area.y));
    let right = projected
        .x
        .saturating_add(i64::from(projected.width))
        .min(i64::from(area.right()));
    let bottom = projected
        .y
        .saturating_add(i64::from(projected.height))
        .min(i64::from(area.bottom()));
    if left >= right || top >= bottom {
        return None;
    }
    Some(Rect::new(
        u16::try_from(left).ok()?,
        u16::try_from(top).ok()?,
        u16::try_from(right - left).ok()?,
        u16::try_from(bottom - top).ok()?,
    ))
}

fn intersect_local_rect(area: Rect, slot: ProjectedRect, local: Rect) -> Rect {
    let projected = ProjectedRect {
        x: slot.x.saturating_add(i64::from(local.x)),
        y: slot.y.saturating_add(i64::from(local.y)),
        width: local.width,
        height: local.height,
    };
    intersect_projected_rect(area, projected).unwrap_or_default()
}

#[cfg(test)]
pub(super) fn placement_bounds(placement: CanvasNodePlacement) -> Rect {
    let x = placement.incoming_button.x.min(placement.area.x);
    let right = placement
        .outgoing_button
        .right()
        .max(placement.area.right());
    Rect::new(
        x,
        placement.area.y,
        right.saturating_sub(x),
        placement.area.height,
    )
}

#[cfg(test)]
pub(super) fn world_rects_overlap(left: WorldRect, right: WorldRect) -> bool {
    left.x < right.x.saturating_add(i32::from(right.width))
        && right.x < left.x.saturating_add(i32::from(left.width))
        && left.y < right.y.saturating_add(i32::from(right.height))
        && right.y < left.y.saturating_add(i32::from(left.height))
}

fn node_placement(
    node_id: NodeId,
    slot: ProjectedRect,
    visible_slot: Rect,
    canvas: Rect,
) -> CanvasNodePlacement {
    let button_width = 3.min(slot.width / 3);
    let local_area = Rect::new(
        button_width,
        0,
        slot.width.saturating_sub(button_width * 2),
        slot.height,
    );
    let local_incoming_button = Rect::new(0, slot.height / 2, button_width, 1);
    let local_outgoing_button = Rect::new(local_area.right(), slot.height / 2, button_width, 1);
    CanvasNodePlacement {
        node_id,
        slot,
        visible_slot,
        area: intersect_local_rect(canvas, slot, local_area),
        incoming_button: intersect_local_rect(canvas, slot, local_incoming_button),
        outgoing_button: intersect_local_rect(canvas, slot, local_outgoing_button),
        local_area,
        local_incoming_button,
        local_outgoing_button,
    }
}

fn node_slot_width(graph: &RelationGraph, node_id: NodeId) -> u16 {
    let maximum_content_width = usize::from(u16::MAX - NODE_CHROME_WIDTH);
    let content_width = graph
        .node(node_id)
        .map(|node| UnicodeWidthStr::width(node.symbol.as_str()))
        .unwrap_or_default()
        .min(maximum_content_width);
    ROOT_NODE_WIDTH.max(
        u16::try_from(content_width)
            .expect("content width was capped to u16")
            .saturating_add(NODE_CHROME_WIDTH),
    )
}
