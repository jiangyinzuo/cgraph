use std::{sync::mpsc::Sender, time::Duration};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};
use tokio::{task::JoinHandle, time::sleep};
use tower_lsp::lsp_types::SymbolKind;

use crate::{
    app::{App, SearchField, SearchItem, SearchKind, SearchRequest, SearchState, SearchStatus},
    fetch::{WorkspaceSymbolClient, WorkspaceSymbolMatch},
    state::SourceLocation,
};

const WORKSPACE_SYMBOL_SEARCH_DELAY: Duration = Duration::from_millis(200);

pub(super) enum QueryEvent {
    Started(u64),
    Finished {
        request_id: u64,
        result: Result<Vec<SearchItem>, String>,
    },
}

pub(super) fn schedule(
    client: WorkspaceSymbolClient,
    request: SearchRequest,
    sender: Sender<QueryEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        sleep(WORKSPACE_SYMBOL_SEARCH_DELAY).await;
        if sender
            .send(QueryEvent::Started(request.request_id))
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
        let _ = sender.send(QueryEvent::Finished {
            request_id: request.request_id,
            result,
        });
    })
}

pub(super) fn handle_key(app: &mut App, key: KeyEvent) -> Option<SearchRequest> {
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
        KeyCode::Tab if key.modifiers == KeyModifiers::NONE => {
            app.cycle_search_field();
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

pub(super) fn handle_mouse(app: &mut App, mouse: MouseEvent, screen: Rect) {
    let Some(search) = app.search.as_ref() else {
        return;
    };
    let (_, _, list_area) = layout(screen);
    if !list_area.contains((mouse.column, mouse.row).into()) || search.items.is_empty() {
        return;
    }

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

pub(super) fn render(frame: &mut Frame, search: &SearchState) {
    let area = modal_area(frame.area());
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

    let (input_areas, status_area, list_area) = layout(frame.area());
    let inputs = [
        (
            input_areas[0],
            "LSP Query",
            search.lsp_query.as_str(),
            SearchField::LspQuery,
        ),
        (
            input_areas[1],
            "Symbol",
            search.symbol_query.as_str(),
            SearchField::Symbol,
        ),
        (
            input_areas[2],
            "URI",
            search.uri_query.as_str(),
            SearchField::Uri,
        ),
    ];
    for &(area, title, input, field) in &inputs {
        if field != search.active_field {
            render_input(frame, area, title, input, false);
        }
    }
    let (area, title, input, _) = inputs
        .into_iter()
        .find(|(_, _, _, field)| *field == search.active_field)
        .expect("active search field has an input area");
    // The boxes overlap by one row. Drawing the active box last keeps both of
    // its shared horizontal edges highlighted instead of letting a neighbor
    // overwrite one edge with the inactive style.
    render_input(frame, area, title, input, true);

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
            format!(
                "{} from provider · 0 matches · Tab switches field",
                search.candidate_count()
            ),
            Style::default().fg(Color::DarkGray),
        ),
        SearchStatus::Ready => (
            format!(
                "{} from provider · {} matches · Tab switches field",
                search.candidate_count(),
                search.items.len()
            ),
            Style::default().fg(Color::Green),
        ),
        SearchStatus::Error(error) => (error.clone(), Style::default().fg(Color::Red)),
    };
    frame.render_widget(Paragraph::new(status).style(status_style), status_area);

    let items = search.items.iter().map(|item| {
        let container = container_label(item)
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

fn render_input(frame: &mut Frame, area: Rect, title: &str, input: &str, active: bool) {
    let style = if active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default().borders(Borders::ALL).border_style(style);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let marker = if active { "> " } else { "  " };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{marker}{title:<9}│ "), style),
            Span::raw(input),
        ])),
        inner,
    );
}

fn container_label(item: &SearchItem) -> Option<&str> {
    item.container_name.as_deref().filter(|container| {
        !item.name.starts_with(&format!("{container}::"))
            && !item.name.starts_with(&format!("{container}."))
    })
}

