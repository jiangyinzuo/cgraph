#![doc = include_str!("README.md")]

use std::{
    io::{self, Stdout},
    sync::mpsc::{self, Sender},
    time::Duration,
};

use anyhow::Result;
use crossterm::{
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
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::{task::JoinHandle, time::sleep};

use crate::{
    app::{
        AnalysisBackend, AnalysisPhase, AnalysisStatus, App, HierarchyLoadRequest, SearchItem,
        SearchKind, SearchRequest, SearchState, SearchStatus,
    },
    fetch::lsp::{HierarchyClient, LspStatusUpdate, WorkspaceSymbolClient, WorkspaceSymbolMatch},
    state::{
        HierarchyDirection, LoadState, NodeId, SourceLocation,
        graph::{GraphBranch, GraphNode},
    },
};
use tower_lsp::lsp_types::SymbolKind;

mod canvas;

use canvas::{CanvasConnections, CanvasNodePlacement, canvas_layout};
#[cfg(test)]
use canvas::{EdgeVisualKind, placement_bounds, world_canvas_layout, world_rects_overlap};

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

enum SearchQueryEvent {
    Started(u64),
    Finished {
        request_id: u64,
        result: Result<Vec<SearchItem>, String>,
    },
}

enum InteractionRequest {
    Search(SearchRequest),
    Hierarchy(HierarchyLoadRequest),
}

struct HierarchyQueryEvent {
    request: HierarchyLoadRequest,
    result: Result<crate::fetch::HierarchyResponse, String>,
}

const WORKSPACE_SYMBOL_SEARCH_DELAY: Duration = Duration::from_millis(200);

#[derive(Default)]
struct CanvasDragState {
    previous: Option<(u16, u16)>,
}

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

pub fn run(
    terminal: &mut Tui,
    app: &mut App,
    symbol_client: Option<WorkspaceSymbolClient>,
    hierarchy_client: Option<HierarchyClient>,
    mut lsp_status_receiver: Option<UnboundedReceiver<LspStatusUpdate>>,
) -> Result<()> {
    // Crossterm is currently polled synchronously, while LSP work runs on the
    // Tokio runtime. A small channel keeps async completion out of App and lets
    // the loop continue rendering loading state and receiving input.
    let (query_sender, query_receiver) = mpsc::channel::<SearchQueryEvent>();
    let (hierarchy_sender, hierarchy_receiver) = mpsc::channel::<HierarchyQueryEvent>();
    let mut search_task: Option<JoinHandle<()>> = None;
    let mut hierarchy_tasks = Vec::<JoinHandle<()>>::new();
    let mut canvas_drag = CanvasDragState::default();

    while !app.should_quit {
        if let Some(receiver) = lsp_status_receiver.as_mut() {
            while let Ok(status) = receiver.try_recv() {
                apply_lsp_status(app, status);
            }
        }
        while let Ok(query_event) = query_receiver.try_recv() {
            match query_event {
                SearchQueryEvent::Started(request_id) => app.start_search(request_id),
                SearchQueryEvent::Finished { request_id, result } => {
                    app.finish_search(request_id, result);
                }
            }
        }
        while let Ok(event) = hierarchy_receiver.try_recv() {
            app.finish_hierarchy(&event.request, event.result);
        }
        hierarchy_tasks.retain(|task| !task.is_finished());
        terminal.draw(|frame| render(frame, app))?;

        if event::poll(Duration::from_millis(50))? {
            let (width, height) = terminal_size()?;
            let screen = Rect::new(0, 0, width, height);
            let request = handle_event(
                app,
                event::read()?,
                hierarchy_client.is_some(),
                screen,
                &mut canvas_drag,
            );
            if app.search.is_none()
                && let Some(task) = search_task.take()
            {
                task.abort();
            }
            match request {
                Some(InteractionRequest::Search(request)) => {
                    if let Some(client) = symbol_client.clone() {
                        let next_task = schedule_search(client, request, query_sender.clone());
                        if let Some(previous_task) = search_task.replace(next_task) {
                            previous_task.abort();
                        }
                    }
                }
                Some(InteractionRequest::Hierarchy(request)) => {
                    if let Some(client) = hierarchy_client.clone() {
                        hierarchy_tasks.push(schedule_hierarchy(
                            client,
                            request,
                            hierarchy_sender.clone(),
                        ));
                    }
                }
                None => {}
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

fn schedule_search(
    client: WorkspaceSymbolClient,
    request: SearchRequest,
    sender: Sender<SearchQueryEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        sleep(WORKSPACE_SYMBOL_SEARCH_DELAY).await;
        if sender
            .send(SearchQueryEvent::Started(request.request_id))
            .is_err()
        {
            return;
        }
        let result = client
            .query(&request.query)
            .await
            .map(|symbols| {
                symbols
                    .into_iter()
                    .filter(|symbol| symbol_matches_search(request.kind, symbol.kind))
                    .map(search_item)
                    .collect()
            })
            .map_err(|error| format!("{error:#}"));
        let _ = sender.send(SearchQueryEvent::Finished {
            request_id: request.request_id,
            result,
        });
    })
}

fn search_item(symbol: WorkspaceSymbolMatch) -> SearchItem {
    let name = symbol.display_name();
    let uri = symbol.uri.to_string();
    let path = symbol
        .uri
        .to_file_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|()| symbol.uri.to_string());
    let (location, line, character) = symbol.range.map_or_else(
        || (path.clone(), None, None),
        |range| {
            (
                format!("{path}:{}", range.start.line + 1),
                Some(range.start.line),
                Some(range.start.character),
            )
        },
    );

    SearchItem {
        name,
        container_name: symbol.container_name,
        location,
        source: Some(SourceLocation {
            uri,
            line,
            character,
        }),
    }
}

fn symbol_matches_search(search_kind: SearchKind, symbol_kind: SymbolKind) -> bool {
    match search_kind {
        SearchKind::Call => matches!(
            symbol_kind,
            SymbolKind::FUNCTION | SymbolKind::METHOD | SymbolKind::CONSTRUCTOR
        ),
        SearchKind::Type => matches!(
            symbol_kind,
            SymbolKind::CLASS
                | SymbolKind::INTERFACE
                | SymbolKind::STRUCT
                | SymbolKind::ENUM
                | SymbolKind::TYPE_PARAMETER
        ),
    }
}

fn handle_event(
    app: &mut App,
    event: Event,
    lsp_available: bool,
    screen: Rect,
    canvas_drag: &mut CanvasDragState,
) -> Option<InteractionRequest> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            let request = if app.search.is_some() {
                handle_search_key(app, key).map(InteractionRequest::Search)
            } else {
                handle_canvas_key(app, key, lsp_available, screen)
            };
            if app.search.is_some() {
                canvas_drag.previous = None;
            }
            request
        }
        Event::Mouse(mouse) => {
            if app.search.is_some() {
                canvas_drag.previous = None;
                handle_search_mouse(app, mouse, screen);
                None
            } else {
                handle_canvas_mouse(app, mouse, lsp_available, screen, canvas_drag)
            }
        }
        _ => None,
    }
}

