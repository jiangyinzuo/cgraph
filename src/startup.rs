use std::path::Path;

use cgraph::{
    app::{AnalysisPhase, AnalysisStatus, App},
    fetch::{
        lsp::{LspConfig, LspProvider},
        treesitter::{TreeSitterLanguage, TreeSitterProvider},
    },
};

pub(super) struct AnalysisProviders {
    pub(super) lsp: Option<LspProvider>,
    pub(super) tree_sitter: Option<TreeSitterProvider>,
}

enum TreeSitterStartup {
    Ready(TreeSitterProvider),
    Unsupported,
    Failed {
        language: TreeSitterLanguage,
        error: anyhow::Error,
    },
}

pub(super) async fn start_analysis_providers(
    lsp_config: Option<LspConfig>,
    workspace: &Path,
    app: &mut App,
) -> AnalysisProviders {
    let (lsp, lsp_failed) = match lsp_config {
        Some(config) => {
            let server_name = config.program.to_string_lossy().into_owned();
            match LspProvider::start(config).await {
                Ok(lsp) => {
                    let server_name = lsp
                        .server_info()
                        .map(|info| info.name.clone())
                        .unwrap_or(server_name);
                    app.set_analysis_status(AnalysisStatus::lsp(server_name, AnalysisPhase::Ready));
                    (Some(lsp), false)
                }
                Err(error) => {
                    app.set_canvas_notice(format!(
                        "Failed to start LSP {server_name}: {error:#}; trying Tree-sitter fallback"
                    ));
                    app.set_analysis_status(AnalysisStatus::inactive(
                        "LSP unavailable; checking Tree-sitter fallback",
                    ));
                    (None, true)
                }
            }
        }
        None => {
            app.set_analysis_status(AnalysisStatus::inactive(
                "No LSP configured or detected; checking Tree-sitter fallback",
            ));
            (None, false)
        }
    };

    if let Some(lsp) = lsp {
        return AnalysisProviders {
            lsp: Some(lsp),
            tree_sitter: start_tree_sitter_hierarchy_fallback(workspace),
        };
    }

    let tree_sitter = match start_tree_sitter_fallback(workspace, app) {
        TreeSitterStartup::Ready(provider) => Some(provider),
        TreeSitterStartup::Unsupported => {
            let reason = if lsp_failed {
                "LSP startup failed and no supported Tree-sitter language was detected"
            } else {
                "no LSP was configured or detected and no supported Tree-sitter language was detected"
            };
            app.set_analysis_status(AnalysisStatus::unavailable(format!(
                "No analysis provider available: {reason}"
            )));
            None
        }
        TreeSitterStartup::Failed { language, error } => {
            let mut status = AnalysisStatus::tree_sitter(language.name(), AnalysisPhase::Error);
            status.message = Some(format!(
                "No analysis provider available: Tree-sitter {} initialization failed: {error:#}",
                language.name()
            ));
            app.set_analysis_status(status);
            None
        }
    };

    AnalysisProviders {
        lsp: None,
        tree_sitter,
    }
}

fn start_tree_sitter_fallback(workspace: &Path, app: &mut App) -> TreeSitterStartup {
    let Some(language) = TreeSitterLanguage::detect(workspace) else {
        return TreeSitterStartup::Unsupported;
    };

    let mut initializing = AnalysisStatus::tree_sitter(language.name(), AnalysisPhase::Working);
    initializing.message = Some("Initializing grammar and symbol query".to_owned());
    app.set_analysis_status(initializing);

    match TreeSitterProvider::start(workspace, language) {
        Ok(provider) => {
            let mut status = AnalysisStatus::tree_sitter(language.name(), AnalysisPhase::Ready);
            status.message = Some("Syntax index builds on first search or expansion".to_owned());
            app.set_analysis_status(status);
            TreeSitterStartup::Ready(provider)
        }
        Err(error) => TreeSitterStartup::Failed { language, error },
    }
}

