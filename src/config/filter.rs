//! Ordered project-local filtering rules.
//!
//! Rules intentionally live outside the TOML loader so the same semantics can
//! be used by the application and by LSP providers. `FilterConfig` keeps one
//! ordered `Vec<FilterRule>`; each rule is either `FilePath` or `Symbol`.
//! A rule excludes a match by default; a leading `!` includes it again. Rules are evaluated in their
//! written order, so the last matching rule wins, just like `.gitignore`.

use std::path::Path;

/// Special pattern that matches every candidate in either filter namespace.
pub const FILTER_ALL: &str = "<all>";
/// Special path pattern that excludes paths outside the current workspace.
pub const FILTER_WORKSPACE: &str = "<workspace>";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterAction {
    Include,
    Exclude,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FilterPattern {
    All,
    Workspace,
    Glob(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FilterRule {
    FilePath {
        action: FilterAction,
        pattern: FilterPattern,
    },
    Symbol {
        action: FilterAction,
        pattern: FilterPattern,
    },
}

impl FilterRule {
    fn action(&self) -> FilterAction {
        match self {
            Self::FilePath { action, .. } | Self::Symbol { action, .. } => *action,
        }
    }

    fn matches(
        &self,
        symbol: Option<&str>,
        relative_path: Option<&str>,
        absolute_path: Option<&str>,
        outside_workspace: bool,
    ) -> bool {
        match self {
            Self::Symbol { pattern, .. } => symbol.is_some_and(|symbol| {
                pattern_matches(pattern, symbol, MatchKind::Symbol, outside_workspace)
            }),
            Self::FilePath { pattern, .. } => relative_path.is_some_and(|relative_path| {
                pattern_matches(pattern, relative_path, MatchKind::Path, outside_workspace)
                    || absolute_path.is_some_and(|absolute_path| {
                        absolute_path != relative_path
                            && pattern_matches(
                                pattern,
                                absolute_path,
                                MatchKind::Path,
                                outside_workspace,
                            )
                    })
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MatchKind {
    Symbol,
    Path,
}

/// The two namespaces parsed from one project-local rule list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilterConfig {
    pub rules: Vec<FilterRule>,
    workspace_only: bool,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            workspace_only: true,
        }
    }
}

impl FilterConfig {
    pub fn from_rules<I, S>(rules: I, workspace_only: bool) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut parsed = Vec::new();
        for raw in rules {
            let raw = raw.into();
            let trimmed = raw.trim();
            let marker = trimmed.strip_prefix('!').unwrap_or(trimmed);
            if marker.strip_prefix('#').is_some() {
                let negation = trimmed.starts_with('!');
                let name = marker.strip_prefix('#').unwrap_or_default();
                let name = if negation {
                    format!("!{name}")
                } else {
                    name.to_owned()
                };
                parsed.push(parse_filter_rule(&name, MatchKind::Symbol)?);
            } else {
                parsed.push(parse_filter_rule(trimmed, MatchKind::Path)?);
            }
        }
        Ok(Self {
            rules: parsed,
            workspace_only,
        })
    }

    pub fn workspace_only(&self) -> bool {
        self.workspace_only
    }

    pub fn is_ignored_symbol(&self, symbol: &str) -> bool {
        self.is_ignored(Some(symbol), None, Path::new("."))
    }

    pub fn is_ignored_path(&self, path: &Path, workspace_root: &Path) -> bool {
        self.is_ignored(None, Some(path), workspace_root)
    }

    pub fn is_visible_symbol_path(&self, symbol: &str, path: &Path, workspace_root: &Path) -> bool {
        !self.is_ignored(Some(symbol), Some(path), workspace_root)
    }

    /// Evaluates all matching file and symbol rules in their original order.
    ///
    /// A later symbol rule can therefore re-include an item that an earlier
    /// file-path rule excluded, and vice versa.
    pub fn is_ignored(
        &self,
        symbol: Option<&str>,
        path: Option<&Path>,
        workspace_root: &Path,
    ) -> bool {
        let path = path.map(absolute_for_matching);
        let workspace_root = absolute_for_matching(workspace_root);
        let inside_workspace = path
            .as_deref()
            .is_none_or(|path| path.starts_with(&workspace_root));
        let absolute_path = path.as_deref().map(normalize_path);
        let relative_path = path.as_deref().map(|path| {
            path.strip_prefix(&workspace_root)
                .map(normalize_path)
                .unwrap_or_else(|_| normalize_path(path))
        });
        let mut included = !self.workspace_only || inside_workspace;
        for rule in &self.rules {
            if rule.matches(
                symbol,
                relative_path.as_deref(),
                absolute_path.as_deref(),
                !inside_workspace,
            ) {
                included = rule.action() == FilterAction::Include;
            }
        }
        !included
    }
}

fn parse_filter_rule(raw: &str, kind: MatchKind) -> anyhow::Result<FilterRule> {
    let raw = raw.trim();
    if raw.is_empty() {
        anyhow::bail!("filter contains an empty pattern");
    }
    let (action, pattern) = raw
        .strip_prefix('!')
        .map_or((FilterAction::Exclude, raw), |pattern| {
            (FilterAction::Include, pattern)
        });
    if pattern.is_empty() {
        anyhow::bail!("filter contains a bare ! pattern");
    }
    let pattern = match pattern {
        FILTER_ALL => FilterPattern::All,
        FILTER_WORKSPACE if kind == MatchKind::Path => FilterPattern::Workspace,
        FILTER_WORKSPACE => {
            anyhow::bail!("{FILTER_WORKSPACE} is only valid for file-path rules")
        }
        pattern => {
            validate_glob(pattern)?;
            FilterPattern::Glob(pattern.to_owned())
        }
    };
    Ok(match kind {
        MatchKind::Symbol => FilterRule::Symbol { action, pattern },
        MatchKind::Path => FilterRule::FilePath { action, pattern },
    })
}

fn validate_glob(pattern: &str) -> anyhow::Result<()> {
    if pattern.contains('\0') {
        anyhow::bail!("filter pattern must not contain NUL");
    }
    Ok(())
}

fn pattern_matches(
    pattern: &FilterPattern,
    candidate: &str,
    kind: MatchKind,
    outside_workspace: bool,
) -> bool {
    match pattern {
        FilterPattern::All => true,
        FilterPattern::Workspace => outside_workspace,
        FilterPattern::Glob(pattern) => match kind {
            MatchKind::Symbol => wildcard_matches(pattern, candidate, true),
            MatchKind::Path => path_glob_matches(pattern, candidate),
        },
    }
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn absolute_for_matching(path: &Path) -> std::path::PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .unwrap_or_else(|_| path.to_owned())
    }
}

fn wildcard_matches(pattern: &str, candidate: &str, star_matches_separator: bool) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let candidate = candidate.chars().collect::<Vec<_>>();
    let mut previous = vec![false; candidate.len() + 1];
    previous[0] = true;
    let mut pattern_index = 0;
    while pattern_index < pattern.len() {
        let mut current = vec![false; candidate.len() + 1];
        if pattern[pattern_index] == '*' {
            let double_star =
                pattern_index + 1 < pattern.len() && pattern[pattern_index + 1] == '*';
            let matches_separator = star_matches_separator || double_star;
            current[0] = previous[0];
            for index in 1..=candidate.len() {
                current[index] = previous[index]
                    || (current[index - 1] && (matches_separator || candidate[index - 1] != '/'));
            }
            if double_star {
                pattern_index += 1;
            }
        } else {
            for index in 1..=candidate.len() {
                current[index] =
                    previous[index - 1] && candidate[index - 1] == pattern[pattern_index];
            }
        }
        previous = current;
        pattern_index += 1;
    }
    previous[candidate.len()]
}

fn path_glob_matches(pattern: &str, candidate: &str) -> bool {
    let pattern = pattern.trim_matches('/');
    let candidate = candidate.trim_matches('/');
    if !pattern.contains('/') {
        return candidate
            .split('/')
            .any(|segment| wildcard_matches(pattern, segment, false));
    }
    let pattern = pattern.split('/').collect::<Vec<_>>();
    let candidate = candidate.split('/').collect::<Vec<_>>();
    let mut previous = vec![false; candidate.len() + 1];
    previous[0] = true;
    for pattern_segment in pattern {
        let mut current = vec![false; candidate.len() + 1];
        if pattern_segment == "**" {
            current[0] = previous[0];
            for index in 1..=candidate.len() {
                current[index] = previous[index] || current[index - 1];
            }
        } else {
            for index in 1..=candidate.len() {
                current[index] = previous[index - 1]
                    && wildcard_matches(pattern_segment, candidate[index - 1], false);
            }
        }
        previous = current;
    }
    previous[candidate.len()]
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{FILTER_ALL, FILTER_WORKSPACE, FilterConfig};

    #[test]
    fn one_rule_list_uses_hash_to_select_symbol_namespace() {
        let filters =
            FilterConfig::from_rules(["#*::into", "!#Option::into", "**/generated/**"], true)
                .unwrap();
        assert!(filters.is_ignored_symbol("Vec::into"));
        assert!(!filters.is_ignored_symbol("Option::into"));
        assert!(filters.is_ignored_path(
            Path::new("/workspace/generated/a.rs"),
            Path::new("/workspace")
        ));
    }

    #[test]
    fn later_rules_override_earlier_rules_and_bang_reincludes() {
        let filters = FilterConfig::from_rules(["#*", "!#main", "#main::*"], false).unwrap();
        assert!(!filters.is_ignored_symbol("main"));
        assert!(filters.is_ignored_symbol("main::run"));
        assert!(filters.is_ignored_symbol("other"));
    }

    #[test]
    fn supports_all_placeholder_and_validates_workspace_placeholder_scope() {
        let filters =
            FilterConfig::from_rules([format!("#{FILTER_ALL}"), "!#main".to_owned()], false)
                .unwrap();
        assert!(!filters.is_ignored_symbol("main"));
        assert!(filters.is_ignored_symbol("other"));
        assert!(FilterConfig::from_rules([format!("#{FILTER_WORKSPACE}")], false).is_err());
    }

    #[test]
    fn path_star_does_not_cross_directories_but_double_star_does() {
        let root = Path::new("/workspace");
        let filters = FilterConfig::from_rules(["src/*.rs"], false).unwrap();
        assert!(filters.is_ignored_path(&root.join("src/main.rs"), root));
        assert!(!filters.is_ignored_path(&root.join("src/nested/main.rs"), root));

        let filters = FilterConfig::from_rules(["src/**/main.rs"], false).unwrap();
        assert!(filters.is_ignored_path(&root.join("src/nested/main.rs"), root));

        let filters = FilterConfig::from_rules(["**/*.rs"], false).unwrap();
        assert!(filters.is_ignored_path(&root.join("main.rs"), root));
    }

    #[test]
    fn workspace_placeholder_excludes_external_paths_and_can_be_reincluded() {
        let root = Path::new("/workspace");
        let filters = FilterConfig::from_rules([FILTER_WORKSPACE], true).unwrap();
        assert!(!filters.is_ignored_path(root.join("src/main.rs").as_path(), root));
        assert!(filters.is_ignored_path(Path::new("/usr/include/stdio.h"), root));

        let filters = FilterConfig::from_rules([FILTER_WORKSPACE, "!**/stdio.h"], true).unwrap();
        assert!(!filters.is_ignored_path(Path::new("/usr/include/stdio.h"), root));
    }

    #[test]
    fn later_symbol_rule_reincludes_one_external_workspace_symbol() {
        let root = Path::new("/workspace");
        let filters = FilterConfig::from_rules([FILTER_WORKSPACE, "!#printf"], true).unwrap();

        assert!(!filters.is_ignored(Some("main"), Some(Path::new("/workspace/main.cpp")), root,));
        assert!(filters.is_ignored(
            Some("malloc"),
            Some(Path::new("/usr/include/stdlib.h")),
            root,
        ));
        assert!(!filters.is_ignored(
            Some("printf"),
            Some(Path::new("/usr/include/stdio.h")),
            root,
        ));
    }
}
