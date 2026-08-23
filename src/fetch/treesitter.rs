use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Context, Result, anyhow};
use tokio::{sync::watch, task};
use tree_sitter::{Parser, Query, Tree};

use crate::fetch::{HierarchyQuery, HierarchyResponse, WorkspaceSymbolMatch};

mod index;

use index::ProjectIndex;

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

    pub(super) fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::C => tree_sitter_c::LANGUAGE.into(),
            Self::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
        }
    }

    pub(super) fn tags_query(self) -> &'static str {
        match self {
            Self::Rust => tree_sitter_rust::TAGS_QUERY,
            Self::C => tree_sitter_c::TAGS_QUERY,
            Self::Cpp => tree_sitter_cpp::TAGS_QUERY,
            Self::Python => tree_sitter_python::TAGS_QUERY,
        }
    }

    pub(super) fn call_query(self) -> &'static str {
        match self {
            Self::Rust | Self::Python => self.tags_query(),
            Self::C | Self::Cpp => {
                r#"
                (call_expression
                    function: (identifier) @name) @reference.call
                (call_expression
                    function: (field_expression
                        field: (field_identifier) @name)) @reference.call
                "#
            }
        }
    }

    pub(super) fn accepts_path(self, path: &Path) -> bool {
        let extension = path.extension().and_then(|extension| extension.to_str());
        match self {
            Self::Rust => extension == Some("rs"),
            Self::C => matches!(extension, Some("c" | "h")),
            Self::Cpp => matches!(extension, Some("cc" | "cpp" | "cxx" | "h" | "hpp")),
            Self::Python => extension == Some("py"),
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
    shared: Arc<SharedIndex>,
}

struct SharedIndex {
    workspace_root: PathBuf,
    language: TreeSitterLanguage,
    state: Mutex<IndexState>,
    next_build_id: AtomicU64,
    #[cfg(test)]
    build_count: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    pause_build: std::sync::atomic::AtomicBool,
}

type IndexBuildResult = std::result::Result<Arc<ProjectIndex>, Arc<str>>;

// The build state lives outside individual query futures. Search debounce may
// abort a waiter, but must not abort or duplicate the project-wide scan.
enum IndexState {
    Empty,
    Building {
        build_id: u64,
        receiver: watch::Receiver<Option<IndexBuildResult>>,
    },
    Ready(Arc<ProjectIndex>),
}

#[derive(Clone)]
pub struct WorkspaceSymbolClient {
    shared: Arc<SharedIndex>,
}

#[derive(Clone)]
pub struct HierarchyClient {
    shared: Arc<SharedIndex>,
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
            shared: Arc::new(SharedIndex {
                workspace_root: workspace_root.clone(),
                language,
                state: Mutex::new(IndexState::Empty),
                next_build_id: AtomicU64::new(1),
                #[cfg(test)]
                build_count: std::sync::atomic::AtomicUsize::new(0),
                #[cfg(test)]
                pause_build: std::sync::atomic::AtomicBool::new(false),
            }),
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

    pub fn workspace_symbol_client(&self) -> WorkspaceSymbolClient {
        WorkspaceSymbolClient {
            shared: Arc::clone(&self.shared),
        }
    }

    pub fn hierarchy_client(&self) -> HierarchyClient {
        HierarchyClient {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl std::fmt::Debug for WorkspaceSymbolClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TreeSitterWorkspaceSymbolClient")
            .finish_non_exhaustive()
    }
}

impl WorkspaceSymbolClient {
    pub async fn query(&self, _query: &str) -> Result<Vec<WorkspaceSymbolMatch>> {
        Ok(load_index(&self.shared).await?.workspace_symbols())
    }
}

impl std::fmt::Debug for HierarchyClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TreeSitterHierarchyClient")
            .finish_non_exhaustive()
    }
}

impl HierarchyClient {
    pub async fn query(&self, query: HierarchyQuery) -> Result<HierarchyResponse> {
        load_index(&self.shared).await?.hierarchy(query)
    }
}

