use std::collections::BTreeMap;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Widget,
};

use super::{EdgeVisualKind, LayoutEdge, ProjectedNodePlacement, ProjectedRect};
use crate::state::NodeId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tui) struct CanvasLineCell {
    pub(in crate::tui) x: u16,
    pub(in crate::tui) y: u16,
    pub(in crate::tui) symbol: char,
    pub(in crate::tui) visual_kind: EdgeVisualKind,
    directions: ConnectionDirections,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tui) struct CanvasEdge {
    pub(in crate::tui) source_id: NodeId,
    pub(in crate::tui) target_id: NodeId,
    pub(in crate::tui) visual_kind: EdgeVisualKind,
    pub(in crate::tui) cells: Vec<CanvasLineCell>,
}

pub(in crate::tui) struct CanvasConnections<'a> {
    pub(in crate::tui) edges: &'a [CanvasEdge],
}

impl Widget for CanvasConnections<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let mut grouped = BTreeMap::<(u16, u16), Vec<(usize, &CanvasLineCell)>>::new();
        for (edge_index, edge) in self.edges.iter().enumerate() {
            for cell in &edge.cells {
                if area.contains((cell.x, cell.y).into()) {
                    grouped
                        .entry((cell.x, cell.y))
                        .or_default()
                        .push((edge_index, cell));
                }
            }
        }
        for ((x, y), cells) in grouped {
            let Some(target) = buffer.cell_mut((x, y)) else {
                continue;
            };
            let crossing = different_edges_cross(&cells);
            let symbol = merged_connection_symbol(&cells, crossing);
            let special = cells.iter().any(|(_, cell)| cell.visual_kind.is_special());
            let style = if crossing {
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD)
            } else if special {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            target.set_char(symbol).set_style(style);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ConnectionDirections(u8);

impl ConnectionDirections {
    const LEFT: Self = Self(1 << 0);
    const RIGHT: Self = Self(1 << 1);
    const UP: Self = Self(1 << 2);
    const DOWN: Self = Self(1 << 3);
    const ALL: Self = Self(Self::LEFT.0 | Self::RIGHT.0 | Self::UP.0 | Self::DOWN.0);

    fn insert(&mut self, direction: Self) {
        self.0 |= direction.0;
    }

    fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    fn has_horizontal(self) -> bool {
        self.intersects(Self(Self::LEFT.0 | Self::RIGHT.0))
    }

    fn has_vertical(self) -> bool {
        self.intersects(Self(Self::UP.0 | Self::DOWN.0))
    }

    fn is_empty(self) -> bool {
        self.0 == 0
    }
}

pub(super) fn route_edge(
    edge: LayoutEdge,
    placements: &[ProjectedNodePlacement],
    area: Rect,
) -> Option<CanvasEdge> {
    let source = placements
        .iter()
        .find(|placement| placement.node_id == edge.source_id)?;
    let target = placements
        .iter()
        .find(|placement| placement.node_id == edge.target_id)?;
    let cells = match edge.visual_kind {
        EdgeVisualKind::Forward => {
            let from = (
                projected_outgoing_x(source.slot),
                projected_center_y(source.slot),
            );
            let target_arrow = (
                target.slot.x.saturating_sub(1),
                projected_center_y(target.slot),
            );
            let mut cells = orthogonal_connection(from, target_arrow, edge.visual_kind, area);
            push_connection_marker(
                &mut cells,
                target_arrow.0,
                target_arrow.1,
                '▶',
                edge.visual_kind,
                area,
            );
            cells
        }
        EdgeVisualKind::BackOrCycle => back_connection(source, target, area),
        EdgeVisualKind::SelfLoop => self_loop_connection(source, area),
    };
    (!cells.is_empty()).then_some(CanvasEdge {
        source_id: edge.source_id,
        target_id: edge.target_id,
        visual_kind: edge.visual_kind,
        cells,
    })
}

fn back_connection(
    source: &ProjectedNodePlacement,
    target: &ProjectedNodePlacement,
    area: Rect,
) -> Vec<CanvasLineCell> {
    let from = (source.slot.x, projected_center_y(source.slot));
    let to = (
        projected_outgoing_x(target.slot),
        projected_center_y(target.slot),
    );
    let channel_x = from.0.min(to.0).saturating_sub(2);
    let mut cells = polyline_connection(
        &[from, (channel_x, from.1), (channel_x, to.1), to],
        EdgeVisualKind::BackOrCycle,
        area,
    );
    push_connection_marker(
        &mut cells,
        projected_right(target.slot),
        projected_center_y(target.slot),
        '◀',
        EdgeVisualKind::BackOrCycle,
        area,
    );
    cells
}

fn self_loop_connection(source: &ProjectedNodePlacement, area: Rect) -> Vec<CanvasLineCell> {
    let right = projected_right(source.slot);
    let center_y = projected_center_y(source.slot);
    let from = (projected_outgoing_x(source.slot), center_y);
    let channel_x = right.saturating_add(2);
    let lower_y = source.slot.y.saturating_add(i64::from(source.slot.height));
    let mut cells = polyline_connection(
        &[
            from,
            (channel_x, from.1),
            (channel_x, lower_y),
            (right, lower_y),
        ],
        EdgeVisualKind::SelfLoop,
        area,
    );
    push_connection_marker(
        &mut cells,
        right,
        center_y.saturating_sub(1),
        '↺',
        EdgeVisualKind::SelfLoop,
        area,
    );
    cells
}

fn orthogonal_connection(
    from: (i64, i64),
    to: (i64, i64),
    visual_kind: EdgeVisualKind,
    area: Rect,
) -> Vec<CanvasLineCell> {
    let bend_x = from.0.midpoint(to.0);
    polyline_connection(
        &[from, (bend_x, from.1), (bend_x, to.1), to],
        visual_kind,
        area,
    )
}

fn polyline_connection(
    points: &[(i64, i64)],
    visual_kind: EdgeVisualKind,
    area: Rect,
) -> Vec<CanvasLineCell> {
    let mut directions = BTreeMap::<(u16, u16), ConnectionDirections>::new();
    if area.is_empty() {
        return Vec::new();
    }
    let left = i64::from(area.x);
    let right = i64::from(area.right()).saturating_sub(1);
    let top = i64::from(area.y);
    let bottom = i64::from(area.bottom()).saturating_sub(1);
    for segment in points.windows(2) {
        let from = segment[0];
        let to = segment[1];
        if from.1 == to.1 && (top..=bottom).contains(&from.1) {
            let start = from.0.min(to.0);
            let end = from.0.max(to.0);
            let visible_start = start.max(left);
            let visible_end = end.min(right);
            for x in visible_start..=visible_end {
                let cell = directions
                    .entry((
                        u16::try_from(x).expect("clipped x fits"),
                        u16::try_from(from.1).expect("clipped y fits"),
                    ))
                    .or_default();
                if x > start {
                    cell.insert(ConnectionDirections::LEFT);
                }
                if x < end {
                    cell.insert(ConnectionDirections::RIGHT);
                }
            }
        } else if from.0 == to.0 && (left..=right).contains(&from.0) {
            let start = from.1.min(to.1);
            let end = from.1.max(to.1);
            let visible_start = start.max(top);
            let visible_end = end.min(bottom);
            for y in visible_start..=visible_end {
                let cell = directions
                    .entry((
                        u16::try_from(from.0).expect("clipped x fits"),
                        u16::try_from(y).expect("clipped y fits"),
                    ))
                    .or_default();
                if y > start {
                    cell.insert(ConnectionDirections::UP);
                }
                if y < end {
                    cell.insert(ConnectionDirections::DOWN);
                }
            }
        }
    }
    directions
        .into_iter()
        .filter(|(_, directions)| !directions.is_empty())
        .map(|((x, y), directions)| CanvasLineCell {
            x,
            y,
            symbol: connection_symbol(directions, visual_kind.is_special()),
            visual_kind,
            directions,
        })
        .collect()
}

fn push_connection_marker(
    cells: &mut Vec<CanvasLineCell>,
    x: i64,
    y: i64,
    symbol: char,
    visual_kind: EdgeVisualKind,
    area: Rect,
) {
    if x < i64::from(area.x)
        || x >= i64::from(area.right())
        || y < i64::from(area.y)
        || y >= i64::from(area.bottom())
    {
        return;
    }
    let x = u16::try_from(x).expect("marker x was clipped to a Rect");
    let y = u16::try_from(y).expect("marker y was clipped to a Rect");
    if let Some(existing) = cells.iter_mut().find(|cell| cell.x == x && cell.y == y) {
        existing.symbol = symbol;
        existing.visual_kind = visual_kind;
        existing.directions = ConnectionDirections::default();
    } else {
        cells.push(CanvasLineCell {
            x,
            y,
            symbol,
            visual_kind,
            directions: ConnectionDirections::default(),
        });
    }
}

fn projected_center_y(slot: ProjectedRect) -> i64 {
    slot.y.saturating_add(i64::from(slot.height) / 2)
}

fn projected_right(slot: ProjectedRect) -> i64 {
    slot.x.saturating_add(i64::from(slot.width))
}

fn projected_outgoing_x(slot: ProjectedRect) -> i64 {
    projected_right(slot).saturating_sub(1)
}

fn different_edges_cross(cells: &[(usize, &CanvasLineCell)]) -> bool {
    cells.iter().any(|(horizontal_edge, horizontal)| {
        horizontal.directions.has_horizontal()
            && cells.iter().any(|(vertical_edge, vertical)| {
                horizontal_edge != vertical_edge && vertical.directions.has_vertical()
            })
    })
}

fn merged_connection_symbol(cells: &[(usize, &CanvasLineCell)], crossing: bool) -> char {
    if let Some((_, marker)) = cells.iter().find(|(_, cell)| cell.directions.is_empty()) {
        return marker.symbol;
    }
    let (forward, special) = cells.iter().fold(
        (
            ConnectionDirections::default(),
            ConnectionDirections::default(),
        ),
        |(forward, special), (_, cell)| {
            if cell.visual_kind.is_special() {
                (forward, special.union(cell.directions))
            } else {
                (forward.union(cell.directions), special)
            }
        },
    );
    let combined = forward.union(special);
    if crossing && combined == ConnectionDirections::ALL {
        let special_horizontal = special.has_horizontal();
        let special_vertical = special.has_vertical();
        let forward_horizontal = forward.has_horizontal();
        let forward_vertical = forward.has_vertical();
        if special_horizontal && !special_vertical && forward_vertical && !forward_horizontal {
            return '╪';
        }
        if special_vertical && !special_horizontal && forward_horizontal && !forward_vertical {
            return '╫';
        }
    }
    connection_symbol(combined, !special.is_empty())
}

fn connection_symbol(directions: ConnectionDirections, double: bool) -> char {
    let left = ConnectionDirections::LEFT.0;
    let right = ConnectionDirections::RIGHT.0;
    let up = ConnectionDirections::UP.0;
    let down = ConnectionDirections::DOWN.0;
    match (directions.0, double) {
        (mask, false) if mask == left || mask == right || mask == left | right => '─',
        (mask, true) if mask == left || mask == right || mask == left | right => '═',
        (mask, false) if mask == up || mask == down || mask == up | down => '│',
        (mask, true) if mask == up || mask == down || mask == up | down => '║',
        (mask, false) if mask == right | down => '╭',
        (mask, false) if mask == left | down => '╮',
        (mask, false) if mask == right | up => '╰',
        (mask, false) if mask == left | up => '╯',
        (mask, true) if mask == right | down => '╔',
        (mask, true) if mask == left | down => '╗',
        (mask, true) if mask == right | up => '╚',
        (mask, true) if mask == left | up => '╝',
        (mask, false) if mask == right | up | down => '├',
        (mask, false) if mask == left | up | down => '┤',
        (mask, false) if mask == left | right | down => '┬',
        (mask, false) if mask == left | right | up => '┴',
        (mask, true) if mask == right | up | down => '╠',
        (mask, true) if mask == left | up | down => '╣',
        (mask, true) if mask == left | right | down => '╦',
        (mask, true) if mask == left | right | up => '╩',
        (_, false) => '┼',
        (_, true) => '╬',
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        style::{Color, Modifier},
        widgets::Widget,
    };

    use super::{CanvasConnections, CanvasEdge, EdgeVisualKind, NodeId, polyline_connection};

    #[test]
    fn orthogonal_bends_use_corner_glyphs_without_crossing_highlight() {
        let cells = polyline_connection(
            &[(1, 1), (4, 1), (4, 3), (7, 3)],
            EdgeVisualKind::Forward,
            Rect::new(0, 0, 10, 6),
        );
        assert_eq!(
            cells
                .iter()
                .find(|cell| (cell.x, cell.y) == (4, 1))
                .unwrap()
                .symbol,
            '╮'
        );
        assert_eq!(
            cells
                .iter()
                .find(|cell| (cell.x, cell.y) == (4, 3))
                .unwrap()
                .symbol,
            '╰'
        );
        assert!(!cells.iter().any(|cell| cell.symbol == '┼'));

        let edge = CanvasEdge {
            source_id: NodeId(1),
            target_id: NodeId(2),
            visual_kind: EdgeVisualKind::Forward,
            cells,
        };
        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 6));
        CanvasConnections { edges: &[edge] }.render(buffer.area, &mut buffer);
        let bend = buffer.cell((4, 1)).unwrap();
        assert_eq!(bend.symbol(), "╮");
        assert!(!bend.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn independent_single_line_edges_render_a_highlighted_crossing() {
        let horizontal = edge(1, EdgeVisualKind::Forward, &[(1, 3), (7, 3)]);
        let vertical = edge(2, EdgeVisualKind::Forward, &[(4, 1), (4, 5)]);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 7));
        CanvasConnections {
            edges: &[horizontal, vertical],
        }
        .render(buffer.area, &mut buffer);

        let crossing = buffer.cell((4, 3)).unwrap();
        assert_eq!(crossing.symbol(), "┼");
        assert_eq!(crossing.fg, Color::Magenta);
        assert!(crossing.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn mixed_single_and_double_crossings_preserve_axis_style() {
        let horizontal_double = edge(1, EdgeVisualKind::BackOrCycle, &[(1, 3), (7, 3)]);
        let vertical_single = edge(2, EdgeVisualKind::Forward, &[(4, 1), (4, 5)]);
        let horizontal_single = edge(3, EdgeVisualKind::Forward, &[(1, 6), (7, 6)]);
        let vertical_double = edge(4, EdgeVisualKind::BackOrCycle, &[(4, 4), (4, 8)]);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 10));
        CanvasConnections {
            edges: &[
                horizontal_double,
                vertical_single,
                horizontal_single,
                vertical_double,
            ],
        }
        .render(buffer.area, &mut buffer);

        assert_eq!(buffer.cell((4, 3)).unwrap().symbol(), "╪");
        assert_eq!(buffer.cell((4, 6)).unwrap().symbol(), "╫");
        assert!(
            buffer
                .cell((4, 6))
                .unwrap()
                .modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn clips_far_offscreen_segments_before_rasterizing_cells() {
        let area = Rect::new(10, 5, 8, 3);
        let cells = polyline_connection(
            &[(-1_000_000, 6), (1_000_000, 6)],
            EdgeVisualKind::Forward,
            area,
        );

        assert_eq!(cells.len(), usize::from(area.width));
        assert_eq!(cells.first().map(|cell| (cell.x, cell.y)), Some((10, 6)));
        assert_eq!(cells.last().map(|cell| (cell.x, cell.y)), Some((17, 6)));
        assert!(cells.iter().all(|cell| cell.symbol == '─'));
    }

    fn edge(id: u64, visual_kind: EdgeVisualKind, points: &[(i64, i64)]) -> CanvasEdge {
        CanvasEdge {
            source_id: NodeId(id),
            target_id: NodeId(id + 10),
            visual_kind,
            cells: polyline_connection(points, visual_kind, Rect::new(0, 0, 10, 10)),
        }
    }
}
