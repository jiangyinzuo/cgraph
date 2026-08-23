use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::app::{App, HelpState};

const HELP_ROWS: &[(&str, &str)] = &[
    ("Canvas", ""),
    ("?", "open or close this complete help"),
    ("ac / at", "search and add a call / type anchor"),
    ("tl / tr", "toggle the selected node's left / right branch"),
    ("r", "refresh both loaded directions of the selected node"),
    (
        "h j k l / arrows",
        "select the nearest visible node by geometry",
    ),
    (
        "mouse click",
        "select a node without moving it in the viewport",
    ),
    (
        "mouse double-click",
        "send an exact source location to editor clients",
    ),
    (
        "mouse side button",
        "select a node and toggle only that branch",
    ),
    ("mouse drag", "pan the viewport from a node or empty canvas"),
    ("dd", "unpin the selected anchor"),
    ("dp / dn", "clear the selected node's left / right branch"),
    (
        "ec",
        "edit .cgraph.toml, reload it, and refresh loaded branches",
    ),
    ("w", "open the graph save dialog"),
    ("q / Esc", "quit cgraph"),
    ("", ""),
    ("Search", ""),
    ("text / Backspace", "edit the fuzzy workspace-symbol query"),
    (
        "Up / Down",
        "move through results (Ctrl-p / Ctrl-n also work)",
    ),
    ("Enter / click", "accept the selected result"),
    ("mouse move / wheel", "highlight or scroll results"),
    ("Esc", "close search without adding an anchor"),
    ("", ""),
    ("Save", ""),
    ("text / Backspace", "edit the destination path"),
    (
        "Enter",
        "create the destination without overwriting an existing file",
    ),
    ("Esc", "close save without writing"),
    ("", ""),
    ("Help", ""),
    ("Up / Down, j / k", "scroll one line"),
    ("PageUp / PageDown", "scroll one page"),
    ("Home / End", "jump to the first / last help row"),
    ("mouse wheel", "scroll help"),
    ("? / q / Esc", "close help without quitting cgraph"),
];

pub(super) fn handle_key(app: &mut App, key: KeyEvent) {
    let Some(help) = app.help.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Char('?') if matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            app.close_help();
        }
        KeyCode::Char('q') | KeyCode::Esc if key.modifiers == KeyModifiers::NONE => {
            app.close_help();
        }
        KeyCode::Up | KeyCode::Char('k') if key.modifiers == KeyModifiers::NONE => {
            help.scroll = help.scroll.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') if key.modifiers == KeyModifiers::NONE => {
            help.scroll = help.scroll.saturating_add(1).min(HELP_ROWS.len() - 1);
        }
        KeyCode::PageUp => help.scroll = help.scroll.saturating_sub(10),
        KeyCode::PageDown => {
            help.scroll = help.scroll.saturating_add(10).min(HELP_ROWS.len() - 1);
        }
        KeyCode::Home => help.scroll = 0,
        KeyCode::End => help.scroll = HELP_ROWS.len() - 1,
        _ => {}
    }
}

pub(super) fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    let Some(help) = app.help.as_mut() else {
        return;
    };
    match mouse.kind {
        MouseEventKind::ScrollUp => help.scroll = help.scroll.saturating_sub(3),
        MouseEventKind::ScrollDown => {
            help.scroll = help.scroll.saturating_add(3).min(HELP_ROWS.len() - 1);
        }
        _ => {}
    }
}

pub(super) fn render(frame: &mut Frame, help: &HelpState) {
    let area = modal_area(frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(" Complete help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [body, footer] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    let lines = HELP_ROWS
        .iter()
        .map(|(key, description)| {
            if description.is_empty() {
                Line::from(Span::styled(*key, Style::default().fg(Color::Yellow)))
            } else {
                Line::from(vec![
                    Span::styled(format!("{key:<22}"), Style::default().fg(Color::Cyan)),
                    Span::raw(*description),
                ])
            }
        })
        .collect::<Vec<_>>();
    let max_scroll = lines.len().saturating_sub(usize::from(body.height));
    let scroll = help.scroll.min(max_scroll) as u16;
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), body);
    frame.render_widget(
        Paragraph::new("↑↓/jk scroll · PgUp/PgDn page · Home/End jump · ?/q/Esc close")
            .style(Style::default().fg(Color::DarkGray)),
        footer,
    );
}

fn modal_area(screen: Rect) -> Rect {
    let width = screen.width.saturating_sub(2).min(100);
    let height = screen.height.saturating_sub(2).min(36);
    Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    use super::super::{CanvasDragState, handle_event, render};
    use crate::{app::App, cli::Cli};

    #[test]
    fn question_mark_help_lists_commands_scrolls_and_captures_canvas_input() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "call", "root"]).unwrap());
        app.viewport.offset_x = 7;
        let screen = Rect::new(0, 0, 110, 40);
        let mut drag = CanvasDragState::default();

        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT)),
            true,
            screen,
            &mut drag,
        );
        assert!(app.help.is_some());
        let backend = TestBackend::new(screen.width, screen.height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
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
        assert!(content.contains("Complete help"));
        assert!(content.contains("edit .cgraph.toml"));
        assert!(content.contains("PageUp / PageDown"));

        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)),
            true,
            screen,
            &mut drag,
        );
        assert_eq!(app.pending_key, None);
        assert_eq!(app.graph.anchors().len(), 1);
        handle_event(
            &mut app,
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 10,
                row: 10,
                modifiers: KeyModifiers::NONE,
            }),
            true,
            screen,
            &mut drag,
        );
        assert_eq!(app.help.as_ref().unwrap().scroll, 3);
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)),
            true,
            screen,
            &mut drag,
        );
        let mut small_terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        small_terminal.draw(|frame| render(frame, &app)).unwrap();
        let small_content = small_terminal.backend().buffer().content().iter().fold(
            String::new(),
            |mut output, cell| {
                output.push_str(cell.symbol());
                output
            },
        );
        assert!(small_content.contains("mouse wheel"));
        assert!(small_content.contains("close help"));
        handle_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT)),
            true,
            screen,
            &mut drag,
        );
        assert!(app.help.is_none());
        assert!(!app.should_quit);
        assert_eq!(app.viewport.offset_x, 7);
    }
}
