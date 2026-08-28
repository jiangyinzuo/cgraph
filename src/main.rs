use std::{ffi::OsString, fs, path::Path};

use anyhow::Result;
use cgraph::{
    app::{AnalysisPhase, AnalysisStatus, App},
    cli::Cli,
    config::ProjectConfig,
    fetch::{
        HierarchyClient, WorkspaceSymbolClient,
        lsp::{LspConfig, LspProvider, builtin_file_extensions},
        treesitter::{TreeSitterLanguage, TreeSitterProvider},
    },
    ipc::IpcServer,
    tui,
};
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let workspace = cli.workspace.clone();
    let ipc_socket = cli.ipc_socket.clone();
    let project_config = ProjectConfig::load(&workspace)?;
    let lsp_config = lsp_config(&cli, &project_config);
    let mut app = App::from_cli(cli);
    app.set_filters(project_config.filters.clone());
    let mut ipc_server = match ipc_socket {
        Some(socket_path) => Some(IpcServer::start(socket_path)?),
        None => None,
    };
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
                    app.set_analysis_error(format!("Failed to start LSP: {error:#}"));
                    let mut status = AnalysisStatus::lsp(server_name, AnalysisPhase::Error);
                    status.message = Some(format!("Failed to start: {error:#}"));
                    app.set_analysis_status(status);
                    None
                }
            }
        }
        None => {
            app.set_analysis_error("No LSP or supported Tree-sitter provider is available");
            app.set_analysis_status(AnalysisStatus::inactive(
                "No LSP configured; checking Tree-sitter fallback",
            ));
            None
        }
    };
    let tree_sitter = if lsp.is_none() {
        start_tree_sitter_fallback(&workspace, &mut app)
    } else {
        start_tree_sitter_hierarchy_fallback(&workspace)
    };
    let symbol_client = lsp
        .as_ref()
        .map(LspProvider::workspace_symbol_client)
        .map(WorkspaceSymbolClient::from)
        .or_else(|| {
            tree_sitter
                .as_ref()
                .map(TreeSitterProvider::workspace_symbol_client)
                .map(WorkspaceSymbolClient::from)
        });
    let hierarchy_client = match (lsp.as_ref(), tree_sitter.as_ref()) {
        (Some(lsp), Some(tree_sitter)) => Some(HierarchyClient::with_fallback(
            lsp.hierarchy_client(),
            tree_sitter.hierarchy_client(),
        )),
        (Some(lsp), None) => Some(HierarchyClient::from(lsp.hierarchy_client())),
        (None, Some(tree_sitter)) => Some(HierarchyClient::from(tree_sitter.hierarchy_client())),
        (None, None) => None,
    };
    let lsp_status_receiver = lsp.as_mut().and_then(LspProvider::take_status_receiver);
    let ipc_event_sender = ipc_server.as_ref().map(IpcServer::event_sender);
    let ipc_command_receiver = ipc_server
        .as_mut()
        .and_then(IpcServer::take_command_receiver);
    let mut terminal = tui::init()?;
    let run_result = tui::run(
        &mut terminal,
        &mut app,
        symbol_client,
        hierarchy_client,
        lsp_status_receiver,
        ipc_event_sender,
        ipc_command_receiver,
    );
    let restore_result = tui::restore(&mut terminal);
    let mut result = run_result.and(restore_result);

    if let Some(ipc_server) = ipc_server {
        result = result.and(ipc_server.shutdown().await);
    }

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
            status.message = Some("Syntax index builds on first search or expansion".to_owned());
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

fn start_tree_sitter_hierarchy_fallback(workspace: &Path) -> Option<TreeSitterProvider> {
    let language = TreeSitterLanguage::detect(workspace)?;
    TreeSitterProvider::start(workspace, language).ok()
}

fn lsp_config(cli: &Cli, project_config: &ProjectConfig) -> Option<LspConfig> {
    if cli.no_lsp {
        return None;
    }

    let (program, args, name, file_extensions) = if let Some(program) = cli.lsp.clone() {
        (program, cli.lsp_args.clone(), None, None)
    } else if let Some(config) = project_config.lsp.as_ref() {
        (
            OsString::from(config.command.as_str()),
            config.args.iter().map(OsString::from).collect(),
            Some(config.name.clone()),
            config.file_extensions.clone(),
        )
    } else {
        let program = detect_language_server(&cli.workspace)?;
        let name = Path::new(&program)
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned);
        (program, Vec::new(), name, None)
    };
    let config = LspConfig::for_server(program, &cli.workspace)
        .workspace_only(project_config.workspace_only)
        .path_filter(project_config.path_filter.clone())
        .filters(project_config.filters.clone());
    let config = match name {
        Some(name) => config.server_name(name),
        None => config,
    };
    let config = match file_extensions {
        Some(file_extensions) => config.file_extensions(file_extensions),
        None => config,
    };
    Some(config.args(args))
}