fn handle_canvas_key(
    app: &mut App,
    key: KeyEvent,
    lsp_available: bool,
    screen: Rect,
) -> Option<InteractionRequest> {
    if let Some(prefix) = app.pending_key.take() {
        return match prefix {
            'a' => match key.code {
                KeyCode::Char('c') if key.modifiers == KeyModifiers::NONE => app
                    .open_search(SearchKind::Call, lsp_available)
                    .map(InteractionRequest::Search),
                KeyCode::Char('t') if key.modifiers == KeyModifiers::NONE => app
                    .open_search(SearchKind::Type, lsp_available)
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
            't' => {
                match key.code {
                    KeyCode::Char('l') if key.modifiers == KeyModifiers::NONE => {
                        return app
                            .toggle_selected_branch(HierarchyDirection::Incoming, lsp_available)
                            .map(InteractionRequest::Hierarchy);
                    }
                    KeyCode::Char('r') if key.modifiers == KeyModifiers::NONE => {
                        return app
                            .toggle_selected_branch(HierarchyDirection::Outgoing, lsp_available)
                            .map(InteractionRequest::Hierarchy);
                    }
                    _ => {}
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
        KeyCode::Char('t') if key.modifiers == KeyModifiers::NONE => {
            app.pending_key = Some('t');
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
        KeyCode::Char('q') | KeyCode::Esc => app.quit(),
        _ => {}
    }

    None
}

fn handle_search_key(app: &mut App, key: KeyEvent) -> Option<SearchRequest> {
    match key.code {
        KeyCode::Esc => {
            app.close_search();
            None
        }
        KeyCode::Enter => {
            app.accept_search_selection();
            None
        }
        KeyCode::Up => {
            app.move_search_selection(-1);
            None
        }
        KeyCode::Down => {
            app.move_search_selection(1);
            None
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.move_search_selection(-1);
            None
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.move_search_selection(1);
            None
        }
        KeyCode::Backspace => app.pop_search_char(),
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            app.push_search_char(character)
        }
        _ => None,
    }
}

fn handle_search_mouse(app: &mut App, mouse: MouseEvent, screen: Rect) {
    let Some(search) = app.search.as_ref() else {
        return;
    };
    let (_, _, list_area) = search_layout(screen);
    if !list_area.contains((mouse.column, mouse.row).into()) || search.items.is_empty() {
        return;
    }

    // Ratatui scrolls a selected item into view. Reconstruct the same minimal
    // offset here so mouse rows keep addressing the item shown under the cursor.
    let visible_items = usize::from(list_area.height);
    let selected = search.selected.unwrap_or(0);
    let offset = selected.saturating_add(1).saturating_sub(visible_items);
    let index = offset + usize::from(mouse.row.saturating_sub(list_area.y));

    match mouse.kind {
        MouseEventKind::Moved => app.select_search_item(index),
        MouseEventKind::Down(MouseButton::Left) => {
            app.select_search_item(index);
            app.accept_search_selection();
        }
        MouseEventKind::ScrollUp => app.move_search_selection(-1),
        MouseEventKind::ScrollDown => app.move_search_selection(1),
        _ => {}
    }
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
            app.pan_viewport(
                i32::from(mouse.column) - i32::from(previous_column),
                i32::from(mouse.row) - i32::from(previous_row),
            );
            drag.previous = Some((mouse.column, mouse.row));
            return None;
        }
        MouseEventKind::Up(MouseButton::Left) => {
            drag.previous = None;
            return None;
        }
        MouseEventKind::Down(MouseButton::Left) => {}
        _ => return None,
    }
    let point = (mouse.column, mouse.row).into();
    let canvas = canvas_inner_area(screen);
    if !canvas.contains(point) {
        drag.previous = None;
        return None;
    }
    let layout = canvas_layout(canvas, &app.graph, app.selected, app.viewport);

    if let Some(placement) = layout
        .nodes
        .iter()
        .find(|placement| placement.incoming_button.contains(point))
    {
        drag.previous = None;
        return app
            .toggle_node_branch(
                placement.node_id,
                HierarchyDirection::Incoming,
                hierarchy_available,
            )
            .map(InteractionRequest::Hierarchy);
    }
    if let Some(placement) = layout
        .nodes
        .iter()
        .find(|placement| placement.outgoing_button.contains(point))
    {
        drag.previous = None;
        return app
            .toggle_node_branch(
                placement.node_id,
                HierarchyDirection::Outgoing,
                hierarchy_available,
            )
            .map(InteractionRequest::Hierarchy);
    }
    if let Some(placement) = layout
        .nodes
        .iter()
        .find(|placement| placement.area.contains(point))
    {
        app.select_node(placement.node_id);
    }
    drag.previous = Some((mouse.column, mouse.row));
    None
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
    let current_center = rect_center(current.area);
    let next = layout
        .nodes
        .iter()
        .filter(|candidate| candidate.node_id != current.node_id)
        .filter_map(|candidate| {
            let candidate_center = rect_center(candidate.area);
            navigation_score(current_center, candidate_center, direction)
                .map(|score| (score, candidate.node_id))
        })
        .min_by_key(|(score, node_id)| (*score, node_id.0));
    let Some((_, node_id)) = next else {
        return false;
    };
    app.select_node(node_id)
}

fn current_placement<'a>(
    layout: &'a [CanvasNodePlacement],
    selected: Option<NodeId>,
) -> Option<&'a CanvasNodePlacement> {
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

fn render(frame: &mut Frame, app: &App) {
    let [canvas, footer] = canvas_and_footer(frame.area());

    let canvas_block = Block::default()
        .title(" ctree ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let canvas_inner = canvas_block.inner(canvas);
    frame.render_widget(canvas_block, canvas);

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
            render_node(
                frame,
                node,
                placement,
                app.selected == Some(placement.node_id),
            );
        }
    }

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
        Some('t') => "t_: l toggle left / r toggle right".to_owned(),
        _ => hierarchy_failure
            .or_else(|| app.canvas_notice.clone())
            .unwrap_or_else(|| {
                "hjkl: move  drag: pan  tl/tr: toggle  ac/at: add  dd/dp/dn: delete  q/Esc: quit"
                    .to_owned()
            }),
    };
    render_footer(frame, footer, footer_text, &app.analysis_status);

    if let Some(search) = &app.search {
        render_search(frame, search);
    }
}

