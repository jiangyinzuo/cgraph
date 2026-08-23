#![doc = include_str!("README.md")]

pub mod lsp;
pub mod treesitter;

use lsp::{HierarchyClient, LspProvider, WorkspaceSymbolClient, WorkspaceSymbolMatch};

use crate::state::{HierarchyDirection, SymbolIdentity};

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

#[derive(Debug, Default)]
pub struct FetchCoordinator {
    lsp: Option<LspProvider>,
}

impl FetchCoordinator {
    pub fn with_lsp(lsp: LspProvider) -> Self {
        Self { lsp: Some(lsp) }
    }

    pub fn lsp(&self) -> Option<&LspProvider> {
        self.lsp.as_ref()
    }

    pub fn workspace_symbol_client(&self) -> Option<WorkspaceSymbolClient> {
        self.lsp.as_ref().map(LspProvider::workspace_symbol_client)
    }

    pub fn hierarchy_client(&self) -> Option<HierarchyClient> {
        self.lsp.as_ref().map(LspProvider::hierarchy_client)
    }

    pub async fn workspace_symbols(
        &self,
        query: &str,
    ) -> anyhow::Result<Vec<WorkspaceSymbolMatch>> {
        let lsp = self
            .lsp
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no LSP provider is configured"))?;

        lsp.workspace_symbols(query).await
    }

    pub async fn hierarchy(&self, query: HierarchyQuery) -> anyhow::Result<HierarchyResponse> {
        let lsp = self
            .lsp
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no hierarchy provider is configured"))?;
        lsp.hierarchy_client().query(query).await
    }
}
