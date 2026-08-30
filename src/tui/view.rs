use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

use crate::app::{AnalysisStatus, App};

use super::{
    canvas::{CanvasConnections, CanvasNodeWidget, canvas_layout},
    help, messages, save, search, status,
};

#[cfg(test)]
pub(super) fn render(frame: &mut Frame, app: &App) {
    render_with_messages(frame, app, None);
}

pub(super) fn render_with_messages(
    frame: &mut Frame,
    app: &App,
    mut message_view: Option<&mut messages::MessageViewState>,
) {
    let [canvas, message_area, footer] = canvas_message_and_footer(frame.area());

    if app.graph.anchors().is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Empty canvas",
                Style::default().fg(Color::DarkGray),
            )))
            .alignment(Alignment::Center),
            canvas,
        );
    } else {
        let layout = canvas_layout(canvas, &app.graph, app.selected, app.viewport);
        frame.render_widget(
            CanvasConnections {
                edges: &layout.edges,
            },
            canvas,
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
                canvas,
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

pub(super) fn canvas_inner_area(screen: Rect) -> Rect {
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
    content.extend(status::line(status).spans);
    frame.render_widget(Paragraph::new(Line::from(content)), area);
}

pub(super) fn canvas_heading(app: &App) -> String {
    app.selected
        .and_then(|selected| app.graph.node(selected))
        .and_then(|node| node.location.as_ref())
        .map(|location| location.uri.trim())
        .filter(|uri| !uri.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| "CALL GRAPH".to_owned())
}
