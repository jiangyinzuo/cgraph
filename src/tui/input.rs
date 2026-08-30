use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use crate::{
    app::{App, HierarchyLoadRequest, SearchKind, SearchRequest},
    state::{HierarchyDirection, NodeId, SourceLocation},
};

use super::{
    canvas::{CanvasNodePlacement, canvas_layout, world_canvas_layout},
    help, messages, save, search,
    view::canvas_inner_area,
};

pub(super) enum InteractionRequest {
    Search(SearchRequest),
    Hierarchy(Vec<HierarchyLoadRequest>),
    OpenLocation(SourceLocation),
    EditConfig,
    OpenMessages,
    CopyMessages(String),
}

#[derive(Default)]
pub(super) struct CanvasDragState {
    pub(super) previous: Option<(u16, u16)>,
    pressed_node: Option<NodeId>,
    dragged: bool,
    last_click: Option<CanvasClick>,
}

struct CanvasClick {
    node_id: NodeId,
    at: Instant,
}

const DOUBLE_CLICK_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NavigationDirection {
    Left,
    Right,
    Up,
    Down,
}

#[cfg(test)]
pub(super) fn handle_event(
    app: &mut App,
    event: Event,
    analysis_available: bool,
    screen: Rect,
    canvas_drag: &mut CanvasDragState,
) -> Option<InteractionRequest> {
    let mut message_view = None;
    handle_event_with_messages(
        app,
        event,
        analysis_available,
        screen,
        canvas_drag,
        &mut message_view,
    )
}

pub(super) fn handle_event_with_messages(
    app: &mut App,
    event: Event,
    analysis_available: bool,
    screen: Rect,
    canvas_drag: &mut CanvasDragState,
    message_view: &mut Option<messages::MessageViewState>,
) -> Option<InteractionRequest> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            let request = if let Some(view) = message_view.as_mut() {
                match messages::handle_key(view, key) {
                    messages::MessageAction::Close => {
                        *message_view = None;
                        None
                    }
                    messages::MessageAction::Copy(text) => {
                        Some(InteractionRequest::CopyMessages(text))
                    }
                    messages::MessageAction::Handled => None,
                }
            } else if app.help.is_some() {
                help::handle_key(app, key);
                None
            } else if app.search.is_some() {
                search::handle_key(app, key).map(InteractionRequest::Search)
            } else if app.save.is_some() {
                save::handle_key(app, key);
                None
            } else {
                handle_canvas_key(app, key, analysis_available, screen)
            };
            if app.search.is_some() || app.save.is_some() || app.help.is_some() {
                *canvas_drag = CanvasDragState::default();
            }
            request
        }
        Event::Mouse(mouse) => {
            if message_view.is_some() {
                *canvas_drag = CanvasDragState::default();
                None
            } else if app.help.is_some() {
                *canvas_drag = CanvasDragState::default();
                help::handle_mouse(app, mouse);
                None
            } else if app.search.is_some() {
                *canvas_drag = CanvasDragState::default();
                search::handle_mouse(app, mouse, screen);
                None
            } else if app.save.is_some() {
                *canvas_drag = CanvasDragState::default();
                None
            } else {
                handle_canvas_mouse(app, mouse, analysis_available, screen, canvas_drag)
            }
        }
        _ => None,
    }
}

