use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use unicode_width::UnicodeWidthChar;

pub(super) struct MessageViewState {
    source: String,
    vertical_offset: u16,
    content_rows: usize,
    viewport_rows: usize,
    follow_tail: bool,
    wrap_width: u16,
    wrapped_rows: Vec<WrappedRow>,
    selection: Option<LineSelection>,
    notice: Option<String>,
}

pub(super) enum MessageAction {
    Handled,
    Close,
    Copy(String),
}

struct WrappedRow {
    text: String,
    source_start: usize,
    source_end_for_copy: usize,
}

#[derive(Clone, Copy)]
struct LineSelection {
    anchor: usize,
    cursor: usize,
}

impl MessageViewState {
    pub(super) fn from_messages(messages: &[String]) -> Self {
        Self {
            source: message_source(messages),
            vertical_offset: 0,
            content_rows: 0,
            viewport_rows: 0,
            follow_tail: true,
            wrap_width: 0,
            wrapped_rows: Vec::new(),
            selection: None,
            notice: None,
        }
    }

    pub(super) fn sync(&mut self, messages: &[String]) {
        let source = message_source(messages);
        if self.source != source {
            self.follow_tail = self.is_at_bottom();
            self.source = source;
            self.selection = None;
            self.notice = None;
        }
    }

    fn prepare_layout(
        &mut self,
        wrapped_rows: Vec<WrappedRow>,
        wrap_width: u16,
        viewport_rows: usize,
    ) {
        if self.wrap_width != 0 && self.wrap_width != wrap_width {
            self.selection = None;
        }
        self.wrap_width = wrap_width;
        self.wrapped_rows = wrapped_rows;
        self.content_rows = self.wrapped_rows.len();
        self.viewport_rows = viewport_rows;
        if self.follow_tail {
            self.vertical_offset = self.max_offset();
        } else {
            self.vertical_offset = self.vertical_offset.min(self.max_offset());
        }
    }

    fn max_offset(&self) -> u16 {
        self.content_rows
            .saturating_sub(self.viewport_rows)
            .min(usize::from(u16::MAX)) as u16
    }

    fn is_at_bottom(&self) -> bool {
        self.vertical_offset >= self.max_offset()
    }

    fn scroll_down(&mut self, rows: u16) {
        self.vertical_offset = self
            .vertical_offset
            .saturating_add(rows)
            .min(self.max_offset());
        self.follow_tail = self.is_at_bottom();
    }

    fn scroll_up(&mut self, rows: u16) {
        self.vertical_offset = self.vertical_offset.saturating_sub(rows);
        self.follow_tail = false;
    }

    fn page_rows(&self) -> u16 {
        self.viewport_rows
            .saturating_sub(1)
            .max(1)
            .min(usize::from(u16::MAX)) as u16
    }

    fn half_page_rows(&self) -> u16 {
        self.viewport_rows
            .div_ceil(2)
            .max(1)
            .min(usize::from(u16::MAX)) as u16
    }

    fn scroll_to_top(&mut self) {
        self.vertical_offset = 0;
        self.follow_tail = false;
    }

    fn scroll_to_bottom(&mut self) {
        self.vertical_offset = self.max_offset();
        self.follow_tail = true;
    }

    fn toggle_line_selection(&mut self) {
        self.notice = None;
        if self.selection.take().is_some() {
            return;
        }
        if let Some(active_row) = self.active_row() {
            self.selection = Some(LineSelection {
                anchor: active_row,
                cursor: active_row,
            });
        }
    }

    fn active_row(&self) -> Option<usize> {
        if self.wrapped_rows.is_empty() {
            return None;
        }
        Some(
            usize::from(self.vertical_offset)
                .saturating_add(self.viewport_rows.saturating_sub(1))
                .min(self.wrapped_rows.len() - 1),
        )
    }

    fn move_selection(&mut self, rows: isize) {
        let Some(selection) = self.selection.as_mut() else {
            return;
        };
        let last_row = self.wrapped_rows.len().saturating_sub(1);
        selection.cursor = selection.cursor.saturating_add_signed(rows).min(last_row);
        self.ensure_selection_cursor_visible();
    }

    fn move_selection_to(&mut self, row: usize) {
        let Some(selection) = self.selection.as_mut() else {
            return;
        };
        selection.cursor = row.min(self.wrapped_rows.len().saturating_sub(1));
        self.ensure_selection_cursor_visible();
    }

    fn ensure_selection_cursor_visible(&mut self) {
        let Some(selection) = self.selection else {
            return;
        };
        let viewport_start = usize::from(self.vertical_offset);
        let viewport_end = viewport_start
            .saturating_add(self.viewport_rows.saturating_sub(1))
            .min(self.wrapped_rows.len().saturating_sub(1));
        if selection.cursor < viewport_start {
            self.vertical_offset = selection.cursor.min(usize::from(u16::MAX)) as u16;
        } else if selection.cursor > viewport_end {
            self.vertical_offset = selection
                .cursor
                .saturating_add(1)
                .saturating_sub(self.viewport_rows)
                .min(usize::from(u16::MAX)) as u16;
        }
        self.vertical_offset = self.vertical_offset.min(self.max_offset());
        self.follow_tail = self.is_at_bottom();
    }