fn render_node(
    frame: &mut Frame,
    node: &GraphNode,
    placement: CanvasNodePlacement,
    selected: bool,
) {
    let border_style = if selected {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    };
    let content_style = if selected {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    frame.render_widget(
        Paragraph::new(node.symbol.as_str())
            .alignment(Alignment::Center)
            .style(content_style)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style),
            ),
        placement.area,
    );

    render_branch_button(
        frame,
        placement.incoming_button,
        &node.incoming,
        Color::Blue,
    );
    render_branch_button(
        frame,
        placement.outgoing_button,
        &node.outgoing,
        Color::Blue,
    );
}

fn render_branch_button(frame: &mut Frame, area: Rect, branch: &GraphBranch, color: Color) {
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
    frame.render_widget(Paragraph::new(label).style(style), area);
}

fn canvas_and_footer(screen: Rect) -> [Rect; 2] {
    Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(screen)
}

fn canvas_inner_area(screen: Rect) -> Rect {
    let [canvas, _] = canvas_and_footer(screen);
    Block::default().borders(Borders::ALL).inner(canvas)
}

fn render_footer(frame: &mut Frame, area: Rect, shortcuts: String, status: &AnalysisStatus) {
    let status_width = area.width.saturating_mul(2) / 5;
    let shortcut_width = area.width.saturating_sub(status_width);
    let shortcut_area = Rect::new(area.x, area.y, shortcut_width, area.height);
    let status_area = Rect::new(shortcut_area.right(), area.y, status_width, area.height);
    frame.render_widget(
        Paragraph::new(shortcuts).style(Style::default().fg(Color::DarkGray)),
        shortcut_area,
    );
    frame.render_widget(Paragraph::new(analysis_status_line(status)), status_area);
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
        Span::styled("│ ", Style::default().fg(Color::DarkGray)),
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

fn render_search(frame: &mut Frame, search: &SearchState) {
    let area = search_modal_area(frame.area());
    frame.render_widget(Clear, area);

    let title = match search.kind {
        SearchKind::Call => " Add call node ",
        SearchKind::Type => " Add type node ",
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block, area);

    let (input_area, status_area, list_area) = search_layout(frame.area());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::raw(&search.input),
        ])),
        input_area,
    );

    let (status, status_style) = match &search.status {
        SearchStatus::Debouncing => (
            "Waiting for typing pause…".to_owned(),
            Style::default().fg(Color::DarkGray),
        ),
        SearchStatus::Loading => (
            "Searching workspace symbols…".to_owned(),
            Style::default().fg(Color::Yellow),
        ),
        SearchStatus::Ready if search.items.is_empty() => (
            "No matching symbols".to_owned(),
            Style::default().fg(Color::DarkGray),
        ),
        SearchStatus::Ready => (
            format!("{} symbols", search.items.len()),
            Style::default().fg(Color::Green),
        ),
        SearchStatus::Error(error) => (error.clone(), Style::default().fg(Color::Red)),
    };
    frame.render_widget(Paragraph::new(status).style(status_style), status_area);

    let items = search.items.iter().map(|item| {
        let container = item
            .container_name
            .as_deref()
            .filter(|_| !item.name.contains("::"))
            .map(|name| format!("  [{name}]"))
            .unwrap_or_default();
        ListItem::new(Line::from(vec![
            Span::styled(&item.name, Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(container, Style::default().fg(Color::Cyan)),
            Span::styled(
                format!("  {}", item.location),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
    });
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut list_state = ListState::default().with_selected(search.selected);
    frame.render_stateful_widget(list, list_area, &mut list_state);
}

fn search_modal_area(screen: Rect) -> Rect {
    let width = screen.width.saturating_sub(2).min(100);
    let height = screen.height.saturating_sub(2).min(16);
    Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + u16::from(screen.height > height),
        width,
        height,
    )
}

fn search_layout(screen: Rect) -> (Rect, Rect, Rect) {
    let area = search_modal_area(screen);
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let [input, status, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);
    (input, status, list)
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    use tower_lsp::lsp_types::{SymbolKind, Url};

    use super::{
        CanvasDragState, EdgeVisualKind, InteractionRequest, NavigationDirection, apply_lsp_status,
        canvas_inner_area, canvas_layout, handle_canvas_key, handle_canvas_mouse,
        move_canvas_selection, placement_bounds, rect_center, render, search_item,
        symbol_matches_search, world_canvas_layout, world_rects_overlap,
    };
    use crate::{
        app::{AnalysisBackend, AnalysisPhase, App, SearchKind},
        cli::Cli,
        fetch::lsp::{LspStatusUpdate, WorkspaceSymbolMatch},
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
            name: "run".to_owned(),
            kind: SymbolKind::METHOD,
            container_name: Some("App".to_owned()),
            uri: Url::parse("file:///workspace/src/main.rs").unwrap(),
            range: None,
        });
        assert_eq!(method.name, "App::run");
    }

    #[test]
    fn maps_lsp_progress_without_losing_server_identity() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
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
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
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
        assert!(bottom_row.contains("Working 68%"));
        for y in 0..height - 1 {
            let row = (0..width).fold(String::new(), |mut row, x| {
                row.push_str(terminal.backend().buffer().cell((x, y)).unwrap().symbol());
                row
            });
            assert!(!row.contains("LSP: rust-analyzer"));
        }
    }

    #[test]
    fn delete_prefix_requires_a_complete_valid_command() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree", "call", "root"]).unwrap());

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
    fn lays_out_multiple_anchors_with_the_selection_at_the_center() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
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
    fn tl_and_tr_toggle_only_the_requested_side() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
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
        let mut app = App::from_cli(Cli::try_parse_from(["ctree", "call", "root"]).unwrap());
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

        let Some(InteractionRequest::Hierarchy(request)) = request else {
            panic!("first tl must schedule a hierarchy request");
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
    fn canvas_layout_only_contains_children_of_expanded_branches() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
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
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
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
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
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
            Cli::try_parse_from(["ctree", "call", "VeryLongClassName::very_long_method_name"])
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
    fn canvas_mouse_selects_nodes_and_toggles_side_buttons_independently() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
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
            second_placement.incoming_button.x + 1,
            second_placement.incoming_button.y,
        );
        assert!(app.graph.node(second_id).unwrap().incoming.expanded);
        assert!(!app.graph.node(second_id).unwrap().outgoing.expanded);

        click(
            &mut app,
            screen,
            second_placement.outgoing_button.x + 1,
            second_placement.outgoing_button.y,
        );
        assert!(app.graph.node(second_id).unwrap().incoming.expanded);
        assert!(app.graph.node(second_id).unwrap().outgoing.expanded);
    }

    #[test]
    fn first_mouse_side_button_schedules_its_branch_query() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree", "type", "Root"]).unwrap());
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

        let Some(InteractionRequest::Hierarchy(request)) = request else {
            panic!("first side-button click must schedule hierarchy loading");
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
    fn dragging_canvas_or_node_pans_viewport_and_reveals_offscreen_nodes() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
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
        assert!(
            !initial
                .nodes
                .iter()
                .any(|placement| placement.node_id == child_id)
        );

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
        assert!(
            after_node_drag
                .nodes
                .iter()
                .any(|placement| placement.node_id == child_id)
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
        assert!(
            after_background_drag
                .nodes
                .iter()
                .any(|placement| placement.node_id == child_id)
        );
        assert_eq!(
            app.graph.node(root_id).unwrap().outgoing.neighbors[0],
            child_id
        );
    }

    #[test]
    fn keyboard_navigation_uses_visible_node_geometry() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
        let root_id = pin(&mut app, "root", HierarchyKind::Call);
        let incoming_id = connect(&mut app, root_id, HierarchyDirection::Incoming, &["caller"])[0];
        let outgoing_id = connect(&mut app, root_id, HierarchyDirection::Outgoing, &["callee"])[0];
        app.graph.node_mut(root_id).unwrap().incoming.expanded = true;
        app.graph.node_mut(root_id).unwrap().outgoing.expanded = true;
        app.selected = Some(root_id);
        let screen = Rect::new(0, 0, 100, 24);

        navigate(&mut app, screen, KeyCode::Right);
        assert_eq!(app.selected, Some(outgoing_id));
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
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
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
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
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
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
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
}
