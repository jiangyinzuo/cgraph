//! Command-line syntax only; process startup remains in `main`.

use std::{ffi::OsString, path::PathBuf};

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "cgraph", version, about)]
pub struct Cli {
    /// Language server executable. If omitted, cgraph detects common project types.
    #[arg(long, global = true, value_name = "PROGRAM", conflicts_with = "no_lsp")]
    pub lsp: Option<OsString>,

    /// Argument passed to the configured language server.
    #[arg(
        long = "lsp-arg",
        global = true,
        value_name = "ARG",
        allow_hyphen_values = true,
        requires = "lsp"
    )]
    pub lsp_args: Vec<OsString>,

    /// Workspace directory supplied to the language server.
    #[arg(long, global = true, value_name = "PATH", default_value = ".")]
    pub workspace: PathBuf,

    /// Disable automatic language server startup.
    #[arg(long, global = true, conflicts_with = "lsp")]
    pub no_lsp: bool,

    /// Listen for editor IPC clients on this Unix socket path.
    #[arg(long, global = true, value_name = "PATH")]
    pub ipc_socket: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Show a function call hierarchy.
    Call {
        /// Function or method to use as the root node.
        symbol: String,
    },
    /// Show a type hierarchy.
    Type {
        /// Type to use as the root node.
        symbol: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command};
    use clap::Parser;

    #[test]
    fn parses_call_query() {
        let cli = Cli::try_parse_from(["cgraph", "call", "Foo::Bar"]).unwrap();

        assert!(matches!(
            cli.command,
            Some(Command::Call { symbol }) if symbol == "Foo::Bar"
        ));
    }

    #[test]
    fn accepts_an_empty_canvas() {
        let cli = Cli::try_parse_from(["cgraph"]).unwrap();

        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_language_server_options_after_subcommand() {
        let cli = Cli::try_parse_from([
            "cgraph",
            "type",
            "Student",
            "--lsp",
            "rust-analyzer",
            "--workspace",
            "/tmp/project",
        ])
        .unwrap();

        assert_eq!(
            cli.lsp.as_deref(),
            Some(std::ffi::OsStr::new("rust-analyzer"))
        );
        assert_eq!(cli.workspace, std::path::Path::new("/tmp/project"));
    }

    #[test]
    fn parses_an_ipc_socket_path() {
        let cli = Cli::try_parse_from([
            "cgraph",
            "--ipc-socket",
            "/run/user/1000/cgraph.sock",
            "call",
            "main",
        ])
        .unwrap();

        assert_eq!(
            cli.ipc_socket.as_deref(),
            Some(std::path::Path::new("/run/user/1000/cgraph.sock"))
        );
    }
}
