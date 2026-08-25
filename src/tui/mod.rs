#![doc = include_str!("README.md")]

use std::{
    io::{self, Stdout},
    sync::mpsc::{self, Sender},
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::{
    clipboard::CopyToClipboard,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
        size as terminal_size,
    },
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;

use crate::{
    app::{
        AnalysisBackend, AnalysisPhase, AnalysisStatus, App, HierarchyLoadRequest, SearchKind,
        SearchRequest,
    },
    fetch::{HierarchyClient, WorkspaceSymbolClient, lsp::LspStatusUpdate},
    ipc::{
        IpcCommand, IpcEventSender,
        protocol::{IpcRequest, IpcResponse},
    },
    state::{HierarchyDirection, NodeId, SourceLocation},
};

mod canvas;
mod config_editor;
mod help;
mod messages;
mod save;
mod search;

use canvas::{
    CanvasConnections, CanvasNodePlacement, CanvasNodeWidget, canvas_layout, world_canvas_layout,
};
#[cfg(test)]
use canvas::{EdgeVisualKind, placement_bounds, world_rects_overlap};

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

enum InteractionRequest {
    Search(SearchRequest),
    Hierarchy(Vec<HierarchyLoadRequest>),
    OpenLocation(SourceLocation),
    EditConfig,
    OpenMessages,
    CopyMessages(String),
}

struct HierarchyQueryEvent {
    request: HierarchyLoadRequest,
    result: Result<crate::fetch::HierarchyResponse, String>,
}

#[derive(Default)]
struct CanvasDragState {
    previous: Option<(u16, u16)>,
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
enum NavigationDirection {
    Left,
    Right,
    Up,
    Down,
}

pub fn init() -> Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

pub fn restore(terminal: &mut Tui) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn resume(terminal: &mut Tui) -> Result<()> {
    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    terminal.clear()?;
    terminal.hide_cursor()?;
    Ok(())
}

fn set_mouse_capture(terminal: &mut Tui, enabled: bool) -> Result<()> {
    if enabled {
        execute!(terminal.backend_mut(), EnableMouseCapture)?;
    } else {
        execute!(terminal.backend_mut(), DisableMouseCapture)?;
    }
    Ok(())
}

pub fn run(
    terminal: &mut Tui,
    app: &mut App,
    symbol_client: Option<WorkspaceSymbolClient>,
    hierarchy_client: Option<HierarchyClient>,
    mut lsp_status_receiver: Option<UnboundedReceiver<LspStatusUpdate>>,
    ipc_event_sender: Option<IpcEventSender>,
    mut ipc_command_receiver: Option<tokio::sync::mpsc::Receiver<IpcCommand>>,
) -> Result<()> {
    // Crossterm is currently polled synchronously, while LSP work runs on the
    // Tokio runtime. A small channel keeps async completion out of App and lets
    // the loop continue rendering loading state and receiving input.
    let (query_sender, query_receiver) = mpsc::channel::<search::QueryEvent>();
    let (hierarchy_sender, hierarchy_receiver) = mpsc::channel::<HierarchyQueryEvent>();
    let mut search_task: Option<JoinHandle<()>> = None;
    let mut hierarchy_tasks = Vec::<JoinHandle<()>>::new();
    let mut canvas_drag = CanvasDragState::default();
    let mut message_view: Option<messages::MessageViewState> = None;

    while !app.should_quit {
        if let Some(receiver) = ipc_command_receiver.as_mut() {
            while let Ok(command) = receiver.try_recv() {
                apply_ipc_command(app, command);
            }
        }
        if let Some(receiver) = lsp_status_receiver.as_mut() {
            while let Ok(status) = receiver.try_recv() {
                apply_lsp_status(app, status);
            }
        }
        while let Ok(query_event) = query_receiver.try_recv() {
            match query_event {
                search::QueryEvent::Started(request_id) => app.start_search(request_id),
                search::QueryEvent::Finished { request_id, result } => {
                    app.finish_search(request_id, result);
                }
            }
        }
        while let Ok(event) = hierarchy_receiver.try_recv() {
            with_stable_node_position(app, event.request.node_id, |app| {
                app.finish_hierarchy(&event.request, event.result);
            });
        }
        hierarchy_tasks.retain(|task| !task.is_finished());
        if let Some(view) = message_view.as_mut() {
            view.sync(&app.message_history);
        }
        terminal.draw(|frame| render_with_messages(frame, app, message_view.as_mut()))?;

        if event::poll(Duration::from_millis(50))? {
            let (width, height) = terminal_size()?;
            let screen = Rect::new(0, 0, width, height);
            let message_was_open = message_view.is_some();
            let request = handle_event_with_messages(
                app,
                event::read()?,
                hierarchy_client.is_some(),
                screen,
                &mut canvas_drag,
                &mut message_view,
            );
            if app.search.is_none()
                && let Some(task) = search_task.take()
            {
                task.abort();
            }
            match request {
                Some(InteractionRequest::Search(request)) => {
                    if let Some(client) = symbol_client.clone() {
                        let next_task = search::schedule(client, request, query_sender.clone());
                        if let Some(previous_task) = search_task.replace(next_task) {
                            previous_task.abort();
                        }
                    }
                }
                Some(InteractionRequest::Hierarchy(requests)) => {
                    if let Some(client) = hierarchy_client.clone() {
                        for request in requests {
                            hierarchy_tasks.push(schedule_hierarchy(
                                client.clone(),
                                request,
                                hierarchy_sender.clone(),
                            ));
                        }
                    }
                }
                Some(InteractionRequest::OpenLocation(location)) => {
                    send_open_location(app, ipc_event_sender.as_ref(), &location);
                }
                Some(InteractionRequest::EditConfig) => {
                    let requests = config_editor::edit_project_config(
                        terminal,
                        app,
                        hierarchy_client.is_some(),
                    )?;
                    if let Some(client) = hierarchy_client.clone() {
                        for request in requests {
                            hierarchy_tasks.push(schedule_hierarchy(
                                client.clone(),
                                request,
                                hierarchy_sender.clone(),
                            ));
                        }
                    }
                }
                Some(InteractionRequest::OpenMessages) => {
                    message_view = Some(messages::MessageViewState::from_messages(
                        &app.message_history,
                    ));
                }
                Some(InteractionRequest::CopyMessages(text)) => {
                    let character_count = text.chars().count();
                    let notice = match copy_message_to_clipboard(terminal, &text) {
                        Ok(()) => format!("yanked {character_count} chars via OSC 52"),
                        Err(error) => format!("copy failed: {error:#}"),
                    };
                    if let Some(view) = message_view.as_mut() {
                        view.set_copy_notice(notice);
                    }
                }
                None => {}
            }
            if message_was_open != message_view.is_some() {
                set_mouse_capture(terminal, message_view.is_none())?;
            }
        }
    }

    if let Some(task) = search_task {
        task.abort();
    }
    for task in hierarchy_tasks {
        task.abort();
    }

    Ok(())
}

fn copy_message_to_clipboard(terminal: &mut Tui, text: &str) -> Result<()> {
    execute!(
        terminal.backend_mut(),
        CopyToClipboard::to_clipboard_from(text)
    )?;
    Ok(())
}

fn apply_ipc_command(app: &mut App, command: IpcCommand) {
    let request_id = command.request_id();
    let (request, responder) = command.into_parts();
    let response = match request {
        IpcRequest::FocusSymbol {
            hierarchy,
            symbol,
            location,
        } => match app.focus_symbol(crate::state::SymbolIdentity {
            symbol: symbol.clone(),
            kind: hierarchy,
            location,
        }) {
            Ok(_) => {
                app.set_canvas_notice(format!(
                    "IPC request {request_id} focused {} symbol {symbol:?}",
                    match hierarchy {
                        crate::state::HierarchyKind::Call => "call",
                        crate::state::HierarchyKind::Type => "type",
                    }
                ));
                IpcResponse::Accepted
            }
            Err(message) => {
                app.set_canvas_error(format!("IPC request {request_id} rejected: {message}"));
                IpcResponse::Error { message }
            }
        },
    };
    if let Err(error) = responder.respond(response) {
        app.set_canvas_error(format!("IPC response {request_id} failed: {error:#}"));
    }
}

fn send_open_location(
    app: &mut App,
    event_sender: Option<&IpcEventSender>,
    location: &SourceLocation,
) {
    let Some(event_sender) = event_sender else {
        app.set_canvas_notice("IPC is not enabled; start cgraph with --ipc-socket <PATH>");
        return;
    };
    match event_sender.send_open_location(location) {
        Ok(1) => app.set_canvas_notice("Sent source location to 1 IPC client"),
        Ok(client_count) => {
            app.set_canvas_notice(format!(
                "Sent source location to {client_count} IPC clients"
            ));
        }
        Err(error) => app.set_canvas_error(format!("IPC open-location failed: {error:#}")),
    }
}

fn schedule_hierarchy(
    client: HierarchyClient,
    request: HierarchyLoadRequest,
    sender: Sender<HierarchyQueryEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let result = client
            .query(request.query.clone())
            .await
            .map_err(|error| format!("{error:#}"));
        let _ = sender.send(HierarchyQueryEvent { request, result });
    })
}