pub(super) fn handle_canvas_key(
    app: &mut App,
    key: KeyEvent,
    analysis_available: bool,
    screen: Rect,
) -> Option<InteractionRequest> {
    if let Some(prefix) = app.pending_key.take() {
        return match prefix {
            'a' => match key.code {
                KeyCode::Char('c') if key.modifiers == KeyModifiers::NONE => app
                    .open_search(SearchKind::Call, analysis_available)
                    .map(InteractionRequest::Search),
                KeyCode::Char('t') if key.modifiers == KeyModifiers::NONE => app
                    .open_search(SearchKind::Type, analysis_available)
                    .map(InteractionRequest::Search),
                _ => None,
            },
            'd' => {
                match key.code {
                    KeyCode::Char('d') if key.modifiers == KeyModifiers::NONE => {
                        app.delete_selected_anchor();
                    }
                    KeyCode::Char('p') if key.modifiers == KeyModifiers::NONE => {
                        app.delete_selected_branch(HierarchyDirection::Incoming);
                    }
                    KeyCode::Char('n') if key.modifiers == KeyModifiers::NONE => {
                        app.delete_selected_branch(HierarchyDirection::Outgoing);
                    }
                    _ => {}
                }
                None
            }
            'e' => {
                if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::NONE {
                    return Some(InteractionRequest::EditConfig);
                }
                None
            }
            't' => {
                match key.code {
                    KeyCode::Char('l') if key.modifiers == KeyModifiers::NONE => {
                        return toggle_selected_branch_stably(
                            app,
                            HierarchyDirection::Incoming,
                            analysis_available,
                        )
                        .map(|request| InteractionRequest::Hierarchy(vec![request]));
                    }
                    KeyCode::Char('r') if key.modifiers == KeyModifiers::NONE => {
                        return toggle_selected_branch_stably(
                            app,
                            HierarchyDirection::Outgoing,
                            analysis_available,
                        )
                        .map(|request| InteractionRequest::Hierarchy(vec![request]));
                    }
                    _ => {}
                }
                None
            }
            'g' => {
                if key.code == KeyCode::Char('<') && key.modifiers == KeyModifiers::NONE {
                    return Some(InteractionRequest::OpenMessages);
                }
                None
            }
            _ => None,
        };
    }

    match key.code {
        KeyCode::Char('a') if key.modifiers == KeyModifiers::NONE => {
            app.pending_key = Some('a');
        }
        KeyCode::Char('d') if key.modifiers == KeyModifiers::NONE => {
            app.pending_key = Some('d');
        }
        KeyCode::Char('e') if key.modifiers == KeyModifiers::NONE => {
            app.pending_key = Some('e');
        }
        KeyCode::Char('t') if key.modifiers == KeyModifiers::NONE => {
            app.pending_key = Some('t');
        }
        KeyCode::Char('g') if key.modifiers == KeyModifiers::NONE => {
            app.pending_key = Some('g');
        }
        KeyCode::Left | KeyCode::Char('h') if key.modifiers == KeyModifiers::NONE => {
            move_canvas_selection(app, NavigationDirection::Left, screen);
        }
        KeyCode::Right | KeyCode::Char('l') if key.modifiers == KeyModifiers::NONE => {
            move_canvas_selection(app, NavigationDirection::Right, screen);
        }
        KeyCode::Up | KeyCode::Char('k') if key.modifiers == KeyModifiers::NONE => {
            move_canvas_selection(app, NavigationDirection::Up, screen);
        }
        KeyCode::Down | KeyCode::Char('j') if key.modifiers == KeyModifiers::NONE => {
            move_canvas_selection(app, NavigationDirection::Down, screen);
        }
        KeyCode::Char('r') if key.modifiers == KeyModifiers::NONE => {
            let requests = app.refresh_selected_branches(analysis_available);
            if !requests.is_empty() {
                return Some(InteractionRequest::Hierarchy(requests));
            }
        }
        KeyCode::Char('w') if key.modifiers == KeyModifiers::NONE => app.open_save(),
        KeyCode::Char('?') if matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            app.open_help();
        }
        KeyCode::Char('q') | KeyCode::Esc => app.quit(),
        _ => {}
    }

    None
}

pub(super) fn handle_canvas_mouse(
    app: &mut App,
    mouse: MouseEvent,
    hierarchy_available: bool,
    screen: Rect,
    drag: &mut CanvasDragState,
) -> Option<InteractionRequest> {
    match mouse.kind {
        MouseEventKind::Drag(MouseButton::Left) => {
            let (previous_column, previous_row) = drag.previous?;
            drag.dragged = true;
            drag.last_click = None;
            app.pan_viewport(
                i32::from(mouse.column) - i32::from(previous_column),
                i32::from(mouse.row) - i32::from(previous_row),
            );
            drag.previous = Some((mouse.column, mouse.row));
            return None;
        }
        MouseEventKind::Up(MouseButton::Left) => {
            let request = finish_canvas_click(app, mouse, screen, drag);
            drag.previous = None;
            drag.pressed_node = None;
            drag.dragged = false;
            return request;
        }
        MouseEventKind::Down(MouseButton::Left) => {}
        _ => return None,
    }
    let point = (mouse.column, mouse.row).into();
    let canvas = canvas_inner_area(screen);
    if !canvas.contains(point) {
        *drag = CanvasDragState::default();
        return None;
    }
    let layout = canvas_layout(canvas, &app.graph, app.selected, app.viewport);

    if let Some(node_id) = layout
        .nodes
        .iter()
        .find(|placement| placement.incoming_button.contains(point))
        .map(|placement| placement.node_id)
    {
        *drag = CanvasDragState::default();
        return with_stable_node_position(app, node_id, |app| {
            app.toggle_node_branch(node_id, HierarchyDirection::Incoming, hierarchy_available)
        })
        .map(|request| InteractionRequest::Hierarchy(vec![request]));
    }
    if let Some(node_id) = layout
        .nodes
        .iter()
        .find(|placement| placement.outgoing_button.contains(point))
        .map(|placement| placement.node_id)
    {
        *drag = CanvasDragState::default();
        return with_stable_node_position(app, node_id, |app| {
            app.toggle_node_branch(node_id, HierarchyDirection::Outgoing, hierarchy_available)
        })
        .map(|request| InteractionRequest::Hierarchy(vec![request]));
    }
    if let Some(node_id) = layout
        .nodes
        .iter()
        .find(|placement| placement.area.contains(point))
        .map(|placement| placement.node_id)
    {
        with_stable_node_position(app, node_id, |app| app.select_node(node_id));
        drag.pressed_node = Some(node_id);
    } else {
        drag.pressed_node = None;
        drag.last_click = None;
    }
    drag.dragged = false;
    drag.previous = Some((mouse.column, mouse.row));
    None
}

