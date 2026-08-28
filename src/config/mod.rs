#![doc = include_str!("README.md")]

use std::{
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const PROJECT_CONFIG_FILE: &str = ".cgraph.toml";
pub mod filter;

pub use filter::{
    FILTER_ALL, FILTER_WORKSPACE, FilterAction, FilterConfig, FilterPattern, FilterRule,
    PathFilter, SymbolFilter,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectConfig {
    pub filters: FilterConfig,
    pub symbol_filter: SymbolFilter,
    pub path_filter: PathFilter,
    pub workspace_only: bool,
    pub lsp: Option<LspSettings>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            filters: FilterConfig::default(),
            symbol_filter: SymbolFilter::default(),
            path_filter: PathFilter::default(),
            workspace_only: true,
            lsp: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default = "LspSettings::empty", deny_unknown_fields)]
pub struct LspSettings {
    #[serde(default = "missing_name", deserialize_with = "deserialize_name")]
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub file_extensions: Option<Vec<String>>,
}

impl LspSettings {
    fn empty() -> Self {
        Self {
            name: String::new(),
            command: String::new(),
            args: Vec::new(),
            file_extensions: None,
        }
    }

    fn template() -> Self {
        Self::default()
    }

    fn normalize(mut self) -> Result<Self> {
        self.command = self.command.trim().to_owned();
        if self.command.is_empty() {
            bail!("lsp.command must not be empty");
        }
        self.name = if self.name == missing_name() {
            Path::new(&self.command)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&self.command)
                .trim_end_matches(".exe")
                .to_owned()
        } else {
            self.name.trim().to_owned()
        };
        if self.args.iter().any(String::is_empty) {
            bail!("lsp.args must not contain empty arguments");
        }
        self.file_extensions = self
            .file_extensions
            .take()
            .map(normalize_file_extensions)
            .transpose()?;
        Ok(self)
    }
}

impl Default for LspSettings {
    fn default() -> Self {
        Self {
            name: "rust-analyzer".to_owned(),
            command: "rust-analyzer".to_owned(),
            args: Vec::new(),
            file_extensions: Some(vec!["rs".to_owned()]),
        }
    }
}

fn missing_name() -> String {
    "__cgraph_missing_name__".to_owned()
}

fn deserialize_name<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let name = String::deserialize(deserializer)?;
    if name.trim().is_empty() {
        return Err(serde::de::Error::custom("lsp.name must not be empty"));
    }
    Ok(name)
}

fn project_config_template() -> String {
    let lsp = toml::to_string(&LspSettings::template())
        .expect("default LSP settings must serialize to TOML");
    let commented_lsp = lsp
        .lines()
        .map(|line| format!("# {line}\n"))
        .collect::<String>();
    format!(
        "# Optional language-server command.\n# When omitted, cgraph selects rust-analyzer, clangd or pyrefly by project markers.\n#[lsp]\n# name identifies the server profile; command is the executable to run.\n{commented_lsp}[filters]\n# Rules are ordered; prefix symbol rules with # and re-include with !.\nworkspace_only = true\nrules = []\n"
    )
}

impl ProjectConfig {
    pub fn path(workspace_root: &Path) -> PathBuf {
        workspace_root.join(PROJECT_CONFIG_FILE)
    }