async fn load_index(shared: &Arc<SharedIndex>) -> Result<Arc<ProjectIndex>> {
    let (receiver, build) = {
        let mut state = shared
            .state
            .lock()
            .expect("Tree-sitter index state mutex poisoned");
        match &*state {
            IndexState::Ready(index) => return Ok(Arc::clone(index)),
            IndexState::Building { receiver, .. } => (receiver.clone(), None),
            IndexState::Empty => {
                let build_id = shared.next_build_id.fetch_add(1, Ordering::Relaxed);
                let (sender, receiver) = watch::channel(None);
                *state = IndexState::Building {
                    build_id,
                    receiver: receiver.clone(),
                };
                (receiver, Some((build_id, sender)))
            }
        }
    };

    if let Some((build_id, sender)) = build {
        spawn_index_build(Arc::clone(shared), build_id, sender);
    }

    wait_for_index(receiver).await
}

fn spawn_index_build(
    shared: Arc<SharedIndex>,
    build_id: u64,
    sender: watch::Sender<Option<IndexBuildResult>>,
) {
    let _build_task = tokio::spawn(async move {
        let workspace_root = shared.workspace_root.clone();
        let language = shared.language;
        #[cfg(test)]
        let build_shared = Arc::clone(&shared);
        let result = task::spawn_blocking(move || {
            #[cfg(test)]
            {
                use std::{thread, time::Duration};

                build_shared.build_count.fetch_add(1, Ordering::SeqCst);
                while build_shared.pause_build.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(1));
                }
            }
            ProjectIndex::build(&workspace_root, language)
        })
        .await
        .context("Tree-sitter indexing task failed")
        .and_then(|result| result)
        .map(Arc::new)
        .map_err(|error| Arc::<str>::from(format!("{error:#}")));

        {
            let mut state = shared
                .state
                .lock()
                .expect("Tree-sitter index state mutex poisoned");
            if matches!(
                &*state,
                IndexState::Building {
                    build_id: active_build_id,
                    ..
                } if *active_build_id == build_id
            ) {
                *state = match &result {
                    Ok(index) => IndexState::Ready(Arc::clone(index)),
                    Err(_) => IndexState::Empty,
                };
            }
        }

        sender.send_replace(Some(result));
    });
}

