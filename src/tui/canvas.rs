use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use petgraph::{algo::kosaraju_scc, graph::DiGraph};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};
use unicode_width::UnicodeWidthStr;

use crate::state::{NodeId, Viewport, graph::RelationGraph};

const ROOT_NODE_WIDTH: u16 = 28;
const ROOT_NODE_HEIGHT: u16 = 3;
const ROOT_COLUMN_GAP: u16 = 4;
const ROOT_ROW_GAP: u16 = 1;
const NODE_CHROME_WIDTH: u16 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CanvasNodePlacement {
    pub(super) node_id: NodeId,
    pub(super) area: Rect,
    pub(super) incoming_button: Rect,
    pub(super) outgoing_button: Rect,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CanvasLineCell {
    pub(super) x: u16,
    pub(super) y: u16,
    pub(super) symbol: char,
    pub(super) visual_kind: EdgeVisualKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CanvasEdge {
    pub(super) source_id: NodeId,
    pub(super) target_id: NodeId,
    pub(super) visual_kind: EdgeVisualKind,
    pub(super) cells: Vec<CanvasLineCell>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct CanvasLayoutSnapshot {
    pub(super) nodes: Vec<CanvasNodePlacement>,
    pub(super) edges: Vec<CanvasEdge>,
}

pub(super) struct CanvasConnections<'a> {
    pub(super) edges: &'a [CanvasEdge],
}

impl Widget for CanvasConnections<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        for cell in self.edges.iter().flat_map(|edge| &edge.cells) {
            if !area.contains((cell.x, cell.y).into()) {
                continue;
            }
            let Some(target) = buffer.cell_mut((cell.x, cell.y)) else {
                continue;
            };
            let symbol = merge_connection_symbol(target.symbol(), cell.symbol);
            let color = if cell.visual_kind.is_special() {
                Color::Yellow
            } else {
                Color::DarkGray
            };
            target
                .set_char(symbol)
                .set_style(Style::default().fg(color));
        }
    }
}