    pub fn create_if_missing(workspace_root: &Path) -> Result<PathBuf> {
        let path = Self::path(workspace_root);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => file
                .write_all(project_config_template().as_bytes())
                .with_context(|| {
                    format!("failed to initialize project config {}", path.display())
                })?,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to create project config {}", path.display())
                });
            }
        }
        Ok(path)
    }

    pub fn load(workspace_root: &Path) -> Result<Self> {
        let path = Self::path(workspace_root);
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to read project config {}", path.display()));
            }
        };
        let raw = toml::from_str::<RawProjectConfig>(&contents)
            .with_context(|| format!("failed to parse project config {}", path.display()))?;
        let filter_config = FilterConfig::from_rules(
            raw.filters.rules.iter().cloned(),
            raw.filters.workspace_only,
        )
        .with_context(|| format!("{} contains invalid filters.rules", path.display()))?;
        Ok(Self {
            filters: filter_config.clone(),
            symbol_filter: filter_config.symbol_filter(),
            path_filter: filter_config.path_filter(),
            workspace_only: raw.filters.workspace_only,
            lsp: raw
                .lsp
                .map(LspSettings::normalize)
                .transpose()
                .with_context(|| {
                    format!("{} contains invalid lsp configuration", path.display())
                })?,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawProjectConfig {
    lsp: Option<LspSettings>,
    filters: RawFilters,
}

fn normalize_file_extensions(extensions: Vec<String>) -> Result<Vec<String>> {
    if extensions.is_empty() {
        bail!("lsp.file_extensions must contain at least one extension");
    }

    let mut normalized = Vec::with_capacity(extensions.len());
    for extension in extensions {
        let extension = extension.trim().trim_start_matches('.').to_lowercase();
        if extension.is_empty() {
            bail!("lsp.file_extensions must not contain empty extensions");
        }
        if extension.contains(['/', '\\', '*']) || extension.contains('.') {
            bail!(
                "lsp.file_extensions entries must be plain extensions without paths or wildcards"
            );
        }
        if !normalized.contains(&extension) {
            normalized.push(extension);
        }
    }
    Ok(normalized)
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawFilters {
    rules: Vec<String>,
    workspace_only: bool,
}

impl Default for RawFilters {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            workspace_only: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{LspSettings, ProjectConfig, SymbolFilter};

    #[test]
    fn loads_and_normalizes_project_local_symbol_filters() {
        let workspace = temporary_workspace("load");
        assert_eq!(ProjectConfig::load(&workspace).unwrap(), Default::default());
        assert!(ProjectConfig::load(&workspace).unwrap().workspace_only);
        let path = ProjectConfig::create_if_missing(&workspace).unwrap();
        assert_eq!(path, workspace.join(".cgraph.toml"));
        assert_eq!(ProjectConfig::load(&workspace).unwrap(), Default::default());
        let template = fs::read_to_string(&path).unwrap();
        assert!(template.contains("# name = \"rust-analyzer\""));
        assert!(template.contains("# file_extensions = [\"rs\"]"));
        assert!(template.contains("rules = []"));
        fs::write(
            &path,
            "[lsp]\nname = \" rust-analyzer \"\ncommand = \" /usr/bin/rust-analyzer \"\nargs = [\"--log-file=/tmp/ra.log\"]\nfile_extensions = [\".RS\", \" rs \", \"RS\"]\n\n[filters]\nworkspace_only = false\nrules = [\"#*::into\", \"#Option::is_some\", \"#*::Some\"]\n",
        )
        .unwrap();
        ProjectConfig::create_if_missing(&workspace).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("*::into"));

        let config = ProjectConfig::load(&workspace).unwrap();

        assert!(!config.workspace_only);
        assert_eq!(
            config.lsp,
            Some(LspSettings {
                name: "rust-analyzer".to_owned(),
                command: "/usr/bin/rust-analyzer".to_owned(),
                args: vec!["--log-file=/tmp/ra.log".to_owned()],
                file_extensions: Some(vec!["rs".to_owned()]),
            })
        );
        fs::write(
            &path,
            "[lsp]\ncommand = \"/usr/bin/clangd\"\n\n[filters]\nworkspace_only = false\nrules = [\"#*::into\", \"#Option::is_some\", \"#*::Some\"]\n",
        )
        .unwrap();
        let config = ProjectConfig::load(&workspace).unwrap();
        assert_eq!(config.lsp.map(|lsp| lsp.name), Some("clangd".to_owned()));
        assert!(config.symbol_filter.is_ignored("Vec::into"));
        assert!(config.symbol_filter.is_ignored("Option::is_some"));
        assert!(config.symbol_filter.is_ignored("Option::Some"));
        assert!(!config.symbol_filter.is_ignored("is_some"));
        assert!(!config.symbol_filter.is_ignored("Option::some"));
        assert!(
            SymbolFilter::from_patterns(["*选*::方*"])
                .unwrap()
                .is_ignored("可选项::方法")
        );
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn rejects_invalid_or_empty_filter_entries() {
        let workspace = temporary_workspace("invalid");
        fs::write(
            workspace.join(".cgraph.toml"),
            "[filters]\nrules = [\"  \"]\n",
        )
        .unwrap();

        let error = ProjectConfig::load(&workspace).unwrap_err();

        assert!(format!("{error:#}").contains("empty pattern"));
        fs::write(
            workspace.join(".cgraph.toml"),
            "[filters]\nrules = []\nunknown = true\n",
        )
        .unwrap();
        let error = ProjectConfig::load(&workspace).unwrap_err();
        assert!(format!("{error:#}").contains("unknown field"));
        fs::write(
            workspace.join(".cgraph.toml"),
            "[lsp]\nargs = [\"--foo\"]\n",
        )
        .unwrap();
        let error = ProjectConfig::load(&workspace).unwrap_err();
        assert!(format!("{error:#}").contains("lsp.command must not be empty"));
        fs::write(
            workspace.join(".cgraph.toml"),
            "[lsp]\nname = \"  \"\ncommand = \"clangd\"\n",
        )
        .unwrap();
        let error = ProjectConfig::load(&workspace).unwrap_err();
        assert!(format!("{error:#}").contains("lsp.name must not be empty"));
        fs::write(
            workspace.join(".cgraph.toml"),
            "[lsp]\ncommand = \"clangd\"\nfile_extensions = []\n",
        )
        .unwrap();
        let error = ProjectConfig::load(&workspace).unwrap_err();
        assert!(format!("{error:#}").contains("must contain at least one extension"));
        fs::write(
            workspace.join(".cgraph.toml"),
            "[lsp]\ncommand = \"clangd\"\nfile_extensions = [\"src/*.cpp\"]\n",
        )
        .unwrap();
        let error = ProjectConfig::load(&workspace).unwrap_err();
        assert!(format!("{error:#}").contains("without paths or wildcards"));
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn loads_ordered_path_rules_from_project_config() {
        let workspace = temporary_workspace("paths");
        fs::write(
            workspace.join(".cgraph.toml"),
            "[filters]\nworkspace_only = false\nrules = [\"**/generated/**\", \"!src/generated/keep.rs\"]\n",
        )
        .unwrap();
        let config = ProjectConfig::load(&workspace).unwrap();
        assert!(
            config
                .path_filter
                .is_ignored(&workspace.join("src/generated/file.rs"), &workspace)
        );
        assert!(
            !config
                .path_filter
                .is_ignored(&workspace.join("src/generated/keep.rs"), &workspace)
        );
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn loads_workspace_scope_with_one_external_symbol_exception() {
        let workspace = temporary_workspace("workspace-exception");
        fs::write(
            workspace.join(".cgraph.toml"),
            "[filters]\nrules = [\"<workspace>\", \"!#printf\"]\n",
        )
        .unwrap();

        let config = ProjectConfig::load(&workspace).unwrap();

        assert!(config.filters.is_ignored(
            Some("malloc"),
            Some(Path::new("/usr/include/stdlib.h")),
            &workspace,
        ));
        assert!(!config.filters.is_ignored(
            Some("printf"),
            Some(Path::new("/usr/include/stdio.h")),
            &workspace,
        ));
        fs::remove_dir_all(workspace).unwrap();
    }

    fn temporary_workspace(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("cgraph-config-{name}-{unique}"));
        fs::create_dir(&workspace).unwrap();
        workspace
    }
}
