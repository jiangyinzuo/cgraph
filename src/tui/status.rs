use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::{
    app::{AnalysisBackend, AnalysisPhase, AnalysisStatus, App},
    fetch::lsp::LspStatusUpdate,
};

pub(super) fn apply_lsp_status(app: &mut App, update: LspStatusUpdate) {
    let update = match update {
        LspStatusUpdate::Diagnostic(message) => {
            app.set_canvas_notice(message);
            return;
        }
        update => update,
    };
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
        LspStatusUpdate::Diagnostic(_) => unreachable!("diagnostics return before status mapping"),
    };
    app.set_analysis_status(status);
}

pub(super) fn line(status: &AnalysisStatus) -> Line<'static> {
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
