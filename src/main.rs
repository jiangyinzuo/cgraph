use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use cgraph::{
    app::App,
    cli::Cli,
    config::ProjectConfig,
    fetch::{
        HierarchyClient, WorkspaceSymbolClient,
        lsp::{LspConfig, LspProvider, builtin_file_extensions},
        treesitter::TreeSitterProvider,
    },
    ipc::IpcServer,
    tui,
};
use clap::Parser;

mod startup;

use startup::{AnalysisProviders, start_analysis_providers};

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
    let AnalysisProviders {
        mut lsp,
        tree_sitter,
    } = start_analysis_providers(lsp_config, &workspace, &mut app).await;
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

fn lsp_config(cli: &Cli, project_config: &ProjectConfig) -> Option<LspConfig> {
    if cli.no_lsp {
        return None;
    }

    let (program, args, name, file_extensions, configured_log) =
        if let Some(program) = cli.lsp.clone() {
            (program, cli.lsp_args.clone(), None, None, None)
        } else if let Some(config) = project_config.lsp.as_ref() {
            (
                OsString::from(config.command.as_str()),
                config.args.iter().map(OsString::from).collect(),
                Some(config.name.clone()),
                config.file_extensions.clone(),
                config.log_file.clone(),
            )
        } else {
            let program = detect_language_server(&cli.workspace)?;
            let name = Path::new(&program)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned);
            (program, Vec::new(), name, None, None)
        };
    let log_file = cli.lsp_log.clone().or(configured_log).unwrap_or_else(|| {
        default_lsp_log_path(name.as_deref().unwrap_or_else(|| {
            Path::new(&program)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("lsp")
        }))
    });
    let config = LspConfig::for_server(program, &cli.workspace)
        .filters(project_config.filters.clone())
        .stderr_log(log_file);
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

fn default_lsp_log_path(server_name: &str) -> PathBuf {
    let server_name = server_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    std::env::temp_dir().join(format!("cgraph-{server_name}-{}.log", std::process::id()))
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
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    use cgraph::{
        cli::Cli,
        config::{FilterConfig, LspSettings, ProjectConfig},
    };
    use clap::Parser;

    use super::{default_lsp_log_path, detect_language_server, lsp_config};

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
        assert!(detected.filters.workspace_only());
        assert_eq!(
            detected.stderr_log.as_deref().and_then(Path::parent),
            Some(std::env::temp_dir().as_path())
        );

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
            filters: FilterConfig::default(),
            lsp: Some(LspSettings {
                name: "clangd".to_owned(),
                command: "custom-lsp".to_owned(),
                args: vec!["--stdio".to_owned(), "--trace".to_owned()],
                file_extensions: Some(vec!["cppm".to_owned(), "ixx".to_owned()]),
                log_file: Some(std::path::PathBuf::from("logs/lsp.log")),
            }),
        };

        let config = lsp_config(&cli, &project_config).unwrap();

        assert_eq!(config.program, "custom-lsp");
        assert_eq!(
            config.args,
            ["--background-index", "--stdio", "--trace"]
                .map(std::ffi::OsString::from)
                .to_vec()
        );
        assert!(config.filters.workspace_only());
        assert_eq!(config.file_extensions, ["cppm", "ixx"]);
        assert_eq!(
            config.stderr_log,
            Some(std::path::PathBuf::from("logs/lsp.log"))
        );
    }

    #[test]
    fn default_lsp_log_uses_temp_directory_and_safe_server_name() {
        let path = default_lsp_log_path("wrapped/clangd");
        assert_eq!(path.parent(), Some(std::env::temp_dir().as_path()));
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("cgraph-wrapped_clangd-")
        );
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