fn detect_language_server(workspace: &Path) -> Option<OsString> {
    // Keep detection shallow and predictable. Recursive monorepo discovery can
    // choose the wrong language; users can override this convenience with --lsp.
    if workspace.join("Cargo.toml").is_file() {
        return Some(OsString::from("rust-analyzer"));
    }
    if workspace.join("compile_commands.json").is_file()
        || workspace.join("CMakeLists.txt").is_file()
        || contains_source_with_extension(workspace, builtin_file_extensions("clangd"))
    {
        return Some(OsString::from("clangd"));
    }
    if workspace.join("pyproject.toml").is_file()
        || workspace.join("pyrefly.toml").is_file()
        || workspace.join("setup.py").is_file()
        || workspace.join("requirements.txt").is_file()
        || contains_source_with_extension(workspace, &["py"])
    {
        return Some(OsString::from("pyrefly"));
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
                .is_some_and(|extension| {
                    extensions
                        .iter()
                        .any(|configured| extension.eq_ignore_ascii_case(configured))
                })
        })
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use cgraph::{
        app::{AnalysisBackend, AnalysisPhase, App},
        cli::Cli,
        config::{LspSettings, PathFilter, ProjectConfig, SymbolFilter},
        fetch::HierarchyQuery,
        state::{HierarchyDirection, HierarchyKind, SourceLocation, SymbolIdentity},
    };
    use clap::Parser;

    use super::{detect_language_server, lsp_config, start_tree_sitter_fallback};

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

        let provider = start_tree_sitter_fallback(&workspace, &mut app);

        let provider = provider.unwrap();
        assert_eq!(
            app.analysis_status.backend,
            AnalysisBackend::TreeSitter("Python".to_owned())
        );
        assert_eq!(app.analysis_status.phase, AnalysisPhase::Ready);
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

    #[test]
    fn selects_pyrefly_as_the_default_python_server_and_preserves_explicit_pylsp() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("cgraph-pyrefly-{unique}"));
        fs::create_dir(&workspace).unwrap();
        fs::write(workspace.join("pyrefly.toml"), "").unwrap();

        let detected =
            Cli::try_parse_from(["cgraph", "--workspace", workspace.to_str().unwrap()]).unwrap();
        let detected = lsp_config(&detected, &ProjectConfig::default()).unwrap();
        assert_eq!(detected.program, "pyrefly");
        assert_eq!(detected.args, ["lsp"].map(std::ffi::OsString::from));
        assert_eq!(detected.server_name.as_deref(), Some("pyrefly"));
        assert!(detected.workspace_only);

        let explicit = Cli::try_parse_from([
            "cgraph",
            "--workspace",
            workspace.to_str().unwrap(),
            "--lsp",
            "pylsp",
        ])
        .unwrap();
        let explicit = lsp_config(&explicit, &ProjectConfig::default()).unwrap();
        assert_eq!(explicit.program, "pylsp");
        assert!(explicit.args.is_empty());

        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn project_lsp_configuration_overrides_builtin_detection() {
        let cli = Cli::try_parse_from(["cgraph", "--workspace", "/workspace"]).unwrap();
        let project_config = ProjectConfig {
            filters: cgraph::config::FilterConfig::from_rules(std::iter::empty::<&str>(), true)
                .unwrap(),
            symbol_filter: SymbolFilter::default(),
            path_filter: PathFilter::default(),
            workspace_only: true,
            lsp: Some(LspSettings {
                name: "clangd".to_owned(),
                command: "custom-lsp".to_owned(),
                args: vec!["--stdio".to_owned(), "--trace".to_owned()],
                file_extensions: Some(vec!["cppm".to_owned(), "ixx".to_owned()]),
            }),
        };

        let config = lsp_config(&cli, &project_config).unwrap();

        assert_eq!(config.program, "custom-lsp");
        assert_eq!(
            config.args,
            ["--stdio", "--trace"]
                .map(std::ffi::OsString::from)
                .to_vec()
        );
        assert!(config.workspace_only);
        assert_eq!(config.file_extensions, ["cppm", "ixx"]);
    }

    #[test]
    fn detects_cpp_workspaces_from_header_extensions() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("cgraph-cpp-headers-{unique}"));
        fs::create_dir(&workspace).unwrap();

        for extension in ["h", "hh", "hpp", "hxx", "HXX"] {
            let path = workspace.join(format!("worker.{extension}"));
            fs::write(&path, "struct Worker {};\n").unwrap();
            assert_eq!(
                detect_language_server(&workspace),
                Some(std::ffi::OsString::from("clangd"))
            );
            fs::remove_file(path).unwrap();
        }

        fs::remove_dir_all(workspace).unwrap();
    }
}