pub(super) fn canvas_layout(
    area: Rect,
    graph: &RelationGraph,
    selected: Option<NodeId>,
    viewport: Viewport,
) -> CanvasLayoutSnapshot {
    let world = world_canvas_layout(graph, selected);
    let nodes = world
        .nodes
        .iter()
        .filter_map(|placement| {
            project_world_slot(area, placement.slot, viewport)
                .map(|slot| node_placement(placement.node_id, slot))
        })
        .collect::<Vec<_>>();
    let mut edges = world
        .edges
        .iter()
        .filter_map(|edge| route_edge(*edge, &nodes))
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
    let selected_rank = component_ranks[&component_by_node[&selected]];

    let mut nodes_by_rank = BTreeMap::<i32, Vec<NodeId>>::new();
    for node_id in &visible.nodes {
        let rank = component_ranks[&component_by_node[node_id]] - selected_rank;
        nodes_by_rank.entry(rank).or_default().push(*node_id);
    }
    if let Some(nodes) = nodes_by_rank.get_mut(&0)
        && let Some(index) = nodes.iter().position(|node_id| *node_id == selected)
    {
        nodes.swap(0, index);
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

fn project_world_slot(area: Rect, slot: WorldRect, viewport: Viewport) -> Option<Rect> {
    if slot.width > area.width || slot.height > area.height {
        return None;
    }
    let anchor_x = area.x + area.width / 2;
    let anchor_y = area.y + area.height / 2;
    let screen_x = i64::from(anchor_x) + i64::from(slot.x) + i64::from(viewport.offset_x);
    let screen_y = i64::from(anchor_y) + i64::from(slot.y) + i64::from(viewport.offset_y);
    let right = screen_x + i64::from(slot.width);
    let bottom = screen_y + i64::from(slot.height);
    if screen_x < i64::from(area.x)
        || screen_y < i64::from(area.y)
        || right > i64::from(area.right())
        || bottom > i64::from(area.bottom())
    {
        return None;
    }
    Some(Rect::new(
        u16::try_from(screen_x).ok()?,
        u16::try_from(screen_y).ok()?,
        slot.width,
        slot.height,
    ))
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

fn route_edge(edge: LayoutEdge, placements: &[CanvasNodePlacement]) -> Option<CanvasEdge> {
    let source = placements
        .iter()
        .find(|placement| placement.node_id == edge.source_id)?;
    let target = placements
        .iter()
        .find(|placement| placement.node_id == edge.target_id)?;
    let cells = match edge.visual_kind {
        EdgeVisualKind::Forward => orthogonal_connection(
            (
                source.outgoing_button.right().saturating_sub(1),
                source.outgoing_button.y,
            ),
            (target.incoming_button.x, target.incoming_button.y),
            edge.visual_kind,
        ),
        EdgeVisualKind::BackOrCycle => back_connection(source, target),
        EdgeVisualKind::SelfLoop => self_loop_connection(source),
    };
    Some(CanvasEdge {
        source_id: edge.source_id,
        target_id: edge.target_id,
        visual_kind: edge.visual_kind,
        cells,
    })
}

fn back_connection(
    source: &CanvasNodePlacement,
    target: &CanvasNodePlacement,
) -> Vec<CanvasLineCell> {
    let from = (source.incoming_button.x, source.incoming_button.y);
    let to = (
        target.outgoing_button.right().saturating_sub(1),
        target.outgoing_button.y,
    );
    let channel_x = from.0.min(to.0).saturating_sub(2);
    let mut cells = polyline_connection(
        &[from, (channel_x, from.1), (channel_x, to.1), to],
        EdgeVisualKind::BackOrCycle,
    );
    push_connection_cell(
        &mut cells,
        target.outgoing_button.right(),
        target.outgoing_button.y,
        '◀',
        EdgeVisualKind::BackOrCycle,
    );
    cells
}

fn self_loop_connection(source: &CanvasNodePlacement) -> Vec<CanvasLineCell> {
    let from = (
        source.outgoing_button.right().saturating_sub(1),
        source.outgoing_button.y,
    );
    let channel_x = source.outgoing_button.right().saturating_add(2);
    let lower_y = source.area.bottom();
    let mut cells = polyline_connection(
        &[
            from,
            (channel_x, from.1),
            (channel_x, lower_y),
            (source.outgoing_button.right(), lower_y),
        ],
        EdgeVisualKind::SelfLoop,
    );
    push_connection_cell(
        &mut cells,
        source.outgoing_button.right(),
        source.outgoing_button.y.saturating_sub(1),
        '↺',
        EdgeVisualKind::SelfLoop,
    );
    cells
}

fn orthogonal_connection(
    from: (u16, u16),
    to: (u16, u16),
    visual_kind: EdgeVisualKind,
) -> Vec<CanvasLineCell> {
    let bend_x = from.0.midpoint(to.0);
    polyline_connection(&[from, (bend_x, from.1), (bend_x, to.1), to], visual_kind)
}

fn polyline_connection(points: &[(u16, u16)], visual_kind: EdgeVisualKind) -> Vec<CanvasLineCell> {
    let mut cells = Vec::new();
    for segment in points.windows(2) {
        let from = segment[0];
        let to = segment[1];
        if from.1 == to.1 {
            let symbol = if visual_kind.is_special() {
                '═'
            } else {
                '─'
            };
            for x in from.0.min(to.0)..=from.0.max(to.0) {
                push_connection_cell(&mut cells, x, from.1, symbol, visual_kind);
            }
        } else {
            let symbol = if visual_kind.is_special() {
                '║'
            } else {
                '│'
            };
            for y in from.1.min(to.1)..=from.1.max(to.1) {
                push_connection_cell(&mut cells, from.0, y, symbol, visual_kind);
            }
        }
    }
    cells
}

fn push_connection_cell(
    cells: &mut Vec<CanvasLineCell>,
    x: u16,
    y: u16,
    symbol: char,
    visual_kind: EdgeVisualKind,
) {
    if let Some(existing) = cells.iter_mut().find(|cell| cell.x == x && cell.y == y) {
        existing.symbol = merge_connection_symbol(&existing.symbol.to_string(), symbol);
        if visual_kind.is_special() {
            existing.visual_kind = visual_kind;
        }
    } else {
        cells.push(CanvasLineCell {
            x,
            y,
            symbol,
            visual_kind,
        });
    }
}

fn merge_connection_symbol(existing: &str, incoming: char) -> char {
    match (existing, incoming) {
        ("", incoming) | (" ", incoming) => incoming,
        ("─", '─') => '─',
        ("│", '│') => '│',
        ("═", '═') => '═',
        ("║", '║') => '║',
        ("┼", '─' | '│') => '┼',
        ("╬", _) | (_, '╬') => '╬',
        ("═" | "║", '─' | '│' | '═' | '║') => '╬',
        ("─" | "│", '═' | '║') => '╬',
        (_, '◀' | '▶' | '▲' | '▼' | '↺') => incoming,
        ("◀" | "▶" | "▲" | "▼" | "↺", _) => existing.chars().next().unwrap_or(incoming),
        _ => '┼',
    }
}

fn node_placement(node_id: NodeId, slot: Rect) -> CanvasNodePlacement {
    let button_width = 3.min(slot.width / 3);
    let area = Rect::new(
        slot.x + button_width,
        slot.y,
        slot.width.saturating_sub(button_width * 2),
        slot.height,
    );
    CanvasNodePlacement {
        node_id,
        area,
        incoming_button: Rect::new(slot.x, slot.y + slot.height / 2, button_width, 1),
        outgoing_button: Rect::new(area.right(), slot.y + slot.height / 2, button_width, 1),
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