fn finish_canvas_click(
    app: &mut App,
    mouse: MouseEvent,
    screen: Rect,
    drag: &mut CanvasDragState,
) -> Option<InteractionRequest> {
    if drag.dragged {
        return None;
    }
    let node_id = drag.pressed_node?;
    let point = (mouse.column, mouse.row).into();
    let canvas = canvas_inner_area(screen);
    if !canvas.contains(point) {
        drag.last_click = None;
        return None;
    }
    let released_on_same_node = canvas_layout(canvas, &app.graph, app.selected, app.viewport)
        .nodes
        .iter()
        .any(|placement| placement.node_id == node_id && placement.area.contains(point));
    if !released_on_same_node {
        drag.last_click = None;
        return None;
    }

    let now = Instant::now();
    let is_double_click = drag.last_click.as_ref().is_some_and(|click| {
        click.node_id == node_id && now.duration_since(click.at) <= DOUBLE_CLICK_TIMEOUT
    });
    if !is_double_click {
        drag.last_click = Some(CanvasClick { node_id, at: now });
        return None;
    }
    drag.last_click = None;

    let Some(location) = app
        .graph
        .node(node_id)
        .and_then(|node| node.location.clone())
        .filter(|location| {
            !location.uri.is_empty() && location.line.is_some() && location.character.is_some()
        })
    else {
        app.set_canvas_notice("Selected node has no exact source location");
        return None;
    };
    Some(InteractionRequest::OpenLocation(location))
}

pub(super) fn move_canvas_selection(
    app: &mut App,
    direction: NavigationDirection,
    screen: Rect,
) -> bool {
    let layout = canvas_layout(
        canvas_inner_area(screen),
        &app.graph,
        app.selected,
        app.viewport,
    );
    let Some(current) = current_placement(&layout.nodes, app.selected) else {
        return false;
    };
    let current_center = rect_center(current.visible_slot);
    let next = layout
        .nodes
        .iter()
        .filter(|candidate| candidate.node_id != current.node_id)
        .filter_map(|candidate| {
            let candidate_center = rect_center(candidate.visible_slot);
            navigation_score(current_center, candidate_center, direction)
                .map(|score| (score, candidate.node_id))
        })
        .min_by_key(|(score, node_id)| (*score, node_id.0));
    let Some((_, node_id)) = next else {
        return false;
    };
    with_stable_node_position(app, node_id, |app| app.select_node(node_id))
}

fn toggle_selected_branch_stably(
    app: &mut App,
    direction: HierarchyDirection,
    hierarchy_available: bool,
) -> Option<HierarchyLoadRequest> {
    let selected = app.selected?;
    with_stable_node_position(app, selected, |app| {
        app.toggle_selected_branch(direction, hierarchy_available)
    })
}

pub(super) fn with_stable_node_position<T>(
    app: &mut App,
    node_id: NodeId,
    mutation: impl FnOnce(&mut App) -> T,
) -> T {
    let before = node_world_anchor(app, node_id);
    let result = mutation(app);
    let after = node_world_anchor(app, node_id);
    if let (Some((before_x, before_y)), Some((after_x, after_y))) = (before, after) {
        app.pan_viewport(
            before_x.saturating_sub(after_x),
            before_y.saturating_sub(after_y),
        );
    }
    result
}

fn node_world_anchor(app: &App, node_id: NodeId) -> Option<(i32, i32)> {
    let node_id = app.graph.resolve_id(node_id)?;
    world_canvas_layout(&app.graph, app.selected)
        .nodes
        .into_iter()
        .find(|placement| placement.node_id == node_id)
        .map(|placement| {
            (
                placement
                    .slot
                    .x
                    .saturating_add(i32::from(placement.slot.width) / 2),
                placement
                    .slot
                    .y
                    .saturating_add(i32::from(placement.slot.height) / 2),
            )
        })
}

fn current_placement(
    layout: &[CanvasNodePlacement],
    selected: Option<NodeId>,
) -> Option<&CanvasNodePlacement> {
    selected
        .and_then(|selected| {
            layout
                .iter()
                .find(|placement| placement.node_id == selected)
        })
        .or_else(|| layout.first())
}

fn navigation_score(
    current: (i32, i32),
    candidate: (i32, i32),
    direction: NavigationDirection,
) -> Option<(i64, i32, i32)> {
    let delta_x = candidate.0 - current.0;
    let delta_y = candidate.1 - current.1;
    let (primary, perpendicular) = match direction {
        NavigationDirection::Left if delta_x < 0 => (-delta_x, delta_y.abs()),
        NavigationDirection::Right if delta_x > 0 => (delta_x, delta_y.abs()),
        NavigationDirection::Up if delta_y < 0 => (-delta_y, delta_x.abs()),
        NavigationDirection::Down if delta_y > 0 => (delta_y, delta_x.abs()),
        _ => return None,
    };
    let distance = i64::from(primary).pow(2) + i64::from(perpendicular).pow(2);
    Some((distance, perpendicular, primary))
}

pub(super) fn rect_center(area: Rect) -> (i32, i32) {
    (
        i32::from(area.x) + i32::from(area.width) / 2,
        i32::from(area.y) + i32::from(area.height) / 2,
    )
}