    fn selection_bounds(&self) -> Option<(usize, usize)> {
        let selection = self.selection?;
        Some(if selection.anchor <= selection.cursor {
            (selection.anchor, selection.cursor)
        } else {
            (selection.cursor, selection.anchor)
        })
    }

    fn is_selected(&self, row: usize) -> bool {
        self.selection_bounds()
            .is_some_and(|(start, end)| (start..=end).contains(&row))
    }

    fn selected_text(&self) -> Option<String> {
        let (start_row, end_row) = self.selection_bounds()?;
        let start = self.wrapped_rows.get(start_row)?.source_start;
        let end = self.wrapped_rows.get(end_row)?.source_end_for_copy;
        self.source.get(start..end).map(str::to_owned)
    }

    pub(super) fn set_copy_notice(&mut self, notice: String) {
        self.notice = Some(notice);
    }

    fn title(&self) -> String {
        if let Some(notice) = &self.notice {
            return format!(" Messages · {notice} · V select · q close ");
        }
        if self.selection.is_some() {
            " Messages · LINE SELECT · j/k extend · y copy · V cancel · q close ".to_owned()
        } else {
            " Messages · j/k scroll · Space/b page · g/G ends · V select · q close ".to_owned()
        }
    }
}

pub(super) fn handle_key(view: &mut MessageViewState, key: KeyEvent) -> MessageAction {
    view.notice = None;
    match (key.code, key.modifiers) {
        (KeyCode::Char('q') | KeyCode::Esc, KeyModifiers::NONE) => MessageAction::Close,
        (KeyCode::Char('V'), KeyModifiers::NONE | KeyModifiers::SHIFT)
        | (KeyCode::Char('v'), KeyModifiers::SHIFT) => {
            view.toggle_line_selection();
            MessageAction::Handled
        }
        (KeyCode::Char('y'), KeyModifiers::NONE) => {
            let Some(text) = view.selected_text().filter(|text| !text.is_empty()) else {
                view.notice = Some("nothing selected".to_owned());
                return MessageAction::Handled;
            };
            view.selection = None;
            MessageAction::Copy(text)
        }
        (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
            if view.selection.is_some() {
                view.move_selection(1);
            } else {
                view.scroll_down(1);
            }
            MessageAction::Handled
        }
        (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
            if view.selection.is_some() {
                view.move_selection(-1);
            } else {
                view.scroll_up(1);
            }
            MessageAction::Handled
        }
        (KeyCode::Char('f') | KeyCode::Char(' ') | KeyCode::PageDown, KeyModifiers::NONE) => {
            let rows = view.page_rows();
            if view.selection.is_some() {
                view.move_selection(rows as isize);
            } else {
                view.scroll_down(rows);
            }
            MessageAction::Handled
        }
        (KeyCode::Char('b') | KeyCode::PageUp, KeyModifiers::NONE) => {
            let rows = view.page_rows();
            if view.selection.is_some() {
                view.move_selection(-(rows as isize));
            } else {
                view.scroll_up(rows);
            }
            MessageAction::Handled
        }
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
            let rows = view.half_page_rows();
            if view.selection.is_some() {
                view.move_selection(rows as isize);
            } else {
                view.scroll_down(rows);
            }
            MessageAction::Handled
        }
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
            let rows = view.half_page_rows();
            if view.selection.is_some() {
                view.move_selection(-(rows as isize));
            } else {
                view.scroll_up(rows);
            }
            MessageAction::Handled
        }
        (KeyCode::Char('g') | KeyCode::Home, KeyModifiers::NONE) => {
            if view.selection.is_some() {
                view.move_selection_to(0);
            } else {
                view.scroll_to_top();
            }
            MessageAction::Handled
        }
        (KeyCode::Char('G'), KeyModifiers::NONE | KeyModifiers::SHIFT) | (KeyCode::End, _) => {
            if view.selection.is_some() {
                view.move_selection_to(view.wrapped_rows.len().saturating_sub(1));
            } else {
                view.scroll_to_bottom();
            }
            MessageAction::Handled
        }
        _ => MessageAction::Handled,
    }
}

