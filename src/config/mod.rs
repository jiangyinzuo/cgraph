#![doc = include_str!("README.md")]

use std::{fs, io::ErrorKind, path::Path};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

pub const PROJECT_CONFIG_FILE: &str = ".ctree.toml";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SymbolFilter {
    patterns: Vec<String>,
}

impl SymbolFilter {
    pub fn from_patterns<I, S>(patterns: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let patterns = patterns.into_iter();
        let mut normalized = Vec::with_capacity(patterns.size_hint().0);
        for pattern in patterns {
            let pattern = pattern.into();
            let pattern = pattern.trim();
            if pattern.is_empty() {
                bail!("symbol filter contains an empty pattern");
            }
            if !normalized.iter().any(|existing| existing == pattern) {
                normalized.push(pattern.to_owned());
            }
        }
        Ok(Self {
            patterns: normalized,
        })
    }

    pub fn is_ignored(&self, symbol_name: &str) -> bool {
        self.patterns
            .iter()
            .any(|pattern| wildcard_matches(pattern, symbol_name))
    }
}

fn wildcard_matches(pattern: &str, candidate: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let candidate = candidate.chars().collect::<Vec<_>>();
    let mut previous = vec![false; candidate.len() + 1];
    previous[0] = true;
    for pattern_character in pattern {
        let mut current = vec![false; candidate.len() + 1];
        if pattern_character == '*' {
            current[0] = previous[0];
            for index in 1..=candidate.len() {
                current[index] = previous[index] || current[index - 1];
            }
        } else {
            for index in 1..=candidate.len() {
                current[index] = previous[index - 1] && candidate[index - 1] == pattern_character;
            }
        }
        previous = current;
    }
    previous[candidate.len()]
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectConfig {
    pub symbol_filter: SymbolFilter,
}

impl ProjectConfig {
    pub fn load(workspace_root: &Path) -> Result<Self> {
        let path = workspace_root.join(PROJECT_CONFIG_FILE);
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
        Ok(Self {
            symbol_filter: SymbolFilter::from_patterns(raw.filters.symbols)
                .with_context(|| format!("{} contains invalid filters.symbols", path.display()))?,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawProjectConfig {
    filters: RawFilters,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawFilters {
    symbols: Vec<String>,
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{ProjectConfig, SymbolFilter};

    #[test]
    fn loads_and_normalizes_project_local_symbol_filters() {
        let workspace = temporary_workspace("load");
        assert_eq!(ProjectConfig::load(&workspace).unwrap(), Default::default());
        fs::write(
            workspace.join(".ctree.toml"),
            "[filters]\nsymbols = [\"*::into\", \" Option::is_some \", \"*::into\", \"*::Some\"]\n",
        )
        .unwrap();

        let config = ProjectConfig::load(&workspace).unwrap();

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
            workspace.join(".ctree.toml"),
            "[filters]\nsymbols = [\"  \"]\n",
        )
        .unwrap();

        let error = ProjectConfig::load(&workspace).unwrap_err();

        assert!(format!("{error:#}").contains("empty pattern"));
        fs::write(
            workspace.join(".ctree.toml"),
            "[filters]\nsymbols = []\nunknown = true\n",
        )
        .unwrap();
        let error = ProjectConfig::load(&workspace).unwrap_err();
        assert!(format!("{error:#}").contains("unknown field"));
        fs::remove_dir_all(workspace).unwrap();
    }

    fn temporary_workspace(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("ctree-config-{name}-{unique}"));
        fs::create_dir(&workspace).unwrap();
        workspace
    }
}
