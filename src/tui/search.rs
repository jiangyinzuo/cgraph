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
    app::{App, SearchItem, SearchKind, SearchRequest, SearchState, SearchStatus},
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

    let (input_area, status_area, list_area) = layout(frame.area());
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

fn layout(screen: Rect) -> (Rect, Rect, Rect) {
    let area = modal_area(screen);
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
    use ratatui::layout::Rect;

    use super::{container_label, modal_area};
    use crate::app::SearchItem;

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
    }
}
