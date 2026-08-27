use std::path::Path;

use tower_lsp::lsp_types::{
    CallHierarchyItem, DocumentSymbol, DocumentSymbolResponse, Location, OneOf, Position, Range,
    SymbolInformation, SymbolKind, TextDocumentIdentifier, TextDocumentPositionParams,
    TypeHierarchyItem, Url, WorkspaceSymbolResponse,
};

use crate::{
    fetch::WorkspaceSymbolMatch,
    state::{HierarchyKind, SourceLocation, SymbolIdentity},
};

use super::symbol_names::SymbolNameAdapter;

pub(super) fn symbol_leaf_name(symbol: &str) -> &str {
    let symbol = symbol
        .rsplit_once("::")
        .map(|(_, name)| name)
        .unwrap_or(symbol);
    symbol
        .rsplit_once('.')
        .map(|(_, name)| name)
        .unwrap_or(symbol)
}

pub(super) fn symbol_belongs_to_workspace(
    symbol: &WorkspaceSymbolMatch,
    workspace_root: &Path,
) -> bool {
    uri_belongs_to_workspace(&symbol.uri, workspace_root)
}

pub(super) fn workspace_symbol_is_visible(
    symbol: &WorkspaceSymbolMatch,
    workspace_root: &Path,
    workspace_only: bool,
) -> bool {
    !workspace_only || symbol_belongs_to_workspace(symbol, workspace_root)
}

pub(super) fn uri_belongs_to_workspace(uri: &Url, workspace_root: &Path) -> bool {
    uri.to_file_path()
        .is_ok_and(|path| path.starts_with(workspace_root))
}

pub(super) fn deduplicate_symbols(
    symbols: impl IntoIterator<Item = WorkspaceSymbolMatch>,
) -> Vec<WorkspaceSymbolMatch> {
    let mut unique = Vec::new();
    for symbol in symbols {
        let duplicate = unique.iter().any(|existing: &WorkspaceSymbolMatch| {
            existing.name == symbol.name
                && existing.kind == symbol.kind
                && existing.uri == symbol.uri
                && existing.range == symbol.range
                && existing.container_name == symbol.container_name
        });
        if !duplicate {
            unique.push(symbol);
        }
    }
    unique
}

pub(super) fn document_position(
    uri: Url,
    position: Position,
) -> (TextDocumentPositionParams, SourceLocation) {
    let location = SourceLocation {
        uri: uri.to_string(),
        line: Some(position.line),
        character: Some(position.character),
    };
    (
        TextDocumentPositionParams::new(TextDocumentIdentifier::new(uri), position),
        location,
    )
}

pub(super) fn symbol_kind_matches_hierarchy(kind: HierarchyKind, symbol_kind: SymbolKind) -> bool {
    match kind {
        HierarchyKind::Call => matches!(
            symbol_kind,
            SymbolKind::FUNCTION | SymbolKind::METHOD | SymbolKind::CONSTRUCTOR
        ),
        HierarchyKind::Type => matches!(
            symbol_kind,
            SymbolKind::CLASS
                | SymbolKind::INTERFACE
                | SymbolKind::STRUCT
                | SymbolKind::ENUM
                | SymbolKind::TYPE_PARAMETER
        ),
    }
}

pub(super) fn call_item_identity(
    item: CallHierarchyItem,
    symbol_names: SymbolNameAdapter,
    document_container: Option<&str>,
) -> SymbolIdentity {
    let symbol = symbol_names.call_hierarchy_item(
        &item.name,
        item.kind,
        item.detail.as_deref(),
        document_container,
    );
    SymbolIdentity {
        symbol,
        kind: HierarchyKind::Call,
        location: Some(SourceLocation {
            uri: item.uri.to_string(),
            line: Some(item.selection_range.start.line),
            character: Some(item.selection_range.start.character),
        }),
    }
}

#[derive(Clone, Debug)]
pub(super) struct DocumentSymbolOwner {
    pub(super) name: String,
    pub(super) kind: SymbolKind,
    pub(super) range: Range,
    pub(super) container_name: Option<String>,
}