pub(super) fn render(frame: &mut Frame, view: &mut MessageViewState, screen: Rect) {
    let area = message_area(screen);
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::Red))
        .title(view.title());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 2 || inner.height == 0 {
        return;
    }

    let [text_area, scrollbar_area] =
        Layout::horizontal([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    let rows = wrapped_rows(&view.source, text_area.width);
    view.prepare_layout(rows, text_area.width, usize::from(text_area.height));

    let lines = view
        .wrapped_rows
        .iter()
        .enumerate()
        .map(|(row, wrapped)| {
            let style = if view.is_selected(row) {
                Style::default().fg(Color::White).bg(Color::DarkGray)
            } else {
                Style::default()
            };
            Line::styled(wrapped.text.clone(), style)
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        Paragraph::new(lines).scroll((view.vertical_offset, 0)),
        text_area,
    );

    let mut scrollbar_state = ScrollbarState::new(view.content_rows)
        .position(usize::from(view.vertical_offset))
        .viewport_content_length(view.viewport_rows);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_style(Style::default().fg(Color::DarkGray))
            .thumb_style(Style::default().fg(Color::Red)),
        scrollbar_area,
        &mut scrollbar_state,
    );
}

fn message_source(messages: &[String]) -> String {
    if messages.is_empty() {
        "No messages.".to_owned()
    } else {
        messages.join("\n")
    }
}

fn message_area(screen: Rect) -> Rect {
    let available_height = screen.height.saturating_sub(1);
    let height = available_height.min(15);
    Rect::new(
        screen.x,
        screen.y + available_height.saturating_sub(height),
        screen.width,
        height,
    )
}

fn wrapped_rows(source: &str, width: u16) -> Vec<WrappedRow> {
    let width = usize::from(width.max(1));
    let mut rows = Vec::new();
    let mut source_offset = 0usize;
    for segment in source.split_inclusive('\n') {
        let line_length = if segment.ends_with("\r\n") {
            segment.len().saturating_sub(2)
        } else if segment.ends_with('\n') {
            segment.len().saturating_sub(1)
        } else {
            segment.len()
        };
        let source_line = &segment[..line_length];
        let line_start = source_offset;
        let segment_end = source_offset.saturating_add(segment.len());
        let mut current = String::new();
        let mut current_width = 0usize;
        let mut current_start = line_start;
        for (relative_offset, character) in source_line.char_indices() {
            let character_width = if character == '\t' {
                4
            } else {
                UnicodeWidthChar::width(character).unwrap_or(0)
            };
            if current_width > 0 && current_width.saturating_add(character_width) > width {
                let source_end = line_start.saturating_add(relative_offset);
                rows.push(WrappedRow {
                    text: current,
                    source_start: current_start,
                    source_end_for_copy: source_end,
                });
                current = String::new();
                current_width = 0;
                current_start = source_end;
            }
            if character == '\t' {
                current.push_str("    ");
            } else {
                current.push(character);
            }
            current_width = current_width.saturating_add(character_width);
        }
        rows.push(WrappedRow {
            text: current,
            source_start: current_start,
            source_end_for_copy: segment_end,
        });
        source_offset = segment_end;
    }
    if source.is_empty() || source.ends_with('\n') {
        rows.push(WrappedRow {
            text: String::new(),
            source_start: source.len(),
            source_end_for_copy: source.len(),
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    use super::{MessageAction, MessageViewState, handle_key, message_area, render, wrapped_rows};

    #[test]
    fn pager_supports_less_navigation_wrapping_and_quit() {
        let mut view = MessageViewState::from_messages(
            &(1..=20)
                .map(|line| format!("error {line}"))
                .collect::<Vec<_>>(),
        );
        let rows = wrapped_rows(&view.source, 80);
        view.prepare_layout(rows, 80, 5);
        assert_eq!(view.vertical_offset, 15);

        handle_key(
            &mut view,
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        );
        assert_eq!(view.vertical_offset, 14);
        handle_key(
            &mut view,
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
        );
        assert_eq!(view.vertical_offset, 10);
        handle_key(
            &mut view,
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        );
        assert_eq!(view.vertical_offset, 0);
        handle_key(
            &mut view,
            KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
        );
        assert_eq!(view.vertical_offset, 15);
        assert_eq!(
            wrapped_rows("abcdef\n界面", 3)
                .into_iter()
                .map(|row| row.text)
                .collect::<Vec<_>>(),
            ["abc", "def", "界", "面"]
        );

        let mut selection_view = MessageViewState::from_messages(&["abcdef".to_owned()]);
        let rows = wrapped_rows(&selection_view.source, 3);
        selection_view.prepare_layout(rows, 3, 2);
        handle_key(
            &mut selection_view,
            KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT),
        );
        handle_key(
            &mut selection_view,
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        );
        let MessageAction::Copy(copied) = handle_key(
            &mut selection_view,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        ) else {
            panic!("y must copy the active line selection");
        };
        assert_eq!(copied, "abcdef");
        assert!(selection_view.selection.is_none());
        assert!(matches!(
            handle_key(
                &mut view,
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            ),
            MessageAction::Close
        ));
    }

    #[test]
    fn message_view_is_bottom_aligned_and_limited_to_fifteen_rows() {
        assert_eq!(
            message_area(Rect::new(0, 0, 100, 40)),
            Rect::new(0, 24, 100, 15)
        );
        assert_eq!(
            message_area(Rect::new(0, 0, 100, 10)),
            Rect::new(0, 0, 100, 9)
        );

        let messages = (1..=20)
            .map(|line| format!("line-{line:02}"))
            .collect::<Vec<_>>();
        let mut view = MessageViewState::from_messages(&messages);
        let mut terminal = Terminal::new(TestBackend::new(40, 20)).unwrap();
        terminal
            .draw(|frame| render(frame, &mut view, frame.area()))
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
        assert!(content.contains("Messages"));
        assert!(content.contains("line-20"));
        assert!(!content.contains("line-01"));
    }
}
