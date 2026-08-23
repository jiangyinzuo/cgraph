use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use tree_sitter::{Parser, Query, Tree};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeSitterLanguage {
    Rust,
    C,
    Cpp,
    Python,
}

impl TreeSitterLanguage {
    pub fn detect(workspace_root: &Path) -> Option<Self> {
        if workspace_root.join("Cargo.toml").is_file()
            || contains_source_with_extension(workspace_root, &["rs"])
        {
            return Some(Self::Rust);
        }
        if contains_source_with_extension(workspace_root, &["cc", "cpp", "cxx", "hpp"]) {
            return Some(Self::Cpp);
        }
        if workspace_root.join("compile_commands.json").is_file()
            || workspace_root.join("CMakeLists.txt").is_file()
            || contains_source_with_extension(workspace_root, &["c", "h"])
        {
            return Some(Self::C);
        }
        if workspace_root.join("pyproject.toml").is_file()
            || workspace_root.join("setup.py").is_file()
            || workspace_root.join("requirements.txt").is_file()
            || contains_source_with_extension(workspace_root, &["py"])
        {
            return Some(Self::Python);
        }

        None
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::C => "C",
            Self::Cpp => "C++",
            Self::Python => "Python",
        }
    }

    fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::C => tree_sitter_c::LANGUAGE.into(),
            Self::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
        }
    }

    fn tags_query(self) -> &'static str {
        match self {
            Self::Rust => tree_sitter_rust::TAGS_QUERY,
            Self::C => tree_sitter_c::TAGS_QUERY,
            Self::Cpp => tree_sitter_cpp::TAGS_QUERY,
            Self::Python => tree_sitter_python::TAGS_QUERY,
        }
    }
}

/// Initialized syntax parser used when no LSP session is available.
///
/// Grammar/query readiness is deliberately narrower than workspace-search
/// readiness: tags queries identify syntax captures in one parsed file, while
/// directory traversal, candidate normalization and hierarchy confidence still
/// need explicit provider semantics.
pub struct TreeSitterProvider {
    workspace_root: PathBuf,
    language: TreeSitterLanguage,
    parser: Parser,
    symbol_query: Query,
}

impl std::fmt::Debug for TreeSitterProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TreeSitterProvider")
            .field("workspace_root", &self.workspace_root)
            .field("language", &self.language)
            .finish_non_exhaustive()
    }
}

impl TreeSitterProvider {
    pub fn start(workspace_root: impl Into<PathBuf>, language: TreeSitterLanguage) -> Result<Self> {
        let workspace_root = workspace_root.into();
        let grammar = language.grammar();
        let mut parser = Parser::new();
        parser
            .set_language(&grammar)
            .with_context(|| format!("failed to initialize {} grammar", language.name()))?;
        let symbol_query = Query::new(&grammar, language.tags_query())
            .with_context(|| format!("failed to initialize {} symbol query", language.name()))?;

        Ok(Self {
            workspace_root,
            language,
            parser,
            symbol_query,
        })
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn language(&self) -> TreeSitterLanguage {
        self.language
    }

    pub fn parse(&mut self, source: &str) -> Result<Tree> {
        self.parser
            .parse(source, None)
            .with_context(|| format!("{} parser returned no syntax tree", self.language.name()))
    }

    pub fn symbol_capture_names(&self) -> &[&str] {
        self.symbol_query.capture_names()
    }
}

fn contains_source_with_extension(workspace_root: &Path, extensions: &[&str]) -> bool {
    fs::read_dir(workspace_root).is_ok_and(|entries| {
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

    use super::{TreeSitterLanguage, TreeSitterProvider};

    #[test]
    fn detects_supported_workspace_languages() {
        let workspace = temporary_workspace("detect");
        fs::write(workspace.join("main.cpp"), "int main() {}\n").unwrap();

        assert_eq!(
            TreeSitterLanguage::detect(&workspace),
            Some(TreeSitterLanguage::Cpp)
        );

        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn initializes_and_parses_each_supported_grammar() {
        let cases = [
            (TreeSitterLanguage::Rust, "fn main() {}"),
            (TreeSitterLanguage::C, "int main(void) { return 0; }"),
            (TreeSitterLanguage::Cpp, "int main() { return 0; }"),
            (TreeSitterLanguage::Python, "def main():\n    pass\n"),
        ];

        for (language, source) in cases {
            let mut provider = TreeSitterProvider::start(".", language).unwrap();
            let tree = provider.parse(source).unwrap();
            assert!(
                !tree.root_node().has_error(),
                "{} parse failed",
                language.name()
            );
            assert!(!provider.symbol_capture_names().is_empty());
        }
    }

    fn temporary_workspace(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ctree-{name}-{unique}"));
        fs::create_dir(&path).unwrap();
        path
    }
}