pub(super) fn normalize_document_symbols(
    response: DocumentSymbolResponse,
) -> Vec<DocumentSymbolOwner> {
    match response {
        DocumentSymbolResponse::Flat(symbols) => {
            symbols.into_iter().map(document_symbol_owner).collect()
        }
        DocumentSymbolResponse::Nested(symbols) => {
            let mut normalized = Vec::new();
            normalize_nested_document_symbols(&symbols, None, &mut normalized);
            normalized
        }
    }
}

#[allow(deprecated)]
fn document_symbol_owner(symbol: SymbolInformation) -> DocumentSymbolOwner {
    DocumentSymbolOwner {
        name: symbol.name,
        kind: symbol.kind,
        range: symbol.location.range,
        container_name: symbol.container_name,
    }
}

fn normalize_nested_document_symbols(
    symbols: &[DocumentSymbol],
    container_name: Option<&str>,
    normalized: &mut Vec<DocumentSymbolOwner>,
) {
    for symbol in symbols {
        normalized.push(DocumentSymbolOwner {
            name: symbol.name.clone(),
            kind: symbol.kind,
            range: symbol.range,
            container_name: container_name.map(str::to_owned),
        });
        if let Some(children) = symbol.children.as_deref() {
            normalize_nested_document_symbols(children, Some(&symbol.name), normalized);
        }
    }
}

pub(super) fn find_document_symbol_container<'a>(
    symbols: &'a [DocumentSymbolOwner],
    item: &CallHierarchyItem,
) -> Option<&'a str> {
    symbols
        .iter()
        .filter(|symbol| {
            symbol.name == item.name
                && matches!(
                    symbol.kind,
                    SymbolKind::FUNCTION | SymbolKind::METHOD | SymbolKind::CONSTRUCTOR
                )
                && range_contains_position(symbol.range, item.selection_range.start)
        })
        .min_by_key(|symbol| range_span_key(symbol.range))
        .and_then(|symbol| symbol.container_name.as_deref())
}

fn range_contains_position(range: Range, position: Position) -> bool {
    position_after_or_equal(position, range.start) && position_after_or_equal(range.end, position)
}

fn position_after_or_equal(left: Position, right: Position) -> bool {
    (left.line, left.character) >= (right.line, right.character)
}

fn range_span_key(range: Range) -> (u32, u32) {
    (
        range.end.line.saturating_sub(range.start.line),
        range.end.character.saturating_sub(range.start.character),
    )
}

pub(super) fn type_item_identity(item: TypeHierarchyItem) -> SymbolIdentity {
    SymbolIdentity {
        symbol: item.name,
        kind: HierarchyKind::Type,
        location: Some(SourceLocation {
            uri: item.uri.to_string(),
            line: Some(item.selection_range.start.line),
            character: Some(item.selection_range.start.character),
        }),
    }
}

pub(super) fn deduplicate_identities(
    identities: impl IntoIterator<Item = SymbolIdentity>,
) -> Vec<SymbolIdentity> {
    let mut unique = Vec::new();
    for identity in identities {
        if !unique.contains(&identity) {
            unique.push(identity);
        }
    }
    unique
}

pub(super) fn normalize_symbols(
    response: WorkspaceSymbolResponse,
    symbol_names: SymbolNameAdapter,
) -> Vec<WorkspaceSymbolMatch> {
    match response {
        WorkspaceSymbolResponse::Flat(symbols) => symbols
            .into_iter()
            .map(|symbol| {
                let name = symbol_names.workspace_symbol(
                    &symbol.name,
                    symbol.kind,
                    symbol.container_name.as_deref(),
                );
                WorkspaceSymbolMatch {
                    name,
                    kind: symbol.kind,
                    container_name: symbol.container_name,
                    uri: symbol.location.uri,
                    range: Some(symbol.location.range),
                }
            })
            .collect(),
        WorkspaceSymbolResponse::Nested(symbols) => symbols
            .into_iter()
            .map(|symbol| {
                let (uri, range) = match symbol.location {
                    OneOf::Left(Location { uri, range }) => (uri, Some(range)),
                    OneOf::Right(location) => (location.uri, None),
                };
                let name = symbol_names.workspace_symbol(
                    &symbol.name,
                    symbol.kind,
                    symbol.container_name.as_deref(),
                );
                WorkspaceSymbolMatch {
                    name,
                    kind: symbol.kind,
                    container_name: symbol.container_name,
                    uri,
                    range,
                }
            })
            .collect(),
    }
}