fn start_tree_sitter_hierarchy_fallback(workspace: &Path) -> Option<TreeSitterProvider> {
    let language = TreeSitterLanguage::detect(workspace)?;
    TreeSitterProvider::start(workspace, language).ok()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use cgraph::{
        app::{AnalysisBackend, AnalysisPhase, App, SearchKind, SearchStatus},
        cli::Cli,
        fetch::{HierarchyQuery, lsp::LspConfig},
        state::{HierarchyDirection, HierarchyKind, SourceLocation, SymbolIdentity},
    };
    use clap::Parser;

    use super::start_analysis_providers;

    #[tokio::test]
    async fn initializes_a_queryable_tree_sitter_fallback_and_reports_ready() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("cgraph-main-{unique}"));
        fs::create_dir(&workspace).unwrap();
        fs::write(
            workspace.join("main.py"),
            "def helper():\n    pass\n\ndef main():\n    helper()\n",
        )
        .unwrap();
        let cli = Cli::try_parse_from([
            "cgraph",
            "--no-lsp",
            "--workspace",
            workspace.to_str().unwrap(),
        ])
        .unwrap();
        let mut app = App::from_cli(cli);

        let providers = start_analysis_providers(None, &workspace, &mut app).await;

        assert!(providers.lsp.is_none());
        let provider = providers.tree_sitter.unwrap();
        assert_eq!(
            app.analysis_status.backend,
            AnalysisBackend::TreeSitter("Python".to_owned())
        );
        assert_eq!(app.analysis_status.phase, AnalysisPhase::Ready);
        assert!(!app.canvas_notice_is_error());
        assert!(app.message_history.is_empty());
        let symbols = provider.workspace_symbol_client().query("").await.unwrap();
        let main = symbols.iter().find(|symbol| symbol.name == "main").unwrap();
        let position = main.range.unwrap().start;
        let response = provider
            .hierarchy_client()
            .query(HierarchyQuery {
                symbol: SymbolIdentity {
                    symbol: "main".to_owned(),
                    kind: HierarchyKind::Call,
                    location: Some(SourceLocation {
                        uri: main.uri.to_string(),
                        line: Some(position.line),
                        character: Some(position.character),
                    }),
                },
                direction: HierarchyDirection::Outgoing,
            })
            .await
            .unwrap();
        assert_eq!(
            response
                .children
                .iter()
                .map(|child| child.symbol.as_str())
                .collect::<Vec<_>>(),
            ["helper"]
        );
        fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn lsp_start_failure_falls_back_to_tree_sitter_without_a_stale_error() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("cgraph-lsp-fallback-{unique}"));
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join("main.py"), "def main():\n    pass\n").unwrap();
        let cli =
            Cli::try_parse_from(["cgraph", "--workspace", workspace.to_str().unwrap()]).unwrap();
        let mut app = App::from_cli(cli);
        let missing_server = workspace.join("missing-language-server");

        let providers = start_analysis_providers(
            Some(LspConfig::for_server(&missing_server, &workspace)),
            &workspace,
            &mut app,
        )
        .await;

        assert!(providers.lsp.is_none());
        assert!(providers.tree_sitter.is_some());
        assert_eq!(
            app.analysis_status.backend,
            AnalysisBackend::TreeSitter("Python".to_owned())
        );
        assert_eq!(app.analysis_status.phase, AnalysisPhase::Ready);
        assert!(!app.canvas_notice_is_error());
        assert!(
            app.message_history
                .iter()
                .any(|message| message.contains("trying Tree-sitter fallback"))
        );
        assert!(
            app.message_history
                .iter()
                .all(|message| !message.starts_with("No analysis provider available"))
        );

        fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn reports_an_error_only_after_all_analysis_providers_are_unavailable() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("cgraph-no-provider-{unique}"));
        fs::create_dir(&workspace).unwrap();
        let cli = Cli::try_parse_from([
            "cgraph",
            "--no-lsp",
            "--workspace",
            workspace.to_str().unwrap(),
        ])
        .unwrap();
        let mut app = App::from_cli(cli);

        let providers = start_analysis_providers(None, &workspace, &mut app).await;

        assert!(providers.lsp.is_none());
        assert!(providers.tree_sitter.is_none());
        assert_eq!(app.analysis_status.backend, AnalysisBackend::None);
        assert_eq!(app.analysis_status.phase, AnalysisPhase::Error);
        assert!(app.canvas_notice_is_error());
        let error = "No analysis provider available: no LSP was configured or detected and no supported Tree-sitter language was detected";
        assert_eq!(app.canvas_notice.as_deref(), Some(error));
        app.open_search(SearchKind::Call, false);
        assert_eq!(
            app.search.as_ref().unwrap().status,
            SearchStatus::Error(error.to_owned())
        );

        fs::remove_dir_all(workspace).unwrap();
    }
}
