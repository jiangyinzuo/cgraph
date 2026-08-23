#![doc = include_str!("README.md")]

pub mod lsp;
pub mod treesitter;

use lsp::{
    HierarchyClient as LspHierarchyClient, LspProvider,
    WorkspaceSymbolClient as LspWorkspaceSymbolClient,
};
use treesitter::{
    HierarchyClient as TreeSitterHierarchyClient, TreeSitterProvider,
    WorkspaceSymbolClient as TreeSitterWorkspaceSymbolClient,
};

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
}

impl HierarchyClient {
    pub async fn query(&self, query: HierarchyQuery) -> anyhow::Result<HierarchyResponse> {
        match self {
            Self::Lsp(client) => client.query(query).await,
            Self::TreeSitter(client) => client.query(query).await,
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

#[derive(Debug, Default)]
pub struct FetchCoordinator {
    lsp: Option<LspProvider>,
    tree_sitter: Option<TreeSitterProvider>,
}

impl FetchCoordinator {
    pub fn with_lsp(lsp: LspProvider) -> Self {
        Self {
            lsp: Some(lsp),
            tree_sitter: None,
        }
    }

    pub fn with_tree_sitter(tree_sitter: TreeSitterProvider) -> Self {
        Self {
            lsp: None,
            tree_sitter: Some(tree_sitter),
        }
    }

    pub fn lsp(&self) -> Option<&LspProvider> {
        self.lsp.as_ref()
    }

    pub fn workspace_symbol_client(&self) -> Option<WorkspaceSymbolClient> {
        self.lsp
            .as_ref()
            .map(LspProvider::workspace_symbol_client)
            .map(WorkspaceSymbolClient::from)
            .or_else(|| {
                self.tree_sitter
                    .as_ref()
                    .map(TreeSitterProvider::workspace_symbol_client)
                    .map(WorkspaceSymbolClient::from)
            })
    }

    pub fn hierarchy_client(&self) -> Option<HierarchyClient> {
        self.lsp
            .as_ref()
            .map(LspProvider::hierarchy_client)
            .map(HierarchyClient::from)
            .or_else(|| {
                self.tree_sitter
                    .as_ref()
                    .map(TreeSitterProvider::hierarchy_client)
                    .map(HierarchyClient::from)
            })
    }

    pub async fn workspace_symbols(
        &self,
        query: &str,
    ) -> anyhow::Result<Vec<WorkspaceSymbolMatch>> {
        self.workspace_symbol_client()
            .ok_or_else(|| anyhow::anyhow!("no workspace-symbol provider is configured"))?
            .query(query)
            .await
    }

    pub async fn hierarchy(&self, query: HierarchyQuery) -> anyhow::Result<HierarchyResponse> {
        self.hierarchy_client()
            .ok_or_else(|| anyhow::anyhow!("no hierarchy provider is configured"))?
            .query(query)
            .await
    }
}
