#![doc = include_str!("README.md")]

pub mod lsp;
pub mod treesitter;

use lsp::{
    HierarchyClient as LspHierarchyClient, WorkspaceSymbolClient as LspWorkspaceSymbolClient,
};
use treesitter::{
    HierarchyClient as TreeSitterHierarchyClient,
    WorkspaceSymbolClient as TreeSitterWorkspaceSymbolClient,
};

use crate::config::FilterConfig;
use crate::state::{HierarchyDirection, SymbolIdentity};
use tower_lsp::lsp_types::{Range, SymbolKind, Url};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceSymbolMatch {
    /// Provider-normalized display name, including the language-appropriate
    /// class or implementation qualifier for methods when it is known.
    pub name: String,
    pub kind: SymbolKind,
    pub container_name: Option<String>,
    pub uri: Url,
    pub range: Option<Range>,
}

impl WorkspaceSymbolMatch {
    pub fn display_name(&self) -> String {
        self.name.clone()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FetchSource {
    Lsp,
    TreeSitter,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CachePolicy {
    #[default]
    UseCache,
    Refresh,
}

/// Backend-independent description of one lazy hierarchy expansion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HierarchyQuery {
    pub symbol: SymbolIdentity,
    pub direction: HierarchyDirection,
}

/// Normalized one-level result returned by either LSP or Tree-sitter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HierarchyResponse {
    pub query: HierarchyQuery,
    pub children: Vec<SymbolIdentity>,
    pub source: FetchSource,
}

#[derive(Clone, Debug)]
pub enum WorkspaceSymbolClient {
    Lsp(LspWorkspaceSymbolClient),
    TreeSitter(TreeSitterWorkspaceSymbolClient),
}

impl WorkspaceSymbolClient {
    pub fn set_filters(&mut self, filters: FilterConfig) {
        if let Self::Lsp(client) = self {
            client.set_filters(filters);
        }
    }

    pub async fn query(&self, query: &str) -> anyhow::Result<Vec<WorkspaceSymbolMatch>> {
        match self {
            Self::Lsp(client) => client.query(query).await,
            Self::TreeSitter(client) => client.query(query).await,
        }
    }
}

impl From<LspWorkspaceSymbolClient> for WorkspaceSymbolClient {
    fn from(client: LspWorkspaceSymbolClient) -> Self {
        Self::Lsp(client)
    }
}

impl From<TreeSitterWorkspaceSymbolClient> for WorkspaceSymbolClient {
    fn from(client: TreeSitterWorkspaceSymbolClient) -> Self {
        Self::TreeSitter(client)
    }
}

#[derive(Clone, Debug)]
pub enum HierarchyClient {
    Lsp(LspHierarchyClient),
    TreeSitter(TreeSitterHierarchyClient),
    Hybrid {
        lsp: LspHierarchyClient,
        tree_sitter: TreeSitterHierarchyClient,
    },
}

impl HierarchyClient {
    pub fn set_filters(&mut self, filters: FilterConfig) {
        match self {
            Self::Lsp(client) => client.set_filters(filters),
            Self::TreeSitter(_) => {}
            Self::Hybrid { lsp, .. } => lsp.set_filters(filters),
        }
    }

    pub fn with_fallback(lsp: LspHierarchyClient, tree_sitter: TreeSitterHierarchyClient) -> Self {
        Self::Hybrid { lsp, tree_sitter }
    }

    pub async fn query(&self, query: HierarchyQuery) -> anyhow::Result<HierarchyResponse> {
        match self {
            Self::Lsp(client) => client.query(query).await,
            Self::TreeSitter(client) => client.query(query).await,
            Self::Hybrid { lsp, tree_sitter } => {
                if lsp.supports(query.symbol.kind) {
                    lsp.query(query).await
                } else {
                    tree_sitter.query(query).await
                }
            }
        }
    }
}

impl From<LspHierarchyClient> for HierarchyClient {
    fn from(client: LspHierarchyClient) -> Self {
        Self::Lsp(client)
    }
}

impl From<TreeSitterHierarchyClient> for HierarchyClient {
    fn from(client: TreeSitterHierarchyClient) -> Self {
        Self::TreeSitter(client)
    }
}
