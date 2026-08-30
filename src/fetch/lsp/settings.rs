use std::{ffi::OsString, path::PathBuf};

use serde_json::Value;

use crate::config::FilterConfig;

use super::profile::{
    append_configured_args, apply_default_args, file_extensions,
    from_name as server_profile_from_name, from_program as server_profile_from_program,
};

pub fn builtin_file_extensions(server_name: &str) -> &'static [&'static str] {
    file_extensions(server_name)
}

#[derive(Clone, Debug)]
pub struct LspConfig {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub workspace_root: PathBuf,
    pub initialization_options: Option<Value>,
    pub filters: FilterConfig,
    pub server_name: Option<String>,
    pub file_extensions: Vec<String>,
    pub stderr_log: Option<PathBuf>,
}

impl LspConfig {
    fn new(program: impl Into<OsString>, workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            workspace_root: workspace_root.into(),
            initialization_options: None,
            filters: FilterConfig::default(),
            server_name: None,
            file_extensions: Vec::new(),
            stderr_log: None,
        }
    }

    /// Builds the command line expected by a supported language-server binary.
    ///
    /// Most servers enter LSP mode directly. Pyrefly exposes it as the
    /// `pyrefly lsp` subcommand, so callers should use this constructor when
    /// the configured value is a server executable rather than a raw command.
    pub fn for_server(program: impl Into<OsString>, workspace_root: impl Into<PathBuf>) -> Self {
        let program = program.into();
        let mut config = Self::new(program.clone(), workspace_root);
        config.file_extensions = file_extensions(&program.to_string_lossy())
            .iter()
            .map(|extension| (*extension).to_owned())
            .collect();
        apply_default_args(&mut config.args, server_profile_from_program(&program));
        config
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        let profile = self.server_name.as_deref().map_or_else(
            || server_profile_from_program(&self.program),
            server_profile_from_name,
        );
        append_configured_args(&mut self.args, [arg.into()], profile);
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let profile = self.server_name.as_deref().map_or_else(
            || server_profile_from_program(&self.program),
            server_profile_from_name,
        );
        append_configured_args(&mut self.args, args.into_iter().map(Into::into), profile);
        self
    }

    pub fn filters(mut self, filters: FilterConfig) -> Self {
        self.filters = filters;
        self
    }

    pub fn server_name(mut self, server_name: impl Into<String>) -> Self {
        let server_name = server_name.into();
        apply_default_args(&mut self.args, server_profile_from_name(&server_name));
        self.file_extensions = file_extensions(&server_name)
            .iter()
            .map(|extension| (*extension).to_owned())
            .collect();
        self.server_name = Some(server_name);
        self
    }

    pub fn stderr_log(mut self, path: impl Into<PathBuf>) -> Self {
        self.stderr_log = Some(path.into());
        self
    }

    pub fn file_extensions<I, S>(mut self, extensions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.file_extensions = extensions.into_iter().map(Into::into).collect();
        self
    }
}