fn apply_lsp_status(app: &mut App, update: LspStatusUpdate) {
    let server = match &app.analysis_status.backend {
        AnalysisBackend::Lsp(server) => server.clone(),
        _ => "LSP".to_owned(),
    };
    let status = match update {
        LspStatusUpdate::Ready { message } => AnalysisStatus {
            backend: AnalysisBackend::Lsp(server),
            phase: AnalysisPhase::Ready,
            message,
            percentage: None,
        },
        LspStatusUpdate::Progress {
            title,
            message,
            percentage,
        } => AnalysisStatus {
            backend: AnalysisBackend::Lsp(server),
            phase: AnalysisPhase::Working,
            message: Some(match message {
                Some(message) => format!("{title}: {message}"),
                None => title,
            }),
            percentage: percentage.map(|percentage| percentage.min(100)),
        },
        LspStatusUpdate::Warning(message) => AnalysisStatus {
            backend: AnalysisBackend::Lsp(server),
            phase: AnalysisPhase::Warning,
            message: Some(message),
            percentage: None,
        },
        LspStatusUpdate::Error(message) => AnalysisStatus {
            backend: AnalysisBackend::Lsp(server),
            phase: AnalysisPhase::Error,
            message: Some(message),
            percentage: None,
        },
        LspStatusUpdate::Disconnected(message) => AnalysisStatus {
            backend: AnalysisBackend::Lsp(server),
            phase: AnalysisPhase::Disconnected,
            message: Some(message),
            percentage: None,
        },
    };
    app.set_analysis_status(status);
}

#[cfg(test)]
fn handle_event(
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

fn handle_event_with_messages(
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

fn handle_canvas_key(
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

fn handle_canvas_mouse(
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

fn move_canvas_selection(app: &mut App, direction: NavigationDirection, screen: Rect) -> bool {
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

fn with_stable_node_position<T>(
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

fn rect_center(area: Rect) -> (i32, i32) {
    (
        i32::from(area.x) + i32::from(area.width) / 2,
        i32::from(area.y) + i32::from(area.height) / 2,
    )
}

#[cfg(test)]
fn render(frame: &mut Frame, app: &App) {
    render_with_messages(frame, app, None);
}

fn render_with_messages(
    frame: &mut Frame,
    app: &App,
    mut message_view: Option<&mut messages::MessageViewState>,
) {
    let [canvas, message_area, footer] = canvas_message_and_footer(frame.area());

    let canvas_inner = canvas;

    if app.graph.anchors().is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Empty canvas",
                Style::default().fg(Color::DarkGray),
            )))
            .alignment(Alignment::Center),
            canvas_inner,
        );
    } else {
        let layout = canvas_layout(canvas_inner, &app.graph, app.selected, app.viewport);
        frame.render_widget(
            CanvasConnections {
                edges: &layout.edges,
            },
            canvas_inner,
        );
        for placement in layout.nodes {
            let node = app
                .graph
                .node(placement.node_id)
                .expect("canvas layout only contains existing nodes");
            frame.render_widget(
                CanvasNodeWidget {
                    node,
                    placement,
                    selected: app.selected == Some(placement.node_id),
                },
                canvas_inner,
            );
        }
    }

    frame.render_widget(
        Block::default().title(canvas_heading(app)).title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        canvas,
    );

    let hierarchy_failure = app.selected.and_then(|selected| {
        let node = app.graph.node(selected)?;
        node.incoming
            .failure()
            .or_else(|| node.outgoing.failure())
            .map(|failure| format!("Hierarchy error: {failure} (tl/tr retries)"))
    });
    let footer_text = match app.pending_key {
        Some('a') => "a_: c call search / t type search".to_owned(),
        Some('d') => "d_: d unpin anchor / p clear left / n clear right".to_owned(),
        Some('e') => "e_: c edit project config".to_owned(),
        Some('t') => "t_: l toggle left / r toggle right".to_owned(),
        Some('g') => "g_: <: show messages".to_owned(),
        _ => "?: help  ac/at: add  tl/tr: expand  hjkl: move  q: quit".to_owned(),
    };
    render_message_summary(
        frame,
        message_area,
        app.canvas_notice
            .as_deref()
            .map(|message| (message, app.canvas_notice_is_error()))
            .or_else(|| hierarchy_failure.as_deref().map(|message| (message, true))),
    );
    render_footer(frame, footer, footer_text, &app.analysis_status);

    if let Some(help_state) = &app.help {
        help::render(frame, help_state);
    } else if let Some(search) = &app.search {
        search::render(frame, search);
    } else if let Some(save_state) = &app.save {
        save::render(frame, save_state);
    }
    if let Some(view) = message_view.as_mut() {
        messages::render(frame, view, frame.area());
    }
}

fn canvas_message_and_footer(screen: Rect) -> [Rect; 3] {
    Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(screen)
}

fn canvas_inner_area(screen: Rect) -> Rect {
    let [canvas, _, _] = canvas_message_and_footer(screen);
    canvas
}

fn render_message_summary(frame: &mut Frame, area: Rect, message: Option<(&str, bool)>) {
    let Some((message, is_error)) = message else {
        return;
    };
    let (prefix, color) = if is_error {
        ("ERROR: ", Color::Red)
    } else {
        ("", Color::Yellow)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(prefix, Style::default().fg(color)),
            Span::styled(message, Style::default().fg(color)),
        ])),
        area,
    );
}

fn render_footer(frame: &mut Frame, area: Rect, shortcuts: String, status: &AnalysisStatus) {
    let mut content = vec![Span::styled(
        shortcuts,
        Style::default().fg(Color::DarkGray),
    )];
    content.push(Span::styled("  │  ", Style::default().fg(Color::DarkGray)));
    content.extend(analysis_status_line(status).spans);
    frame.render_widget(Paragraph::new(Line::from(content)), area);
}

