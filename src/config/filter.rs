//! Ordered project-local filtering rules.
//!
//! Rules intentionally live outside the TOML loader so the same semantics can
//! be used by the application and by LSP providers. `FilterConfig` keeps one
//! ordered `Vec<FilterRule>`; each rule is either `FilePath` or `Symbol`.
//! A rule excludes a match by default; a leading `!` includes it again. Rules are evaluated in their
//! written order, so the last matching rule wins, just like `.gitignore`.

use std::{fmt, path::Path};

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

    fn pattern(&self) -> &FilterPattern {
        match self {
            Self::FilePath { pattern, .. } | Self::Symbol { pattern, .. } => pattern,
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct FilterRules {
    rules: Vec<FilterRule>,
    kind: MatchKind,
}

impl FilterRules {
    fn from_patterns<I, S>(patterns: I, kind: MatchKind) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut rules = Vec::new();
        for raw in patterns {
            let raw = raw.into();
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
            rules.push(match kind {
                MatchKind::Symbol => FilterRule::Symbol { action, pattern },
                MatchKind::Path => FilterRule::FilePath { action, pattern },
            });
        }
        Ok(Self { rules, kind })
    }

    fn matches(&self, rule: &FilterRule, candidate: &str, outside_workspace: bool) -> bool {
        pattern_matches(rule.pattern(), candidate, self.kind, outside_workspace)
    }

    fn is_ignored(&self, candidate: &str, outside_workspace: bool, mut included: bool) -> bool {
        for rule in &self.rules {
            if self.matches(rule, candidate, outside_workspace) {
                included = rule.action() == FilterAction::Include;
            }
        }
        !included
    }
}

/// Ordered filters for normalized, provider-generated display names.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolFilter {
    rules: FilterRules,
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
    pub fn from_patterns<I, S>(rules: I, workspace_only: bool) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::from_rules(rules, workspace_only)
    }

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
                parsed.extend(FilterRules::from_patterns([name], MatchKind::Symbol)?.rules);
            } else {
                parsed.extend(
                    FilterRules::from_patterns([trimmed.to_owned()], MatchKind::Path)?.rules,
                );
            }
        }
        Ok(Self {
            rules: parsed,
            workspace_only,
        })
    }

    pub fn symbol_filter(&self) -> SymbolFilter {
        SymbolFilter {
            rules: FilterRules {
                rules: self
                    .rules
                    .iter()
                    .filter(|rule| matches!(rule, FilterRule::Symbol { .. }))
                    .cloned()
                    .collect(),
                kind: MatchKind::Symbol,
            },
        }
    }

    pub fn workspace_only(&self) -> bool {
        self.workspace_only
    }

    pub fn with_workspace_only(mut self, workspace_only: bool) -> Self {
        self.workspace_only = workspace_only;
        self
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
    /// This is intentionally not implemented by combining `SymbolFilter` and
    /// `PathFilter`: a later `!#symbol` must be able to re-include an item that
    /// an earlier file-path rule excluded.
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

    pub fn path_filter(&self) -> PathFilter {
        PathFilter {
            rules: FilterRules {
                rules: self
                    .rules
                    .iter()
                    .filter(|rule| matches!(rule, FilterRule::FilePath { .. }))
                    .cloned()
                    .collect(),
                kind: MatchKind::Path,
            },
            workspace_only: self.workspace_only,
        }
    }
}

impl Default for SymbolFilter {
    fn default() -> Self {
        Self {
            rules: FilterRules {
                rules: Vec::new(),
                kind: MatchKind::Symbol,
            },
        }
    }
}

impl SymbolFilter {
    pub fn from_patterns<I, S>(patterns: I) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Ok(Self {
            rules: FilterRules::from_patterns(patterns, MatchKind::Symbol)?,
        })
    }

    /// Returns true when the ordered rules exclude `symbol_name`.
    pub fn is_ignored(&self, symbol_name: &str) -> bool {
        self.rules.is_ignored(symbol_name, false, true)
    }
}

/// Ordered filters for workspace-relative file paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathFilter {
    rules: FilterRules,
    workspace_only: bool,
}

impl Default for PathFilter {
    fn default() -> Self {
        Self {
            rules: FilterRules {
                rules: Vec::new(),
                kind: MatchKind::Path,
            },
            workspace_only: true,
        }
    }
}