pub(super) fn search_item(symbol: WorkspaceSymbolMatch) -> SearchItem {
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

pub(super) fn symbol_matches_search(search_kind: SearchKind, symbol_kind: SymbolKind) -> bool {
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

fn modal_area(screen: Rect) -> Rect {
    let width = proportional_dimension(screen.width, 4, 5);
    let height = proportional_dimension(screen.height, 7, 10);
    Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn proportional_dimension(total: u16, numerator: u16, denominator: u16) -> u16 {
    if total == 0 {
        return 0;
    }
    total
        .saturating_mul(numerator)
        .checked_div(denominator)
        .unwrap_or(total)
        .max(1)
        .min(total)
}

fn layout(screen: Rect) -> ([Rect; 3], Rect, Rect) {
    let area = modal_area(screen);
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let [inputs, status, list] = Layout::vertical([
        Constraint::Length(7),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);
    let input_area = |offset: u16| {
        let y = inputs.y.saturating_add(offset).min(inputs.bottom());
        Rect::new(
            inputs.x,
            y,
            inputs.width,
            inputs.bottom().saturating_sub(y).min(3),
        )
    };
    ([input_area(0), input_area(2), input_area(4)], status, list)
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;
    use ratatui::{Terminal, backend::TestBackend};

    use super::{container_label, handle_key, layout, modal_area, render};
    use crate::{
        app::{App, SearchField, SearchItem, SearchKind},
        cli::Cli,
    };

    #[test]
    fn hides_container_when_the_provider_name_is_already_qualified() {
        let mut item = SearchItem {
            name: "Worker::run".to_owned(),
            container_name: Some("Worker".to_owned()),
            location: "src/lib.rs:1".to_owned(),
            source: None,
        };
        assert_eq!(container_label(&item), None);

        item.name = "Worker.run".to_owned();
        assert_eq!(container_label(&item), None);

        item.name = "run".to_owned();
        assert_eq!(container_label(&item), Some("Worker"));
    }

    #[test]
    fn workspace_symbol_modal_uses_most_of_the_screen_without_overflowing() {
        let area = modal_area(Rect::new(0, 0, 120, 40));
        assert_eq!(area, Rect::new(12, 6, 96, 28));

        let small = modal_area(Rect::new(3, 4, 7, 5));
        assert!(small.x >= 3);
        assert!(small.y >= 4);
        assert!(small.right() <= 10);
        assert!(small.bottom() <= 9);
        assert!(small.width > 0);
        assert!(small.height > 0);

        let (inputs, status, _) = layout(Rect::new(0, 0, 120, 40));
        assert_eq!(inputs[0].bottom().saturating_sub(1), inputs[1].y);
        assert_eq!(inputs[1].bottom().saturating_sub(1), inputs[2].y);
        assert_eq!(inputs[2].bottom(), status.y);
    }

    #[test]
    fn tab_cycles_focus_without_scheduling_a_provider_query() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        app.open_search(SearchKind::Call, true).unwrap();
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);

        assert!(handle_key(&mut app, tab).is_none());
        assert_eq!(
            app.search.as_ref().unwrap().active_field,
            SearchField::Symbol
        );
        assert!(handle_key(&mut app, tab).is_none());
        assert_eq!(app.search.as_ref().unwrap().active_field, SearchField::Uri);
        assert!(handle_key(&mut app, tab).is_none());
        assert_eq!(
            app.search.as_ref().unwrap().active_field,
            SearchField::LspQuery
        );
    }

    #[test]
    fn renders_three_labeled_search_inputs() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        app.open_search(SearchKind::Call, true).unwrap();
        let search = app.search.as_mut().unwrap();
        search.lsp_query = "main worker".to_owned();
        search.symbol_query = "prs thrd".to_owned();
        search.uri_query = "src backend".to_owned();

        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, search)).unwrap();
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

        assert!(content.contains("> LSP Query"));
        assert!(content.contains("main worker"));
        assert!(content.contains("Symbol"));
        assert!(content.contains("prs thrd"));
        assert!(content.contains("URI"));
        assert!(content.contains("src backend"));

        let small_backend = TestBackend::new(7, 5);
        let mut small_terminal = Terminal::new(small_backend).unwrap();
        small_terminal.draw(|frame| render(frame, search)).unwrap();
    }
}