async fn wait_for_index(
    mut receiver: watch::Receiver<Option<IndexBuildResult>>,
) -> Result<Arc<ProjectIndex>> {
    loop {
        if let Some(result) = receiver.borrow().clone() {
            return result.map_err(|error| anyhow!(error.to_string()));
        }
        receiver
            .changed()
            .await
            .context("Tree-sitter indexing task ended without a result")?;
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
        sync::atomic::Ordering,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{TreeSitterLanguage, TreeSitterProvider};
    use crate::{
        fetch::{FetchSource, HierarchyQuery},
        state::{HierarchyDirection, HierarchyKind, SourceLocation, SymbolIdentity},
    };

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

    #[test]
    fn normalizes_tree_sitter_byte_columns_to_utf16() {
        assert_eq!(super::index::utf16_column("é😀name", 6, 6).unwrap(), 3);
    }

    #[tokio::test]
    async fn indexes_rust_symbols_and_bidirectional_static_calls_once() {
        let workspace = temporary_workspace("rust-index");
        fs::write(
            workspace.join("lib.rs"),
            r#"
            struct Worker;
            trait Job {
                fn execute(&self) {}
            }
            impl Worker {
                fn run(&self) {
                    helper();
                    self.finish();
                }
                fn finish(&self) {}
            }
            fn helper() {}
            "#,
        )
        .unwrap();
        fs::create_dir(workspace.join("target")).unwrap();
        fs::write(workspace.join("target/ignored.rs"), "fn ignored() {}\n").unwrap();
        let provider = TreeSitterProvider::start(&workspace, TreeSitterLanguage::Rust).unwrap();
        let symbol_client = provider.workspace_symbol_client();
        let hierarchy_client = provider.hierarchy_client();

        let symbols = symbol_client.query("").await.unwrap();
        let names = symbols
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"Worker::run"));
        assert!(names.contains(&"Worker::finish"));
        assert!(names.contains(&"Job::execute"));
        assert!(names.contains(&"helper"));
        assert!(!names.contains(&"ignored"));

        let run = identity(&symbols, "Worker::run", HierarchyKind::Call);
        let outgoing = hierarchy_client
            .query(HierarchyQuery {
                symbol: run.clone(),
                direction: HierarchyDirection::Outgoing,
            })
            .await
            .unwrap();
        assert_eq!(outgoing.source, FetchSource::TreeSitter);
        assert_eq!(
            outgoing
                .children
                .iter()
                .map(|child| child.symbol.as_str())
                .collect::<Vec<_>>(),
            ["helper", "Worker::finish"]
        );

        let helper = identity(&symbols, "helper", HierarchyKind::Call);
        let incoming = hierarchy_client
            .query(HierarchyQuery {
                symbol: helper,
                direction: HierarchyDirection::Incoming,
            })
            .await
            .unwrap();
        assert_eq!(incoming.children, [run]);
        fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn indexes_python_methods_calls_and_type_inheritance() {
        let workspace = temporary_workspace("python-index");
        fs::write(
            workspace.join("main.py"),
            "class Base:\n    pass\n\nclass Child(Base):\n    def run(self):\n        helper()\n\ndef helper():\n    pass\n",
        )
        .unwrap();
        let provider = TreeSitterProvider::start(&workspace, TreeSitterLanguage::Python).unwrap();
        let symbols = provider.workspace_symbol_client().query("").await.unwrap();
        let hierarchy = provider.hierarchy_client();

        let run = identity(&symbols, "Child.run", HierarchyKind::Call);
        let outgoing = hierarchy
            .query(HierarchyQuery {
                symbol: run,
                direction: HierarchyDirection::Outgoing,
            })
            .await
            .unwrap();
        assert_eq!(
            outgoing
                .children
                .iter()
                .map(|child| child.symbol.as_str())
                .collect::<Vec<_>>(),
            ["helper"]
        );

        let base = identity(&symbols, "Base", HierarchyKind::Type);
        let child = identity(&symbols, "Child", HierarchyKind::Type);
        let subtypes = hierarchy
            .query(HierarchyQuery {
                symbol: base.clone(),
                direction: HierarchyDirection::Outgoing,
            })
            .await
            .unwrap();
        assert_eq!(subtypes.children.as_slice(), std::slice::from_ref(&child));
        let supertypes = hierarchy
            .query(HierarchyQuery {
                symbol: child,
                direction: HierarchyDirection::Incoming,
            })
            .await
            .unwrap();
        assert_eq!(supertypes.children, [base]);
        fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn python_self_calls_prefer_the_method_on_the_current_class() {
        let workspace = temporary_workspace("python-self-call");
        fs::write(
            workspace.join("main.py"),
            "class First:\n    def finish(self):\n        pass\n\nclass Second:\n    def run(self):\n        self.finish()\n\n    def finish(self):\n        pass\n",
        )
        .unwrap();
        let provider = TreeSitterProvider::start(&workspace, TreeSitterLanguage::Python).unwrap();
        let symbols = provider.workspace_symbol_client().query("").await.unwrap();
        let run = identity(&symbols, "Second.run", HierarchyKind::Call);

        let outgoing = provider
            .hierarchy_client()
            .query(HierarchyQuery {
                symbol: run,
                direction: HierarchyDirection::Outgoing,
            })
            .await
            .unwrap();

        assert_eq!(
            outgoing
                .children
                .iter()
                .map(|child| child.symbol.as_str())
                .collect::<Vec<_>>(),
            ["Second.finish"]
        );
        fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn indexes_c_and_cpp_calls_plus_cpp_inheritance() {
        let c_workspace = temporary_workspace("c-index");
        fs::write(
            c_workspace.join("main.c"),
            "void helper(void);\nvoid helper(void) {}\nvoid run(void) { helper(); }\n",
        )
        .unwrap();
        let c_provider = TreeSitterProvider::start(&c_workspace, TreeSitterLanguage::C).unwrap();
        assert_call_edge(&c_provider, "run", "helper").await;

        let cpp_workspace = temporary_workspace("cpp-index");
        fs::write(
            cpp_workspace.join("main.cpp"),
            "class Base {};\nclass Child : public Base { public: void run(); };\nvoid helper() {}\nvoid Child::run() { helper(); }\n",
        )
        .unwrap();
        let cpp_provider =
            TreeSitterProvider::start(&cpp_workspace, TreeSitterLanguage::Cpp).unwrap();
        assert_call_edge(&cpp_provider, "Child::run", "helper").await;
        let symbols = cpp_provider
            .workspace_symbol_client()
            .query("")
            .await
            .unwrap();
        let base = identity(&symbols, "Base", HierarchyKind::Type);
        let child = identity(&symbols, "Child", HierarchyKind::Type);
        let response = cpp_provider
            .hierarchy_client()
            .query(HierarchyQuery {
                symbol: base,
                direction: HierarchyDirection::Outgoing,
            })
            .await
            .unwrap();
        assert_eq!(response.children, [child]);

        fs::remove_dir_all(c_workspace).unwrap();
        fs::remove_dir_all(cpp_workspace).unwrap();
    }

    #[tokio::test]
    async fn leaves_ambiguous_targets_unbound_and_requires_an_exact_root() {
        let workspace = temporary_workspace("ambiguous-index");
        fs::write(workspace.join("first.rs"), "pub fn helper() {}\n").unwrap();
        fs::write(workspace.join("second.rs"), "pub fn helper() {}\n").unwrap();
        fs::write(workspace.join("main.rs"), "fn run() { helper(); }\n").unwrap();
        let provider = TreeSitterProvider::start(&workspace, TreeSitterLanguage::Rust).unwrap();
        let symbols = provider.workspace_symbol_client().query("").await.unwrap();
        let run = identity(&symbols, "run", HierarchyKind::Call);

        let response = provider
            .hierarchy_client()
            .query(HierarchyQuery {
                symbol: run,
                direction: HierarchyDirection::Outgoing,
            })
            .await
            .unwrap();
        assert!(response.children.is_empty());

        let error = provider
            .hierarchy_client()
            .query(HierarchyQuery {
                symbol: SymbolIdentity {
                    symbol: "helper".to_owned(),
                    kind: HierarchyKind::Call,
                    location: None,
                },
                direction: HierarchyDirection::Incoming,
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("ambiguous"));
        fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn cancelling_the_first_query_does_not_restart_project_indexing() {
        let workspace = temporary_workspace("cancelled-index-query");
        fs::write(workspace.join("lib.rs"), "fn main() {}\n").unwrap();
        let provider = TreeSitterProvider::start(&workspace, TreeSitterLanguage::Rust).unwrap();
        provider.shared.pause_build.store(true, Ordering::SeqCst);
        let first_client = provider.workspace_symbol_client();
        let first_query = tokio::spawn(async move { first_client.query("").await });

        let mut build_started = false;
        for _ in 0..10_000 {
            if provider.shared.build_count.load(Ordering::SeqCst) == 1 {
                build_started = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        if !build_started {
            provider.shared.pause_build.store(false, Ordering::SeqCst);
            panic!("background index build did not start");
        }

        first_query.abort();
        assert!(first_query.await.unwrap_err().is_cancelled());
        provider.shared.pause_build.store(false, Ordering::SeqCst);
        let symbols = provider.workspace_symbol_client().query("").await.unwrap();

        assert!(symbols.iter().any(|symbol| symbol.name == "main"));
        assert_eq!(provider.shared.build_count.load(Ordering::SeqCst), 1);
        fs::remove_dir_all(workspace).unwrap();
    }

    async fn assert_call_edge(provider: &TreeSitterProvider, caller: &str, callee: &str) {
        let symbols = provider.workspace_symbol_client().query("").await.unwrap();
        let caller = identity(&symbols, caller, HierarchyKind::Call);
        let response = provider
            .hierarchy_client()
            .query(HierarchyQuery {
                symbol: caller,
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
            [callee]
        );
    }

    fn identity(
        symbols: &[crate::fetch::WorkspaceSymbolMatch],
        name: &str,
        kind: HierarchyKind,
    ) -> SymbolIdentity {
        let symbol = symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .unwrap_or_else(|| panic!("missing indexed symbol {name:?}"));
        let position = symbol.range.unwrap().start;
        SymbolIdentity {
            symbol: symbol.name.clone(),
            kind,
            location: Some(SourceLocation {
                uri: symbol.uri.to_string(),
                line: Some(position.line),
                character: Some(position.character),
            }),
        }
    }

    fn temporary_workspace(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("cgraph-{name}-{unique}"));
        fs::create_dir(&path).unwrap();
        path
    }
}