impl PathFilter {
    pub fn from_patterns<I, S>(patterns: I, workspace_only: bool) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Ok(Self {
            rules: FilterRules::from_patterns(patterns, MatchKind::Path)?,
            workspace_only,
        })
    }

    pub fn workspace_only(&self) -> bool {
        self.workspace_only
    }

    pub fn with_workspace_only(mut self, workspace_only: bool) -> Self {
        self.workspace_only = workspace_only;
        self
    }

    /// Returns true when a path is excluded by scope or by an ordered rule.
    pub fn is_ignored(&self, path: &Path, workspace_root: &Path) -> bool {
        let path = absolute_for_matching(path);
        let workspace_root = absolute_for_matching(workspace_root);
        let normalized_path = normalize_path(&path);
        let inside_workspace = path.starts_with(&workspace_root);
        let relative = path
            .strip_prefix(&workspace_root)
            .map(normalize_path)
            .unwrap_or_else(|_| normalized_path.clone());
        let mut included = !self.workspace_only || inside_workspace;
        for rule in &self.rules.rules {
            let matches = self.rules.matches(rule, &relative, !inside_workspace)
                || (relative != normalized_path
                    && self
                        .rules
                        .matches(rule, &normalized_path, !inside_workspace));
            if matches {
                included = rule.action() == FilterAction::Include;
            }
        }
        !included
    }

    pub fn is_visible(&self, path: &Path, workspace_root: &Path) -> bool {
        !self.is_ignored(path, workspace_root)
    }
}

impl fmt::Display for PathFilter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("path filter")
    }
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

    use super::{FILTER_ALL, FILTER_WORKSPACE, FilterConfig, PathFilter, SymbolFilter};

    #[test]
    fn one_rule_list_uses_hash_to_select_symbol_namespace() {
        let filters =
            FilterConfig::from_rules(["#*::into", "!#Option::into", "**/generated/**"], true)
                .unwrap();
        assert!(filters.symbol_filter().is_ignored("Vec::into"));
        assert!(!filters.symbol_filter().is_ignored("Option::into"));
        assert!(filters.path_filter().is_ignored(
            Path::new("/workspace/generated/a.rs"),
            Path::new("/workspace")
        ));
    }

    #[test]
    fn later_rules_override_earlier_rules_and_bang_reincludes() {
        let filter = SymbolFilter::from_patterns(["*", "!main", "main::*"]).unwrap();
        assert!(!filter.is_ignored("main"));
        assert!(filter.is_ignored("main::run"));
        assert!(filter.is_ignored("other"));
    }

    #[test]
    fn supports_all_placeholder_and_validates_workspace_placeholder_scope() {
        let filter = SymbolFilter::from_patterns([FILTER_ALL, "!main"]).unwrap();
        assert!(!filter.is_ignored("main"));
        assert!(filter.is_ignored("other"));
        assert!(SymbolFilter::from_patterns([FILTER_WORKSPACE]).is_err());
    }

    #[test]
    fn path_star_does_not_cross_directories_but_double_star_does() {
        let root = Path::new("/workspace");
        let filter = PathFilter::from_patterns(["src/*.rs"], false).unwrap();
        assert!(filter.is_ignored(&root.join("src/main.rs"), root));
        assert!(!filter.is_ignored(&root.join("src/nested/main.rs"), root));

        let filter = PathFilter::from_patterns(["src/**/main.rs"], false).unwrap();
        assert!(filter.is_ignored(&root.join("src/nested/main.rs"), root));

        let filter = PathFilter::from_patterns(["**/*.rs"], false).unwrap();
        assert!(filter.is_ignored(&root.join("main.rs"), root));
    }

    #[test]
    fn workspace_placeholder_excludes_external_paths_and_can_be_reincluded() {
        let root = Path::new("/workspace");
        let filter = PathFilter::from_patterns([FILTER_WORKSPACE], true).unwrap();
        assert!(!filter.is_ignored(root.join("src/main.rs").as_path(), root));
        assert!(filter.is_ignored(Path::new("/usr/include/stdio.h"), root));

        let filter = PathFilter::from_patterns([FILTER_WORKSPACE, "!**/stdio.h"], true).unwrap();
        assert!(!filter.is_ignored(Path::new("/usr/include/stdio.h"), root));
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