fn canvas_heading(app: &App) -> String {
    app.selected
        .and_then(|selected| app.graph.node(selected))
        .and_then(|node| node.location.as_ref())
        .map(|location| location.uri.trim())
        .filter(|uri| !uri.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| "CALL GRAPH".to_owned())
}

fn analysis_status_line(status: &AnalysisStatus) -> Line<'static> {
    let (backend, backend_style) = match &status.backend {
        AnalysisBackend::Lsp(server) => {
            (format!("LSP: {server}"), Style::default().fg(Color::Cyan))
        }
        AnalysisBackend::TreeSitter(language) => (
            format!("Tree-sitter: {language}"),
            Style::default().fg(Color::Magenta),
        ),
        AnalysisBackend::None => (
            "Backend: none".to_owned(),
            Style::default().fg(Color::DarkGray),
        ),
    };
    let (phase, phase_style) = match status.phase {
        AnalysisPhase::Inactive => ("Inactive", Style::default().fg(Color::DarkGray)),
        AnalysisPhase::Ready => ("Ready", Style::default().fg(Color::Green)),
        AnalysisPhase::Working => ("Working", Style::default().fg(Color::Yellow)),
        AnalysisPhase::Warning => ("Warning", Style::default().fg(Color::Yellow)),
        AnalysisPhase::Error => ("Error", Style::default().fg(Color::Red)),
        AnalysisPhase::Disconnected => ("Disconnected", Style::default().fg(Color::Red)),
    };
    let percentage = status
        .percentage
        .map(|percentage| format!(" {percentage}%"))
        .unwrap_or_default();
    let mut content = vec![
        Span::styled(backend, backend_style),
        Span::styled(" · ", Style::default().fg(Color::DarkGray)),
        Span::styled(phase, phase_style),
        Span::raw(percentage),
    ];
    if let Some(message) = status
        .message
        .as_ref()
        .filter(|message| !message.is_empty())
    {
        content.push(Span::styled(" · ", Style::default().fg(Color::DarkGray)));
        content.push(Span::styled(
            message.clone(),
            Style::default().fg(Color::DarkGray),
        ));
    }
    Line::from(content)
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    use tower_lsp::lsp_types::{SymbolKind, Url};

    use super::messages;
    use super::{
        CanvasDragState, EdgeVisualKind, InteractionRequest, NavigationDirection,
        apply_ipc_command, apply_lsp_status, canvas_heading, canvas_inner_area, canvas_layout,
        handle_canvas_key, handle_canvas_mouse, move_canvas_selection, placement_bounds,
        rect_center, render, render_with_messages,
        search::{search_item, symbol_matches_search},
        send_open_location, with_stable_node_position, world_canvas_layout, world_rects_overlap,
    };
    use crate::{
        app::{AnalysisBackend, AnalysisPhase, App, SearchKind},
        cli::Cli,
        fetch::{
            CachePolicy, FetchSource, HierarchyResponse, WorkspaceSymbolMatch, lsp::LspStatusUpdate,
        },
        ipc::{
            IpcCommand,
            protocol::{Envelope, IpcRequest, IpcResponse},
        },
        state::{HierarchyDirection, HierarchyKind, LoadState, SourceLocation, SymbolIdentity},
    };

    #[test]
    fn filters_workspace_symbols_by_search_kind() {
        assert!(symbol_matches_search(
            SearchKind::Call,
            SymbolKind::FUNCTION
        ));
        assert!(symbol_matches_search(SearchKind::Type, SymbolKind::STRUCT));
        assert!(!symbol_matches_search(SearchKind::Call, SymbolKind::STRUCT));
        assert!(!symbol_matches_search(
            SearchKind::Type,
            SymbolKind::FUNCTION
        ));
        let method = search_item(WorkspaceSymbolMatch {
            name: "App::run".to_owned(),
            kind: SymbolKind::METHOD,
            container_name: Some("App".to_owned()),
            uri: Url::parse("file:///workspace/src/main.rs").unwrap(),
            range: None,
        });
        assert_eq!(method.name, "App::run");
    }

    #[test]
    fn ipc_focus_commands_mutate_app_and_return_matching_responses() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let request = IpcRequest::FocusSymbol {
            hierarchy: HierarchyKind::Call,
            symbol: "main".to_owned(),
            location: Some(SourceLocation {
                uri: "file:///workspace/src/main.rs".to_owned(),
                line: Some(4),
                character: Some(2),
            }),
        };
        let (command, mut responses) = IpcCommand::test_command(17, request);

        apply_ipc_command(&mut app, command);

        let selected = app.selected.unwrap();
        assert_eq!(app.graph.node(selected).unwrap().symbol, "main");
        assert!(app.graph.is_anchor(selected));
        let response: Envelope<IpcResponse> =
            serde_json::from_slice(&responses.try_recv().unwrap()).unwrap();
        assert_eq!(response.request_id, Some(17));
        assert_eq!(response.payload, IpcResponse::Accepted);

        let (command, mut responses) = IpcCommand::test_command(
            18,
            IpcRequest::FocusSymbol {
                hierarchy: HierarchyKind::Type,
                symbol: String::new(),
                location: None,
            },
        );
        apply_ipc_command(&mut app, command);
        let response: Envelope<IpcResponse> =
            serde_json::from_slice(&responses.try_recv().unwrap()).unwrap();
        assert_eq!(response.request_id, Some(18));
        let IpcResponse::Error { message } = response.payload else {
            panic!("invalid focus request must return an error");
        };
        assert!(message.contains("symbol must not be empty"));
    }

    #[test]
    fn maps_lsp_progress_without_losing_server_identity() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        app.set_analysis_status(crate::app::AnalysisStatus::lsp(
            "rust-analyzer",
            AnalysisPhase::Ready,
        ));

        apply_lsp_status(
            &mut app,
            LspStatusUpdate::Progress {
                title: "Roots Scanned".to_owned(),
                message: Some("68/251".to_owned()),
                percentage: Some(127),
            },
        );

        assert_eq!(
            app.analysis_status.backend,
            AnalysisBackend::Lsp("rust-analyzer".to_owned())
        );
        assert_eq!(app.analysis_status.phase, AnalysisPhase::Working);
        assert_eq!(
            app.analysis_status.message.as_deref(),
            Some("Roots Scanned: 68/251")
        );
        assert_eq!(app.analysis_status.percentage, Some(100));
    }

    #[test]
    fn footer_places_shortcuts_and_analysis_status_on_the_same_bottom_row() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        app.set_analysis_status(crate::app::AnalysisStatus {
            backend: AnalysisBackend::Lsp("rust-analyzer".to_owned()),
            phase: AnalysisPhase::Working,
            message: Some("Indexing".to_owned()),
            percentage: Some(68),
        });
        let width = 120;
        let height = 20;
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let bottom_row = (0..width).fold(String::new(), |mut row, x| {
            row.push_str(
                terminal
                    .backend()
                    .buffer()
                    .cell((x, height - 1))
                    .unwrap()
                    .symbol(),
            );
            row
        });

        let shortcuts = bottom_row.find("hjkl: move").unwrap();
        let status = bottom_row.find("LSP: rust-analyzer").unwrap();
        assert!(shortcuts < status);
        assert!(bottom_row.contains("?: help"));
        assert!(!bottom_row.contains("w: save"));
        assert!(!bottom_row.contains("dd/dp/dn"));
        assert!(bottom_row.contains("Working 68%"));
        assert_eq!(
            bottom_row
                .chars()
                .filter(|character| *character == '│')
                .count(),
            1
        );
        for y in 0..height - 1 {
            let row = (0..width).fold(String::new(), |mut row, x| {
                row.push_str(terminal.backend().buffer().cell((x, y)).unwrap().symbol());
                row
            });
            assert!(!row.contains("LSP: rust-analyzer"));
        }
    }

    #[test]
    fn messages_use_the_penultimate_row_without_replacing_footer_shortcuts() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        app.set_analysis_status(crate::app::AnalysisStatus {
            backend: AnalysisBackend::Lsp("rust-analyzer".to_owned()),
            phase: AnalysisPhase::Error,
            message: Some("server crashed".to_owned()),
            percentage: None,
        });
        let backend = TestBackend::new(100, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let message_row = (0..100).fold(String::new(), |mut row, x| {
            row.push_str(terminal.backend().buffer().cell((x, 10)).unwrap().symbol());
            row
        });
        let bottom_row = (0..100).fold(String::new(), |mut row, x| {
            row.push_str(terminal.backend().buffer().cell((x, 11)).unwrap().symbol());
            row
        });
        assert!(message_row.contains("ERROR: server crashed"));
        assert!(bottom_row.contains("hjkl: move"));
        assert!(bottom_row.contains("LSP: rust-analyzer"));
        assert!(!bottom_row.contains("server crashed"));

        let mut hierarchy_app =
            App::from_cli(Cli::try_parse_from(["cgraph", "call", "main"]).unwrap());
        let request = hierarchy_app
            .toggle_selected_branch(HierarchyDirection::Outgoing, true)
            .unwrap();
        assert!(hierarchy_app.finish_hierarchy(&request, Err("content modified".to_owned())));
        terminal
            .draw(|frame| render(frame, &hierarchy_app))
            .unwrap();
        let message_row = (0..100).fold(String::new(), |mut row, x| {
            row.push_str(terminal.backend().buffer().cell((x, 10)).unwrap().symbol());
            row
        });
        let bottom_row = (0..100).fold(String::new(), |mut row, x| {
            row.push_str(terminal.backend().buffer().cell((x, 11)).unwrap().symbol());
            row
        });
        assert!(message_row.contains("ERROR: Hierarchy query failed"));
        assert!(message_row.contains("content modified"));
        assert!(bottom_row.contains("hjkl: move"));
    }

    #[test]
    fn canvas_heading_uses_the_default_label_or_selected_node_uri() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        assert_eq!(canvas_heading(&app), "CALL GRAPH");

        let node = app.graph.pin_symbol(SymbolIdentity {
            symbol: "main".to_owned(),
            kind: HierarchyKind::Call,
            location: Some(SourceLocation {
                uri: "file:///workspace/src/main.rs".to_owned(),
                line: Some(4),
                character: Some(0),
            }),
        });
        app.selected = Some(node);
        assert_eq!(canvas_heading(&app), "file:///workspace/src/main.rs");
    }

    #[test]
    fn delete_prefix_requires_a_complete_valid_command() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "call", "root"]).unwrap());

        handle_canvas_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            false,
            Rect::new(0, 0, 100, 24),
        );
        assert_eq!(app.pending_key, Some('d'));
        handle_canvas_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            false,
            Rect::new(0, 0, 100, 24),
        );
        assert_eq!(app.graph.anchors().len(), 1);

        handle_canvas_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            false,
            Rect::new(0, 0, 100, 24),
        );
        handle_canvas_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            false,
            Rect::new(0, 0, 100, 24),
        );
        assert!(app.graph.anchors().is_empty());
        assert_eq!(app.selected, None);
    }

    #[test]
    fn ec_requires_the_complete_prefix_command() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let screen = Rect::new(0, 0, 100, 24);

        assert!(
            handle_canvas_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
                true,
                screen,
            )
            .is_none()
        );
        assert_eq!(app.pending_key, Some('e'));
        assert!(
            handle_canvas_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
                true,
                screen,
            )
            .is_none()
        );
        handle_canvas_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
            true,
            screen,
        );
        assert!(matches!(
            handle_canvas_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
                true,
                screen,
            ),
            Some(InteractionRequest::EditConfig)
        ));
        assert_eq!(app.pending_key, None);
    }

    #[test]
    fn g_less_than_opens_the_message_view_command() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let screen = Rect::new(0, 0, 100, 24);

        assert!(
            handle_canvas_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
                false,
                screen,
            )
            .is_none()
        );
        assert_eq!(app.pending_key, Some('g'));
        assert!(matches!(
            handle_canvas_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('<'), KeyModifiers::NONE),
                false,
                screen,
            ),
            Some(InteractionRequest::OpenMessages)
        ));
        assert_eq!(app.pending_key, None);

        app.set_canvas_notice("first message");
        app.set_canvas_notice("second message");
        let mut view = messages::MessageViewState::from_messages(&app.message_history);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| render_with_messages(frame, &app, Some(&mut view)))
            .unwrap();
        let content =
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .fold(String::new(), |mut output, cell| {
                    output.push_str(cell.symbol());
                    output
                });
        let bottom_row = (0..100).fold(String::new(), |mut row, column| {
            row.push_str(
                terminal
                    .backend()
                    .buffer()
                    .cell((column, 23))
                    .unwrap()
                    .symbol(),
            );
            row
        });
        assert!(content.contains("Messages"));
        assert!(content.contains("first message"));
        assert!(content.contains("second message"));
        assert!(bottom_row.contains("?: help"));
        assert!(bottom_row.contains("Backend: none"));
    }

    #[test]
    fn w_opens_an_empty_save_modal_without_quitting() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "call", "root"]).unwrap());

        let request = handle_canvas_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE),
            false,
            Rect::new(0, 0, 100, 24),
        );

        assert!(request.is_none());
        assert_eq!(app.save.as_ref().unwrap().input, "");
        assert!(!app.should_quit);
    }

    #[test]
    fn lays_out_multiple_anchors_with_the_selection_at_the_center() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        pin(&mut app, "first", HierarchyKind::Call);
        let selected = pin(&mut app, "selected", HierarchyKind::Type);
        pin(&mut app, "third", HierarchyKind::Call);
        app.selected = Some(selected);
        let layout = world_canvas_layout(&app.graph, app.selected);
        let selected = layout
            .nodes
            .iter()
            .find(|placement| placement.node_id == selected)
            .unwrap();

        assert_eq!(layout.nodes.len(), 3);
        assert_eq!(selected.slot.x + i32::from(selected.slot.width) / 2, 0);
        assert_eq!(selected.slot.y + i32::from(selected.slot.height) / 2, 0);
        assert!(
            layout
                .nodes
                .iter()
                .all(|placement| placement.slot.width > 0 && placement.slot.height > 0)
        );
    }

    #[test]
    fn selection_changes_only_translate_the_stable_world_layout() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let first = pin(&mut app, "first", HierarchyKind::Call);
        let second = pin(&mut app, "second", HierarchyKind::Call);
        let third = pin(&mut app, "third", HierarchyKind::Call);

        let first_selected = world_canvas_layout(&app.graph, Some(first));
        let second_selected = world_canvas_layout(&app.graph, Some(second));
        for (left, right) in [(first, second), (first, third), (second, third)] {
            let first_delta = world_delta(&first_selected, left, right);
            let second_delta = world_delta(&second_selected, left, right);
            assert_eq!(first_delta, second_delta);
        }
    }

    #[test]
    fn tl_and_tr_toggle_only_the_requested_side() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let root = pin(&mut app, "root", HierarchyKind::Call);
        connect(&mut app, root, HierarchyDirection::Incoming, &["caller"]);
        connect(&mut app, root, HierarchyDirection::Outgoing, &["callee"]);
        app.selected = Some(root);

        press(&mut app, 't');
        assert_eq!(app.pending_key, Some('t'));
        press(&mut app, 'l');
        assert!(app.graph.node(root).unwrap().incoming.expanded);
        assert!(!app.graph.node(root).unwrap().outgoing.expanded);

        press(&mut app, 't');
        press(&mut app, 'r');
        assert!(app.graph.node(root).unwrap().incoming.expanded);
        assert!(app.graph.node(root).unwrap().outgoing.expanded);

        press(&mut app, 't');
        press(&mut app, 'l');
        assert!(!app.graph.node(root).unwrap().incoming.expanded);
        assert!(app.graph.node(root).unwrap().outgoing.expanded);
    }

    #[test]
    fn first_tl_schedules_only_the_left_branch_query() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "call", "root"]).unwrap());
        let screen = Rect::new(0, 0, 100, 24);
        handle_canvas_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
            true,
            screen,
        );
        let request = handle_canvas_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE),
            true,
            screen,
        );

        let Some(InteractionRequest::Hierarchy(requests)) = request else {
            panic!("first tl must schedule a hierarchy request");
        };
        let [request] = requests.as_slice() else {
            panic!("first tl must schedule exactly one hierarchy request");
        };
        let root = app.selected.unwrap();
        assert_eq!(request.query.direction, HierarchyDirection::Incoming);
        assert_eq!(
            app.graph.node(root).unwrap().incoming.load_state,
            LoadState::Loading
        );
        assert_eq!(
            app.graph.node(root).unwrap().outgoing.load_state,
            LoadState::NotLoaded
        );
    }

    #[test]
    fn r_schedules_refreshes_for_both_branches() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "call", "root"]).unwrap());
        let request = handle_canvas_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
            true,
            Rect::new(0, 0, 100, 24),
        );

        let Some(InteractionRequest::Hierarchy(requests)) = request else {
            panic!("r must schedule hierarchy refresh requests");
        };
        assert_eq!(requests.len(), 2);
        assert!(
            requests
                .iter()
                .all(|request| request.cache_policy == CachePolicy::Refresh)
        );
        assert_eq!(
            requests
                .iter()
                .map(|request| request.query.direction)
                .collect::<Vec<_>>(),
            [HierarchyDirection::Incoming, HierarchyDirection::Outgoing]
        );
        let root = app.selected.unwrap();
        assert_eq!(
            app.graph.node(root).unwrap().incoming.load_state,
            LoadState::Loading
        );
        assert_eq!(
            app.graph.node(root).unwrap().outgoing.load_state,
            LoadState::Loading
        );
    }

    #[test]
    fn canvas_layout_only_contains_children_of_expanded_branches() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let root_id = pin(&mut app, "root", HierarchyKind::Call);
        let child_id = connect(&mut app, root_id, HierarchyDirection::Incoming, &["caller"])[0];
        let area = Rect::new(0, 0, 100, 20);

        let collapsed = canvas_layout(area, &app.graph, Some(root_id), Default::default());
        assert_eq!(collapsed.nodes.len(), 1);

        app.graph.node_mut(root_id).unwrap().incoming.expanded = true;
        let expanded = canvas_layout(area, &app.graph, Some(root_id), Default::default());
        assert_eq!(expanded.nodes.len(), 2);
        assert!(
            expanded
                .nodes
                .iter()
                .any(|placement| placement.node_id == child_id)
        );
    }

    #[test]
    fn expanded_node_rectangles_never_overlap() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let root = pin(&mut app, "root", HierarchyKind::Call);
        let mut callers = Vec::new();
        let mut callees = Vec::new();
        for index in 0..8 {
            callers.push(if index % 2 == 0 {
                format!("VeryLongCallerClass{index}::call_root")
            } else {
                format!("caller-{index}")
            });
            callees.push(format!("callee-{index}"));
        }
        connect_owned(&mut app, root, HierarchyDirection::Incoming, &callers);
        connect_owned(&mut app, root, HierarchyDirection::Outgoing, &callees);
        app.graph.node_mut(root).unwrap().incoming.expanded = true;
        app.graph.node_mut(root).unwrap().outgoing.expanded = true;
        let layout = world_canvas_layout(&app.graph, None);

        assert!(
            layout.nodes.len() > 3,
            "test needs several visible hierarchy nodes"
        );
        for (index, placement) in layout.nodes.iter().enumerate() {
            for other in layout.nodes.iter().skip(index + 1) {
                assert!(
                    !world_rects_overlap(placement.slot, other.slot),
                    "placements overlap: {placement:?} and {other:?}"
                );
            }
        }
    }

    #[test]
    fn visible_parent_child_relationship_renders_a_connector() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let root_id = pin(&mut app, "root", HierarchyKind::Call);
        let child_id = connect(&mut app, root_id, HierarchyDirection::Outgoing, &["child"])[0];
        app.graph.node_mut(root_id).unwrap().outgoing.expanded = true;
        app.selected = Some(root_id);
        let screen = Rect::new(0, 0, 100, 20);
        let layout = canvas_layout(
            canvas_inner_area(screen),
            &app.graph,
            app.selected,
            app.viewport,
        );

        assert_eq!(layout.edges.len(), 1);
        assert_eq!(layout.edges[0].source_id, root_id);
        assert_eq!(layout.edges[0].target_id, child_id);
        assert!(
            layout.edges[0].cells.iter().any(|cell| cell.symbol == '▶'),
            "forward connections must show their direction before the target"
        );
        let connector = layout.edges[0]
            .cells
            .iter()
            .find(|cell| {
                layout.nodes.iter().all(|placement| {
                    !placement_bounds(*placement).contains((cell.x, cell.y).into())
                })
            })
            .expect("connection must occupy at least one cell between node boxes");

        let backend = TestBackend::new(screen.width, screen.height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        assert_ne!(
            terminal
                .backend()
                .buffer()
                .cell((connector.x, connector.y))
                .unwrap()
                .symbol(),
            " "
        );
    }

    #[test]
    fn node_box_does_not_render_call_or_type_corner_labels() {
        let app = App::from_cli(
            Cli::try_parse_from(["cgraph", "call", "VeryLongClassName::very_long_method_name"])
                .unwrap(),
        );
        let screen = Rect::new(0, 0, 80, 16);
        let layout = canvas_layout(
            canvas_inner_area(screen),
            &app.graph,
            app.selected,
            app.viewport,
        );
        let placement = layout.nodes[0];
        let node = app.graph.node(placement.node_id).unwrap();
        assert!(usize::from(placement.area.width.saturating_sub(2)) >= node.symbol.chars().count());
        let backend = TestBackend::new(screen.width, screen.height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let top_border = (placement.area.x..placement.area.right())
            .map(|x| {
                terminal
                    .backend()
                    .buffer()
                    .cell((x, placement.area.y))
                    .unwrap()
                    .symbol()
            })
            .collect::<String>();

        assert!(!top_border.contains("call"));
        assert!(!top_border.contains("type"));
    }

    #[test]
    fn partially_visible_node_renders_a_true_slice_until_fully_offscreen() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "call", "root"]).unwrap());
        let screen = Rect::new(0, 0, 40, 12);
        let canvas = canvas_inner_area(screen);
        let initial = canvas_layout(canvas, &app.graph, app.selected, app.viewport);
        let initial_slot = initial.nodes[0].slot;
        let desired_x = i64::from(canvas.x).saturating_sub(5);
        app.viewport.offset_x = i32::try_from(desired_x.saturating_sub(initial_slot.x)).unwrap();

        let clipped = canvas_layout(canvas, &app.graph, app.selected, app.viewport);
        let placement = clipped.nodes[0];
        assert!(placement.visible_slot.width < placement.slot.width);
        assert_eq!(placement.visible_slot.x, canvas.x);

        let backend = TestBackend::new(screen.width, screen.height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        assert_eq!(
            terminal
                .backend()
                .buffer()
                .cell((canvas.x, placement.area.y))
                .unwrap()
                .symbol(),
            "─",
            "the viewport boundary must not be rendered as a synthetic left border"
        );

        app.viewport.offset_x = app.viewport.offset_x.saturating_sub(1000);
        assert!(
            canvas_layout(canvas, &app.graph, app.selected, app.viewport)
                .nodes
                .is_empty(),
            "a node should disappear only after it no longer intersects the viewport"
        );
    }

    #[test]
    fn connection_remains_visible_when_its_target_box_is_offscreen() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let root_id = pin(&mut app, "root", HierarchyKind::Call);
        let child_id = connect(&mut app, root_id, HierarchyDirection::Outgoing, &["child"])[0];
        app.graph.node_mut(root_id).unwrap().outgoing.expanded = true;
        app.selected = Some(root_id);
        let screen = Rect::new(0, 0, 36, 12);
        let canvas = canvas_inner_area(screen);
        let layout = canvas_layout(canvas, &app.graph, app.selected, app.viewport);

        assert!(
            layout
                .nodes
                .iter()
                .any(|placement| placement.node_id == root_id)
        );
        assert!(
            !layout
                .nodes
                .iter()
                .any(|placement| placement.node_id == child_id),
            "the fixture requires the target box to be completely offscreen"
        );
        let edge = layout
            .edges
            .iter()
            .find(|edge| edge.source_id == root_id && edge.target_id == child_id)
            .expect("an edge crossing the viewport must survive endpoint clipping");
        let continuation = edge
            .cells
            .iter()
            .find(|cell| cell.symbol == '▶')
            .expect("the offscreen target direction remains visible at the boundary");
        assert_eq!(continuation.x, canvas.right() - 1);

        let backend = TestBackend::new(screen.width, screen.height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        assert_eq!(
            terminal
                .backend()
                .buffer()
                .cell((continuation.x, continuation.y))
                .unwrap()
                .symbol(),
            "▶"
        );
    }

    #[test]
    fn canvas_mouse_selects_nodes_and_toggles_side_buttons_independently() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let first_id = pin(&mut app, "first", HierarchyKind::Call);
        let second_id = pin(&mut app, "second", HierarchyKind::Call);
        connect(
            &mut app,
            second_id,
            HierarchyDirection::Incoming,
            &["caller"],
        );
        connect(
            &mut app,
            second_id,
            HierarchyDirection::Outgoing,
            &["callee"],
        );
        app.selected = Some(first_id);
        let screen = Rect::new(0, 0, 100, 24);
        let layout = canvas_layout(
            canvas_inner_area(screen),
            &app.graph,
            app.selected,
            app.viewport,
        );
        let second_placement = *layout
            .nodes
            .iter()
            .find(|placement| placement.node_id == second_id)
            .unwrap();

        click(
            &mut app,
            screen,
            second_placement.area.x + 1,
            second_placement.area.y + 1,
        );
        assert_eq!(app.selected, Some(second_id));

        let after_selection = canvas_layout(
            canvas_inner_area(screen),
            &app.graph,
            app.selected,
            app.viewport,
        );
        let second_after_selection = *after_selection
            .nodes
            .iter()
            .find(|placement| placement.node_id == second_id)
            .unwrap();
        assert_eq!(second_after_selection.slot, second_placement.slot);

        let first_placement = *after_selection
            .nodes
            .iter()
            .find(|placement| placement.node_id == first_id)
            .unwrap();
        click(
            &mut app,
            screen,
            first_placement.area.x + 1,
            first_placement.area.y + 1,
        );
        assert_eq!(app.selected, Some(first_id));
        let before_incoming_toggle = canvas_layout(
            canvas_inner_area(screen),
            &app.graph,
            app.selected,
            app.viewport,
        );
        let second_placement = *before_incoming_toggle
            .nodes
            .iter()
            .find(|placement| placement.node_id == second_id)
            .unwrap();
        click(
            &mut app,
            screen,
            second_placement.incoming_button.x + 1,
            second_placement.incoming_button.y,
        );
        assert_eq!(app.selected, Some(second_id));
        assert!(app.graph.node(second_id).unwrap().incoming.expanded);
        assert!(!app.graph.node(second_id).unwrap().outgoing.expanded);
        let after_incoming_toggle = canvas_layout(
            canvas_inner_area(screen),
            &app.graph,
            app.selected,
            app.viewport,
        );
        let second_after_incoming_toggle = *after_incoming_toggle
            .nodes
            .iter()
            .find(|placement| placement.node_id == second_id)
            .unwrap();
        assert_eq!(second_after_incoming_toggle.slot, second_placement.slot);

        click(
            &mut app,
            screen,
            second_after_incoming_toggle.outgoing_button.x + 1,
            second_after_incoming_toggle.outgoing_button.y,
        );
        assert!(app.graph.node(second_id).unwrap().incoming.expanded);
        assert!(app.graph.node(second_id).unwrap().outgoing.expanded);
        let after_outgoing_toggle = canvas_layout(
            canvas_inner_area(screen),
            &app.graph,
            app.selected,
            app.viewport,
        );
        assert_eq!(
            after_outgoing_toggle
                .nodes
                .iter()
                .find(|placement| placement.node_id == second_id)
                .unwrap()
                .slot,
            second_after_incoming_toggle.slot
        );
    }

    #[test]
    fn first_mouse_side_button_schedules_its_branch_query() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "type", "Root"]).unwrap());
        let screen = Rect::new(0, 0, 100, 24);
        let placement = canvas_layout(
            canvas_inner_area(screen),
            &app.graph,
            app.selected,
            app.viewport,
        )
        .nodes[0];
        let mut drag = CanvasDragState::default();
        let request = handle_canvas_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: placement.outgoing_button.x + 1,
                row: placement.outgoing_button.y,
                modifiers: KeyModifiers::NONE,
            },
            true,
            screen,
            &mut drag,
        );

        let Some(InteractionRequest::Hierarchy(requests)) = request else {
            panic!("first side-button click must schedule hierarchy loading");
        };
        let [request] = requests.as_slice() else {
            panic!("first side-button click must schedule exactly one hierarchy request");
        };
        let root = app.selected.unwrap();
        assert_eq!(request.query.direction, HierarchyDirection::Outgoing);
        assert_eq!(
            app.graph.node(root).unwrap().outgoing.load_state,
            LoadState::Loading
        );
        assert_eq!(
            app.graph.node(root).unwrap().incoming.load_state,
            LoadState::NotLoaded
        );
        assert_eq!(drag.previous, None);
    }

    #[test]
    fn double_clicking_a_node_opens_only_an_exact_source_location() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let node_id = pin(&mut app, "main", HierarchyKind::Call);
        app.selected = Some(node_id);
        let expected = app.graph.node(node_id).unwrap().location.clone().unwrap();
        let screen = Rect::new(0, 0, 100, 24);
        let placement = canvas_layout(
            canvas_inner_area(screen),
            &app.graph,
            app.selected,
            app.viewport,
        )
        .nodes[0];
        let column = placement.area.x + 1;
        let row = placement.area.y + 1;
        let mut pointer = CanvasDragState::default();

        assert!(complete_click(&mut app, screen, &mut pointer, column, row).is_none());
        let second = complete_click(&mut app, screen, &mut pointer, column, row);
        let Some(InteractionRequest::OpenLocation(location)) = second else {
            panic!("double-click must emit the node's exact source location");
        };
        assert_eq!(location, expected);
        send_open_location(&mut app, None, &expected);
        assert_eq!(
            app.canvas_notice.as_deref(),
            Some("IPC is not enabled; start cgraph with --ipc-socket <PATH>")
        );

        let mut provisional =
            App::from_cli(Cli::try_parse_from(["cgraph", "call", "unresolved"]).unwrap());
        let placement = canvas_layout(
            canvas_inner_area(screen),
            &provisional.graph,
            provisional.selected,
            provisional.viewport,
        )
        .nodes[0];
        let mut pointer = CanvasDragState::default();
        let column = placement.area.x + 1;
        let row = placement.area.y + 1;
        assert!(complete_click(&mut provisional, screen, &mut pointer, column, row).is_none());
        assert!(complete_click(&mut provisional, screen, &mut pointer, column, row).is_none());
        assert_eq!(
            provisional.canvas_notice.as_deref(),
            Some("Selected node has no exact source location")
        );
    }

    #[test]
    fn hierarchy_completion_keeps_the_queried_node_center_stable() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "call", "temporary"]).unwrap());
        let screen = Rect::new(0, 0, 100, 24);
        let request = app
            .toggle_selected_branch(HierarchyDirection::Outgoing, true)
            .unwrap();
        let before = canvas_layout(
            canvas_inner_area(screen),
            &app.graph,
            app.selected,
            app.viewport,
        )
        .nodes[0]
            .slot;
        let mut resolved_query = request.query.clone();
        resolved_query.symbol = identity(
            "VeryLongResolvedType::very_long_resolved_method",
            HierarchyKind::Call,
        );

        assert!(with_stable_node_position(
            &mut app,
            request.node_id,
            |app| app.finish_hierarchy(
                &request,
                Ok(HierarchyResponse {
                    query: resolved_query,
                    children: Vec::new(),
                    source: FetchSource::Lsp,
                }),
            )
        ));

        let after = canvas_layout(
            canvas_inner_area(screen),
            &app.graph,
            app.selected,
            app.viewport,
        )
        .nodes[0]
            .slot;
        assert_eq!(projected_center(after), projected_center(before));
        assert!(after.width > before.width);
    }

    #[test]
    fn dragging_canvas_or_node_pans_viewport_and_reveals_offscreen_nodes() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let root_id = pin(&mut app, "root", HierarchyKind::Call);
        let child_id = connect(&mut app, root_id, HierarchyDirection::Outgoing, &["child"])[0];
        app.graph.node_mut(root_id).unwrap().outgoing.expanded = true;
        app.selected = Some(root_id);
        let screen = Rect::new(0, 0, 60, 16);
        let canvas = canvas_inner_area(screen);
        let world_before = world_canvas_layout(&app.graph, app.selected);
        let initial = canvas_layout(canvas, &app.graph, app.selected, app.viewport);
        assert!(
            initial
                .nodes
                .iter()
                .any(|placement| placement.node_id == root_id)
        );
        let initial_child = initial
            .nodes
            .iter()
            .find(|placement| placement.node_id == child_id)
            .expect("the intersecting part of the child must remain visible");
        assert!(initial_child.visible_slot.width < initial_child.slot.width);

        let root_placement = initial
            .nodes
            .iter()
            .find(|placement| placement.node_id == root_id)
            .copied()
            .unwrap();
        let mut node_drag = CanvasDragState::default();
        let node_start = root_placement.area.right() - 2;
        handle_canvas_mouse(
            &mut app,
            mouse_event(
                MouseEventKind::Down(MouseButton::Left),
                node_start,
                root_placement.area.y + 1,
            ),
            false,
            screen,
            &mut node_drag,
        );
        handle_canvas_mouse(
            &mut app,
            mouse_event(
                MouseEventKind::Drag(MouseButton::Left),
                node_start - 20,
                root_placement.area.y + 1,
            ),
            false,
            screen,
            &mut node_drag,
        );

        assert_eq!(app.viewport.offset_x, -20);
        assert_eq!(world_canvas_layout(&app.graph, app.selected), world_before);
        let after_node_drag = canvas_layout(canvas, &app.graph, app.selected, app.viewport);
        let child_after_node_drag = after_node_drag
            .nodes
            .iter()
            .find(|placement| placement.node_id == child_id)
            .unwrap();
        assert_eq!(
            child_after_node_drag.visible_slot.width,
            child_after_node_drag.slot.width
        );

        app.viewport = Default::default();
        let mut background_drag = CanvasDragState::default();
        let background_start = canvas.right() - 2;
        handle_canvas_mouse(
            &mut app,
            mouse_event(
                MouseEventKind::Down(MouseButton::Left),
                background_start,
                canvas.y,
            ),
            false,
            screen,
            &mut background_drag,
        );
        handle_canvas_mouse(
            &mut app,
            mouse_event(
                MouseEventKind::Drag(MouseButton::Left),
                background_start - 20,
                canvas.y,
            ),
            false,
            screen,
            &mut background_drag,
        );

        assert_eq!(app.viewport.offset_x, -20);
        assert_eq!(world_canvas_layout(&app.graph, app.selected), world_before);
        let after_background_drag = canvas_layout(canvas, &app.graph, app.selected, app.viewport);
        let child_after_background_drag = after_background_drag
            .nodes
            .iter()
            .find(|placement| placement.node_id == child_id)
            .unwrap();
        assert_eq!(
            child_after_background_drag.visible_slot.width,
            child_after_background_drag.slot.width
        );
        assert_eq!(
            app.graph.node(root_id).unwrap().outgoing.neighbors[0],
            child_id
        );
    }

    #[test]
    fn keyboard_navigation_uses_visible_node_geometry() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let root_id = pin(&mut app, "root", HierarchyKind::Call);
        let incoming_id = connect(&mut app, root_id, HierarchyDirection::Incoming, &["caller"])[0];
        let outgoing_id = connect(&mut app, root_id, HierarchyDirection::Outgoing, &["callee"])[0];
        app.graph.node_mut(root_id).unwrap().incoming.expanded = true;
        app.graph.node_mut(root_id).unwrap().outgoing.expanded = true;
        app.selected = Some(root_id);
        let screen = Rect::new(0, 0, 100, 24);

        let before_navigation = canvas_layout(
            canvas_inner_area(screen),
            &app.graph,
            app.selected,
            app.viewport,
        );
        let outgoing_before_selection = before_navigation
            .nodes
            .iter()
            .find(|placement| placement.node_id == outgoing_id)
            .unwrap()
            .slot;
        navigate(&mut app, screen, KeyCode::Right);
        assert_eq!(app.selected, Some(outgoing_id));
        let outgoing_after_selection = canvas_layout(
            canvas_inner_area(screen),
            &app.graph,
            app.selected,
            app.viewport,
        )
        .nodes
        .into_iter()
        .find(|placement| placement.node_id == outgoing_id)
        .unwrap()
        .slot;
        assert_eq!(outgoing_after_selection, outgoing_before_selection);
        navigate(&mut app, screen, KeyCode::Char('h'));
        assert_eq!(app.selected, Some(root_id));
        navigate(&mut app, screen, KeyCode::Left);
        assert_eq!(app.selected, Some(incoming_id));
        navigate(&mut app, screen, KeyCode::Char('l'));
        assert_eq!(app.selected, Some(root_id));

        pin(&mut app, "other-before", HierarchyKind::Call);
        for index in 0..3 {
            pin(&mut app, &format!("other-{index}"), HierarchyKind::Call);
        }
        let before = canvas_layout(
            canvas_inner_area(screen),
            &app.graph,
            app.selected,
            app.viewport,
        );
        let before_center = rect_center(
            before
                .nodes
                .iter()
                .find(|placement| placement.node_id == root_id)
                .unwrap()
                .area,
        );
        assert!(move_canvas_selection(
            &mut app,
            NavigationDirection::Up,
            screen
        ));
        let selected = app.selected.unwrap();
        let after = canvas_layout(
            canvas_inner_area(screen),
            &app.graph,
            app.selected,
            app.viewport,
        );
        let selected_center = rect_center(
            after
                .nodes
                .iter()
                .find(|placement| placement.node_id == selected)
                .unwrap()
                .area,
        );
        assert!(selected_center.1 <= before_center.1);
        assert!(move_canvas_selection(
            &mut app,
            NavigationDirection::Down,
            screen
        ));
    }

    #[test]
    fn diamond_layout_uses_one_shared_node_and_keeps_all_edges() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let root = pin(&mut app, "root", HierarchyKind::Call);
        let branches = connect(
            &mut app,
            root,
            HierarchyDirection::Outgoing,
            &["left", "right"],
        );
        app.graph.node_mut(root).unwrap().outgoing.expanded = true;
        let shared_from_left = connect(
            &mut app,
            branches[0],
            HierarchyDirection::Outgoing,
            &["shared"],
        )[0];
        let shared_from_right = connect(
            &mut app,
            branches[1],
            HierarchyDirection::Outgoing,
            &["shared"],
        )[0];
        app.graph.node_mut(branches[0]).unwrap().outgoing.expanded = true;
        app.graph.node_mut(branches[1]).unwrap().outgoing.expanded = true;

        let layout = world_canvas_layout(&app.graph, Some(root));
        assert_eq!(shared_from_left, shared_from_right);
        assert_eq!(layout.nodes.len(), 4);
        assert_eq!(layout.edges.len(), 4);
        assert_eq!(
            layout
                .nodes
                .iter()
                .filter(|placement| placement.node_id == shared_from_left)
                .count(),
            1
        );
    }

    #[test]
    fn cycle_edges_use_the_special_double_line_style() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let first = pin(&mut app, "first", HierarchyKind::Call);
        let second = connect(&mut app, first, HierarchyDirection::Outgoing, &["second"])[0];
        let third = connect(&mut app, second, HierarchyDirection::Outgoing, &["third"])[0];
        connect(&mut app, third, HierarchyDirection::Outgoing, &["first"]);
        for node_id in [first, second, third] {
            app.graph.node_mut(node_id).unwrap().outgoing.expanded = true;
        }

        let world = world_canvas_layout(&app.graph, Some(first));
        assert_eq!(world.nodes.len(), 3);
        assert!(
            world
                .edges
                .iter()
                .all(|edge| edge.visual_kind == EdgeVisualKind::BackOrCycle)
        );
        let canvas = canvas_layout(
            Rect::new(0, 0, 120, 30),
            &app.graph,
            Some(first),
            Default::default(),
        );
        assert!(
            canvas
                .edges
                .iter()
                .flat_map(|edge| &edge.cells)
                .any(|cell| { matches!(cell.symbol, '═' | '║' | '╬' | '◀') })
        );
    }

    #[test]
    fn self_loop_is_rendered_as_a_special_loop() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let recursive = pin(&mut app, "recursive", HierarchyKind::Call);
        connect(
            &mut app,
            recursive,
            HierarchyDirection::Outgoing,
            &["recursive"],
        );
        app.graph.node_mut(recursive).unwrap().outgoing.expanded = true;

        let layout = canvas_layout(
            Rect::new(0, 0, 100, 24),
            &app.graph,
            Some(recursive),
            Default::default(),
        );
        assert_eq!(layout.edges.len(), 1);
        assert_eq!(layout.edges[0].visual_kind, EdgeVisualKind::SelfLoop);
        assert!(layout.edges[0].cells.iter().any(|cell| cell.symbol == '↺'));
    }

    fn press(app: &mut App, character: char) {
        handle_canvas_key(
            app,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            false,
            Rect::new(0, 0, 100, 24),
        );
    }

    fn navigate(app: &mut App, screen: Rect, key_code: KeyCode) {
        handle_canvas_key(
            app,
            KeyEvent::new(key_code, KeyModifiers::NONE),
            false,
            screen,
        );
    }

    fn click(app: &mut App, screen: Rect, column: u16, row: u16) {
        let mut drag = CanvasDragState::default();
        handle_canvas_mouse(
            app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            },
            false,
            screen,
            &mut drag,
        );
    }

    fn complete_click(
        app: &mut App,
        screen: Rect,
        pointer: &mut CanvasDragState,
        column: u16,
        row: u16,
    ) -> Option<InteractionRequest> {
        handle_canvas_mouse(
            app,
            mouse_event(MouseEventKind::Down(MouseButton::Left), column, row),
            false,
            screen,
            pointer,
        );
        handle_canvas_mouse(
            app,
            mouse_event(MouseEventKind::Up(MouseButton::Left), column, row),
            false,
            screen,
            pointer,
        )
    }

    fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn pin(app: &mut App, symbol: &str, kind: HierarchyKind) -> crate::state::NodeId {
        app.graph.pin_symbol(identity(symbol, kind))
    }

    fn connect(
        app: &mut App,
        node_id: crate::state::NodeId,
        direction: HierarchyDirection,
        children: &[&str],
    ) -> Vec<crate::state::NodeId> {
        let kind = app.graph.node(node_id).unwrap().kind;
        app.graph
            .replace_branch_neighbors(
                node_id,
                direction,
                children
                    .iter()
                    .map(|symbol| identity(symbol, kind))
                    .collect(),
            )
            .unwrap()
    }

    fn connect_owned(
        app: &mut App,
        node_id: crate::state::NodeId,
        direction: HierarchyDirection,
        children: &[String],
    ) -> Vec<crate::state::NodeId> {
        let names = children.iter().map(String::as_str).collect::<Vec<_>>();
        connect(app, node_id, direction, &names)
    }

    fn identity(symbol: &str, kind: HierarchyKind) -> SymbolIdentity {
        SymbolIdentity {
            symbol: symbol.to_owned(),
            kind,
            location: Some(SourceLocation {
                uri: "file:///workspace/src/main.rs".to_owned(),
                line: Some(symbol.bytes().map(u32::from).sum()),
                character: Some(0),
            }),
        }
    }

    fn world_delta(
        layout: &crate::tui::canvas::WorldLayoutSnapshot,
        from: crate::state::NodeId,
        to: crate::state::NodeId,
    ) -> (i32, i32) {
        let from = layout
            .nodes
            .iter()
            .find(|placement| placement.node_id == from)
            .unwrap()
            .slot;
        let to = layout
            .nodes
            .iter()
            .find(|placement| placement.node_id == to)
            .unwrap()
            .slot;
        (to.x - from.x, to.y - from.y)
    }

    fn projected_center(slot: crate::tui::canvas::ProjectedRect) -> (i64, i64) {
        (
            slot.x + i64::from(slot.width) / 2,
            slot.y + i64::from(slot.height) / 2,
        )
    }
}
