use super::App;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnalysisBackend {
    Lsp(String),
    TreeSitter(String),
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnalysisPhase {
    Inactive,
    Ready,
    Working,
    Warning,
    Error,
    Disconnected,
}

/// UI-independent status reported by the active source-analysis backend.
///
/// This intentionally does not reuse the workspace-search status: an LSP can
/// still be indexing after one symbol request has completed, and Tree-sitter
/// can have initialization work without an LSP request lifecycle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalysisStatus {
    pub backend: AnalysisBackend,
    pub phase: AnalysisPhase,
    pub message: Option<String>,
    pub percentage: Option<u32>,
}

impl AnalysisStatus {
    pub fn inactive(message: impl Into<String>) -> Self {
        Self {
            backend: AnalysisBackend::None,
            phase: AnalysisPhase::Inactive,
            message: Some(message.into()),
            percentage: None,
        }
    }

    pub fn lsp(server: impl Into<String>, phase: AnalysisPhase) -> Self {
        Self {
            backend: AnalysisBackend::Lsp(server.into()),
            phase,
            message: None,
            percentage: None,
        }
    }

    pub fn tree_sitter(language: impl Into<String>, phase: AnalysisPhase) -> Self {
        Self {
            backend: AnalysisBackend::TreeSitter(language.into()),
            phase,
            message: None,
            percentage: None,
        }
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            backend: AnalysisBackend::None,
            phase: AnalysisPhase::Error,
            message: Some(message.into()),
            percentage: None,
        }
    }
}

impl App {
    pub fn set_analysis_status(&mut self, status: AnalysisStatus) {
        let message = status
            .message
            .as_ref()
            .filter(|message| !message.is_empty());
        if matches!(
            status.phase,
            AnalysisPhase::Error | AnalysisPhase::Disconnected
        ) {
            if let Some(message) = message {
                self.analysis_error = Some(message.clone());
                self.set_canvas_error(message.clone());
            }
        } else {
            self.analysis_error = None;
            if status.phase == AnalysisPhase::Warning {
                if let Some(message) = message {
                    self.set_canvas_notice(message.clone());
                }
            } else if self.canvas_notice_is_error {
                self.clear_canvas_notice();
            }
        }
        self.analysis_status = status;
    }
}
