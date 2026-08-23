use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::{
    app::{App, SaveState, SaveStatus},
    export,
};

pub(super) fn handle_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.close_save(),
        KeyCode::Enter => save(app),
        KeyCode::Backspace => app.pop_save_char(),
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            app.push_save_char(character);
        }
        _ => {}
    }
}

pub(super) fn render(frame: &mut Frame, save: &SaveState) {
    let area = modal_area(frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default().title(" Save graph ").borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [input_area, status_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::raw(&save.input),
        ])),
        input_area,
    );
    let (message, style) = match &save.status {
        SaveStatus::Editing => (
            "Enter a new path; existing targets are never overwritten",
            Style::default().fg(Color::DarkGray),
        ),
        SaveStatus::Error(error) => (error.as_str(), Style::default().fg(Color::Red)),
    };
    frame.render_widget(Paragraph::new(message).style(style), status_area);
}

fn save(app: &mut App) {
    let Some(input) = app.save.as_ref().map(|save| save.input.clone()) else {
        return;
    };
    let path = PathBuf::from(input);
    match export::write_text(&app.graph, &path) {
        Ok(()) => app.complete_save(&path),
        Err(error) => app.fail_save(format!("{error:#}")),
    }
}

fn modal_area(screen: Rect) -> Rect {
    let width = screen.width.saturating_sub(2).min(90);
    let height = screen.height.min(5);
    Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use std::{fs, time::SystemTime};

    use clap::Parser;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    use super::{handle_key, render};
    use crate::{
        app::{App, SaveStatus},
        cli::Cli,
    };

    #[test]
    fn existing_target_stays_unchanged_and_error_remains_in_modal() {
        let workspace = temporary_workspace("existing");
        let target = workspace.join("graph.txt");
        fs::write(&target, "keep me\n").unwrap();
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "call", "root"]).unwrap());
        app.open_save();
        app.save.as_mut().unwrap().input = target.display().to_string();

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(fs::read_to_string(&target).unwrap(), "keep me\n");
        assert!(matches!(
            app.save.as_ref().map(|save| &save.status),
            Some(SaveStatus::Error(error)) if error.contains("failed to create export")
        ));
        let backend = TestBackend::new(100, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, app.save.as_ref().unwrap()))
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
        assert!(content.contains("Save graph"));
        assert!(content.contains("failed to create export"));
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn successful_save_closes_modal_and_reports_the_path() {
        let workspace = temporary_workspace("success");
        let target = workspace.join("graph.txt");
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "call", "root"]).unwrap());
        app.open_save();
        app.save.as_mut().unwrap().input = target.display().to_string();

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(app.save.is_none());
        let notice = format!("Saved graph to {}", target.display());
        assert_eq!(app.canvas_notice.as_deref(), Some(notice.as_str()));
        let written = fs::read_to_string(&target).unwrap();
        assert!(written.starts_with("cgraph graph · text v1"));
        assert!(written.contains("[1] call  root  [anchor]"));
        fs::remove_dir_all(workspace).unwrap();
    }

    fn temporary_workspace(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("cgraph-save-{name}-{unique}"));
        fs::create_dir(&path).unwrap();
        path
    }
}
