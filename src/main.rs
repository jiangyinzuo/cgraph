use std::{ffi::OsString, fs, path::Path};

use anyhow::Result;
use clap::Parser;
use ctree::{
    app::{AnalysisPhase, AnalysisStatus, App},
    cli::Cli,
    config::ProjectConfig,
    fetch::{
        lsp::{LspConfig, LspProvider},
        treesitter::{TreeSitterLanguage, TreeSitterProvider},
    },
    tui,
};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let workspace = cli.workspace.clone();
    let project_config = ProjectConfig::load(&workspace)?;
    let lsp_config = lsp_config(&cli);
    let mut app = App::from_cli(cli);
    app.set_symbol_filter(project_config.symbol_filter);
    // LSP is an optional capability. A missing or broken server should degrade
    // the search modal with a visible error, not prevent the canvas from opening.
    let mut lsp = match lsp_config {
        Some(config) => {
            let server_name = config.program.to_string_lossy().into_owned();
            match LspProvider::start(config).await {
                Ok(lsp) => {
                    let server_name = lsp
                        .server_info()
                        .map(|info| info.name.clone())
                        .unwrap_or(server_name);
                    app.set_analysis_status(AnalysisStatus::lsp(server_name, AnalysisPhase::Ready));
                    Some(lsp)
                }
                Err(error) => {
                    app.set_lsp_error(format!("Failed to start LSP: {error:#}"));
                    let mut status = AnalysisStatus::lsp(server_name, AnalysisPhase::Error);
                    status.message = Some(format!("Failed to start: {error:#}"));
                    app.set_analysis_status(status);
                    None
                }
            }
        }
        None => {
            app.set_lsp_error("No language server detected; use --lsp PROGRAM");
            app.set_analysis_status(AnalysisStatus::inactive(
                "No LSP configured; checking Tree-sitter fallback",
            ));
            None
        }
    };
    let _tree_sitter = if lsp.is_none() {
        start_tree_sitter_fallback(&workspace, &mut app)
    } else {
        None
    };
    let symbol_client = lsp.as_ref().map(LspProvider::workspace_symbol_client);
    let hierarchy_client = lsp.as_ref().map(LspProvider::hierarchy_client);
    let lsp_status_receiver = lsp.as_mut().and_then(LspProvider::take_status_receiver);
    let mut terminal = tui::init()?;
    let run_result = tui::run(
        &mut terminal,
        &mut app,
        symbol_client,
        hierarchy_client,
        lsp_status_receiver,
    );
    let restore_result = tui::restore(&mut terminal);
    let mut result = run_result.and(restore_result);

    if let Some(lsp) = lsp {
        result = result.and(lsp.shutdown().await);
    }

    result
}

fn start_tree_sitter_fallback(workspace: &Path, app: &mut App) -> Option<TreeSitterProvider> {
    let Some(language) = TreeSitterLanguage::detect(workspace) else {
        if app.analysis_status.phase != AnalysisPhase::Error {
            app.set_analysis_status(AnalysisStatus::inactive(
                "No LSP and no supported Tree-sitter language detected",
            ));
        }
        return None;
    };

    let mut initializing = AnalysisStatus::tree_sitter(language.name(), AnalysisPhase::Working);
    initializing.message = Some("Initializing grammar and symbol query".to_owned());
    app.set_analysis_status(initializing);

    match TreeSitterProvider::start(workspace, language) {
        Ok(provider) => {
            let mut status = AnalysisStatus::tree_sitter(language.name(), AnalysisPhase::Ready);
            status.message = Some("Grammar/query ready; search needs LSP".to_owned());
            app.set_analysis_status(status);
            Some(provider)
        }
        Err(error) => {
            let mut status = AnalysisStatus::tree_sitter(language.name(), AnalysisPhase::Error);
            status.message = Some(format!("Grammar/query initialization failed: {error:#}"));
            app.set_analysis_status(status);
            None
        }
    }
}

fn lsp_config(cli: &Cli) -> Option<LspConfig> {
    if cli.no_lsp {
        return None;
    }

    let program = cli
        .lsp
        .clone()
        .or_else(|| detect_language_server(&cli.workspace))?;
    Some(LspConfig::new(program, &cli.workspace).args(cli.lsp_args.clone()))
}

fn detect_language_server(workspace: &Path) -> Option<OsString> {
    // Keep detection shallow and predictable. Recursive monorepo discovery can
    // choose the wrong language; users can override this convenience with --lsp.
    if workspace.join("Cargo.toml").is_file() {
        return Some(OsString::from("rust-analyzer"));
    }
    if workspace.join("compile_commands.json").is_file()
        || workspace.join("CMakeLists.txt").is_file()
        || contains_source_with_extension(workspace, &["c", "cc", "cpp", "cxx", "h", "hpp"])
    {
        return Some(OsString::from("clangd"));
    }
    if workspace.join("pyproject.toml").is_file()
        || workspace.join("setup.py").is_file()
        || workspace.join("requirements.txt").is_file()
        || contains_source_with_extension(workspace, &["py"])
    {
        return Some(OsString::from("pylsp"));
    }

    None
}

fn contains_source_with_extension(workspace: &Path, extensions: &[&str]) -> bool {
    fs::read_dir(workspace).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extensions.contains(&extension))
        })
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use clap::Parser;
    use ctree::{
        app::{AnalysisBackend, AnalysisPhase, App},
        cli::Cli,
    };

    use super::start_tree_sitter_fallback;

    #[test]
    fn initializes_tree_sitter_fallback_and_reports_ready() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("ctree-main-{unique}"));
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join("main.py"), "def main():\n    pass\n").unwrap();
        let cli = Cli::try_parse_from([
            "ctree",
            "--no-lsp",
            "--workspace",
            workspace.to_str().unwrap(),
        ])
        .unwrap();
        let mut app = App::from_cli(cli);

        let provider = start_tree_sitter_fallback(&workspace, &mut app);

        assert!(provider.is_some());
        assert_eq!(
            app.analysis_status.backend,
            AnalysisBackend::TreeSitter("Python".to_owned())
        );
        assert_eq!(app.analysis_status.phase, AnalysisPhase::Ready);
        fs::remove_dir_all(workspace).unwrap();
    }
}
