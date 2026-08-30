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
}

impl App {
    pub fn set_analysis_error(&mut self, error: impl Into<String>) {
        let error = error.into();
        self.analysis_error = Some(error.clone());
        self.set_canvas_error(error);
    }

    pub fn set_analysis_status(&mut self, status: AnalysisStatus) {
        if matches!(
            status.phase,
            AnalysisPhase::Warning | AnalysisPhase::Error | AnalysisPhase::Disconnected
        ) && let Some(message) = status.message.as_ref()
            && !message.is_empty()
        {
            if matches!(
                status.phase,
                AnalysisPhase::Error | AnalysisPhase::Disconnected
            ) {
                self.set_canvas_error(message.clone());
            } else {
                self.set_canvas_notice(message.clone());
            }
        }
        self.analysis_status = status;
    }
}
