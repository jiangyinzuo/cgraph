use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use tower_lsp::lsp_types::{Position, Range, SymbolKind, Url};
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator, Tree};

use super::TreeSitterLanguage;
use crate::{
    fetch::{FetchSource, HierarchyQuery, HierarchyResponse, WorkspaceSymbolMatch},
    state::{HierarchyDirection, HierarchyKind, SourceLocation, SymbolIdentity},
};

#[derive(Debug)]
pub(super) struct ProjectIndex {
    symbols: Vec<IndexedSymbol>,
    outgoing: HashMap<SymbolIdentity, Vec<SymbolIdentity>>,
    incoming: HashMap<SymbolIdentity, Vec<SymbolIdentity>>,
}

#[derive(Clone, Debug)]
struct IndexedSymbol {
    identity: SymbolIdentity,
    simple_name: String,
    container_name: Option<String>,
    symbol_kind: SymbolKind,
    path: PathBuf,
    scope_start: usize,
    scope_end: usize,
}

#[derive(Debug)]
struct PendingCall {
    owner: SymbolIdentity,
    target_name: String,
    path: PathBuf,
    preferred_container: Option<String>,
}

#[derive(Debug)]
struct PendingTypeRelation {
    parent_name: String,
    child_name: String,
    path: PathBuf,
}

impl ProjectIndex {
    pub(super) fn build(workspace_root: &Path, language: TreeSitterLanguage) -> Result<Self> {
        let grammar = language.grammar();
        let tags_query = Query::new(&grammar, language.tags_query())
            .with_context(|| format!("failed to initialize {} tags query", language.name()))?;
        let call_query = Query::new(&grammar, language.call_query()).with_context(|| {
            format!(
                "failed to initialize {} call-reference query",
                language.name()
            )
        })?;
        let files = source_files(workspace_root, language)?;
        let mut symbols = Vec::new();
        let mut pending_calls = Vec::new();
        let mut pending_types = Vec::new();

        for path in files {
            let source = fs::read_to_string(&path)
                .with_context(|| format!("failed to read source file {}", path.display()))?;
            let tree = parse_source(&grammar, language, &path, &source)?;
            let file_symbol_start = symbols.len();
            collect_definitions(language, &tags_query, &tree, &source, &path, &mut symbols)?;
            let file_symbols = &symbols[file_symbol_start..];
            collect_calls(
                &call_query,
                &tree,
                &source,
                &path,
                file_symbols,
                &mut pending_calls,
            )?;
            collect_type_relations(language, &tree, &source, &path, &mut pending_types)?;
        }

        let mut outgoing = HashMap::<SymbolIdentity, Vec<SymbolIdentity>>::new();
        for call in pending_calls {
            let Some(target) = resolve_symbol(
                &symbols,
                &call.target_name,
                HierarchyKind::Call,
                &call.path,
                call.preferred_container.as_deref(),
            ) else {
                continue;
            };
            push_unique(
                outgoing.entry(call.owner).or_default(),
                target.identity.clone(),
            );
        }
        for relation in pending_types {
            let Some(parent) = resolve_symbol(
                &symbols,
                &relation.parent_name,
                HierarchyKind::Type,
                &relation.path,
                None,
            ) else {
                continue;
            };
            let Some(child) = resolve_symbol(
                &symbols,
                &relation.child_name,
                HierarchyKind::Type,
                &relation.path,
                None,
            ) else {
                continue;
            };
            push_unique(
                outgoing.entry(parent.identity.clone()).or_default(),
                child.identity.clone(),
            );
        }

        let mut incoming = HashMap::<SymbolIdentity, Vec<SymbolIdentity>>::new();
        for (source, targets) in &outgoing {
            for target in targets {
                push_unique(incoming.entry(target.clone()).or_default(), source.clone());
            }
        }

        Ok(Self {
            symbols,
            outgoing,
            incoming,
        })
    }

    pub(super) fn workspace_symbols(&self) -> Vec<WorkspaceSymbolMatch> {
        self.symbols
            .iter()
            .filter_map(|symbol| {
                let location = symbol.identity.location.as_ref()?;
                let uri = Url::parse(&location.uri).ok()?;
                let position = Position::new(location.line?, location.character?);
                Some(WorkspaceSymbolMatch {
                    name: symbol.identity.symbol.clone(),
                    kind: symbol.symbol_kind,
                    container_name: symbol.container_name.clone(),
                    uri,
                    range: Some(Range::new(position, position)),
                })
            })
            .collect()
    }

    pub(super) fn hierarchy(&self, mut query: HierarchyQuery) -> Result<HierarchyResponse> {
        let symbol = self.resolve_query_symbol(&query.symbol)?;
        query.symbol = symbol.clone();
        let children = match query.direction {
            HierarchyDirection::Incoming => self.incoming.get(&symbol),
            HierarchyDirection::Outgoing => self.outgoing.get(&symbol),
        }
        .cloned()
        .unwrap_or_default();

        Ok(HierarchyResponse {
            query,
            children,
            source: FetchSource::TreeSitter,
        })
    }

    fn resolve_query_symbol(&self, query: &SymbolIdentity) -> Result<SymbolIdentity> {
        if let Some(location) = &query.location {
            if let Some(symbol) = self.symbols.iter().find(|symbol| {
                symbol.identity.kind == query.kind
                    && symbol.identity.location.as_ref() == Some(location)
            }) {
                return Ok(symbol.identity.clone());
            }
            bail!(
                "Tree-sitter index has no symbol at {}",
                display_location(location)
            );
        }

        let simple_name = terminal_name(&query.symbol);
        let candidates = self
            .symbols
            .iter()
            .filter(|symbol| {
                symbol.identity.kind == query.kind
                    && (symbol.identity.symbol == query.symbol || symbol.simple_name == simple_name)
            })
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [symbol] => Ok(symbol.identity.clone()),
            [] => bail!(
                "could not resolve {:?} in the Tree-sitter project index",
                query.symbol
            ),
            _ => bail!(
                "symbol {:?} is ambiguous; add it through ac/at to select an exact location",
                query.symbol
            ),
        }
    }
}

fn collect_definitions(
    language: TreeSitterLanguage,
    query: &Query,
    tree: &Tree,
    source: &str,
    path: &Path,
    symbols: &mut Vec<IndexedSymbol>,
) -> Result<()> {
    let uri = Url::from_file_path(path).map_err(|()| {
        anyhow::anyhow!(
            "source path is not representable as a file URI: {}",
            path.display()
        )
    })?;
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
    let mut identities = symbols
        .iter()
        .map(|symbol| symbol.identity.clone())
        .collect::<HashSet<_>>();

    while let Some(query_match) = matches.next() {
        let definition = query_match
            .captures
            .iter()
            .find(|capture| capture_names[capture.index as usize].starts_with("definition."));
        let name = query_match
            .captures
            .iter()
            .find(|capture| capture_names[capture.index as usize] == "name");
        let (Some(definition), Some(name)) = (definition, name) else {
            continue;
        };
        let capture_name = capture_names[definition.index as usize];
        let Some((hierarchy_kind, symbol_kind)) = definition_kind(capture_name) else {
            continue;
        };
        if hierarchy_kind == HierarchyKind::Call
            && matches!(language, TreeSitterLanguage::C | TreeSitterLanguage::Cpp)
            && ancestor_of_kind(definition.node, "function_definition").is_none()
        {
            continue;
        }
        let simple_name = node_text(name.node, source)?.to_owned();
        let container_name = (hierarchy_kind == HierarchyKind::Call)
            .then(|| function_container(language, definition.node, source))
            .flatten();
        let symbol_name = match (&container_name, language) {
            (Some(container), TreeSitterLanguage::Python) => {
                format!("{container}.{simple_name}")
            }
            (Some(container), _) => format!("{container}::{simple_name}"),
            (None, _) => simple_name.clone(),
        };
        let point = name.node.start_position();
        let identity = SymbolIdentity {
            symbol: symbol_name,
            kind: hierarchy_kind,
            location: Some(SourceLocation {
                uri: uri.to_string(),
                line: Some(u32::try_from(point.row).context("source row exceeds u32")?),
                character: Some(utf16_column(source, name.node.start_byte(), point.column)?),
            }),
        };
        if !identities.insert(identity.clone()) {
            continue;
        }
        let scope = definition_scope(language, definition.node);
        symbols.push(IndexedSymbol {
            identity,
            simple_name,
            container_name,
            symbol_kind,
            path: path.to_owned(),
            scope_start: scope.start_byte(),
            scope_end: scope.end_byte(),
        });
    }
    Ok(())
}

fn collect_calls(
    query: &Query,
    tree: &Tree,
    source: &str,
    path: &Path,
    file_symbols: &[IndexedSymbol],
    calls: &mut Vec<PendingCall>,
) -> Result<()> {
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
    while let Some(query_match) = matches.next() {
        let reference = query_match
            .captures
            .iter()
            .find(|capture| capture_names[capture.index as usize] == "reference.call");
        let name = query_match
            .captures
            .iter()
            .find(|capture| capture_names[capture.index as usize] == "name");
        let (Some(reference), Some(name)) = (reference, name) else {
            continue;
        };
        let Some(owner) = file_symbols
            .iter()
            .filter(|symbol| {
                symbol.identity.kind == HierarchyKind::Call
                    && symbol.scope_start <= reference.node.start_byte()
                    && symbol.scope_end >= reference.node.end_byte()
            })
            .min_by_key(|symbol| symbol.scope_end.saturating_sub(symbol.scope_start))
        else {
            continue;
        };
        calls.push(PendingCall {
            owner: owner.identity.clone(),
            target_name: terminal_name(node_text(name.node, source)?),
            path: path.to_owned(),
            preferred_container: owner.container_name.clone(),
        });
    }
    Ok(())
}

fn collect_type_relations(
    language: TreeSitterLanguage,
    tree: &Tree,
    source: &str,
    path: &Path,
    relations: &mut Vec<PendingTypeRelation>,
) -> Result<()> {
    walk_named_nodes(tree.root_node(), |node| match language {
        TreeSitterLanguage::Rust if node.kind() == "impl_item" => {
            let Some(parent) = node.child_by_field_name("trait") else {
                return Ok(());
            };
            let Some(child) = node.child_by_field_name("type") else {
                return Ok(());
            };
            relations.push(PendingTypeRelation {
                parent_name: terminal_name(node_text(parent, source)?),
                child_name: terminal_name(node_text(child, source)?),
                path: path.to_owned(),
            });
            Ok(())
        }
        TreeSitterLanguage::Cpp if node.kind() == "class_specifier" => {
            let Some(child) = node.child_by_field_name("name") else {
                return Ok(());
            };
            let child_name = terminal_name(node_text(child, source)?);
            let mut cursor = node.walk();
            for base_clause in node
                .named_children(&mut cursor)
                .filter(|child| child.kind() == "base_class_clause")
            {
                let mut base_cursor = base_clause.walk();
                for base in base_clause.named_children(&mut base_cursor).filter(|base| {
                    matches!(
                        base.kind(),
                        "type_identifier" | "qualified_identifier" | "template_type"
                    )
                }) {
                    relations.push(PendingTypeRelation {
                        parent_name: terminal_name(node_text(base, source)?),
                        child_name: child_name.clone(),
                        path: path.to_owned(),
                    });
                }
            }
            Ok(())
        }
        TreeSitterLanguage::Python if node.kind() == "class_definition" => {
            let Some(child) = node.child_by_field_name("name") else {
                return Ok(());
            };
            let Some(superclasses) = node.child_by_field_name("superclasses") else {
                return Ok(());
            };
            let child_name = terminal_name(node_text(child, source)?);
            let mut cursor = superclasses.walk();
            for parent in superclasses.named_children(&mut cursor) {
                relations.push(PendingTypeRelation {
                    parent_name: terminal_name(node_text(parent, source)?),
                    child_name: child_name.clone(),
                    path: path.to_owned(),
                });
            }
            Ok(())
        }
        _ => Ok(()),
    })
}

fn resolve_symbol<'a>(
    symbols: &'a [IndexedSymbol],
    simple_name: &str,
    kind: HierarchyKind,
    source_path: &Path,
    preferred_container: Option<&str>,
) -> Option<&'a IndexedSymbol> {
    let candidates = symbols
        .iter()
        .filter(|symbol| symbol.identity.kind == kind && symbol.simple_name == simple_name)
        .collect::<Vec<_>>();
    if let Some(container) = preferred_container {
        let matching_container = candidates
            .iter()
            .copied()
            .filter(|symbol| symbol.container_name.as_deref() == Some(container))
            .collect::<Vec<_>>();
        if let [symbol] = matching_container.as_slice() {
            return Some(*symbol);
        }
    }
    let same_file = candidates
        .iter()
        .copied()
        .filter(|symbol| symbol.path == source_path)
        .collect::<Vec<_>>();
    if let [symbol] = same_file.as_slice() {
        return Some(*symbol);
    }
    match candidates.as_slice() {
        [symbol] => Some(*symbol),
        _ => None,
    }
}

fn definition_kind(capture_name: &str) -> Option<(HierarchyKind, SymbolKind)> {
    match capture_name {
        "definition.function" => Some((HierarchyKind::Call, SymbolKind::FUNCTION)),
        "definition.method" => Some((HierarchyKind::Call, SymbolKind::METHOD)),
        "definition.class" => Some((HierarchyKind::Type, SymbolKind::CLASS)),
        "definition.interface" => Some((HierarchyKind::Type, SymbolKind::INTERFACE)),
        "definition.type" => Some((HierarchyKind::Type, SymbolKind::STRUCT)),
        _ => None,
    }
}

fn definition_scope<'tree>(language: TreeSitterLanguage, mut node: Node<'tree>) -> Node<'tree> {
    let scope_kind = match language {
        TreeSitterLanguage::Rust => "function_item",
        TreeSitterLanguage::C | TreeSitterLanguage::Cpp => "function_definition",
        TreeSitterLanguage::Python => "function_definition",
    };
    let original = node;
    loop {
        if node.kind() == scope_kind {
            return node;
        }
        let Some(parent) = node.parent() else {
            return original;
        };
        node = parent;
    }
}

fn function_container(
    language: TreeSitterLanguage,
    mut node: Node<'_>,
    source: &str,
) -> Option<String> {
    let definition = node;
    loop {
        let container = match language {
            TreeSitterLanguage::Rust if node.kind() == "impl_item" => {
                node.child_by_field_name("type")
            }
            TreeSitterLanguage::Rust if node.kind() == "trait_item" => {
                node.child_by_field_name("name")
            }
            TreeSitterLanguage::Cpp if node.kind() == "class_specifier" => {
                node.child_by_field_name("name")
            }
            TreeSitterLanguage::Python if node.kind() == "class_definition" => {
                node.child_by_field_name("name")
            }
            _ => None,
        };
        if let Some(container) = container {
            return node_text(container, source)
                .ok()
                .map(terminal_name)
                .filter(|name| !name.is_empty());
        }
        let Some(parent) = node.parent() else {
            break;
        };
        node = parent;
    }

    if language == TreeSitterLanguage::Cpp {
        let mut stack = vec![definition];
        while let Some(candidate) = stack.pop() {
            if candidate.kind() == "qualified_identifier"
                && let Some(scope) = candidate.child_by_field_name("scope")
            {
                return node_text(scope, source).ok().map(terminal_name);
            }
            let mut cursor = candidate.walk();
            stack.extend(candidate.named_children(&mut cursor));
        }
    }
    None
}

fn ancestor_of_kind<'tree>(mut node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    loop {
        if node.kind() == kind {
            return Some(node);
        }
        node = node.parent()?;
    }
}

fn parse_source(
    grammar: &tree_sitter::Language,
    language: TreeSitterLanguage,
    path: &Path,
    source: &str,
) -> Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(grammar)
        .with_context(|| format!("failed to initialize {} grammar", language.name()))?;
    parser.parse(source, None).with_context(|| {
        format!(
            "{} parser returned no syntax tree for {}",
            language.name(),
            path.display()
        )
    })
}

fn source_files(workspace_root: &Path, language: TreeSitterLanguage) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_source_files(workspace_root, language, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_source_files(
    directory: &Path,
    language: TreeSitterLanguage,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    let mut entries = fs::read_dir(directory)
        .with_context(|| format!("failed to read workspace directory {}", directory.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", path.display()))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || matches!(name.as_ref(), "target" | "node_modules") {
                continue;
            }
            collect_source_files(&path, language, files)?;
        } else if file_type.is_file() && language.accepts_path(&path) {
            files.push(path.canonicalize().with_context(|| {
                format!("failed to canonicalize source file {}", path.display())
            })?);
        }
    }
    Ok(())
}

fn walk_named_nodes<'tree>(
    root: Node<'tree>,
    mut visit: impl FnMut(Node<'tree>) -> Result<()>,
) -> Result<()> {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        visit(node)?;
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    Ok(())
}

fn node_text<'a>(node: Node<'_>, source: &'a str) -> Result<&'a str> {
    node.utf8_text(source.as_bytes())
        .context("Tree-sitter capture was not valid UTF-8")
}

pub(super) fn utf16_column(source: &str, byte_offset: usize, byte_column: usize) -> Result<u32> {
    let line_start = byte_offset
        .checked_sub(byte_column)
        .context("Tree-sitter source column exceeds node byte offset")?;
    let prefix = source
        .get(line_start..byte_offset)
        .context("Tree-sitter source column did not end on a UTF-8 boundary")?;
    u32::try_from(prefix.encode_utf16().count()).context("source UTF-16 column exceeds u32")
}

fn terminal_name(value: &str) -> String {
    let value = value.trim();
    let value = value.rsplit("::").next().unwrap_or(value);
    let value = value.rsplit('.').next().unwrap_or(value);
    value
        .split(['<', '[', '(', ' ', '\t', '\n'])
        .next()
        .unwrap_or(value)
        .trim_matches(|character: char| !character.is_alphanumeric() && character != '_')
        .to_owned()
}

fn push_unique(values: &mut Vec<SymbolIdentity>, value: SymbolIdentity) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn display_location(location: &SourceLocation) -> String {
    match (location.line, location.character) {
        (Some(line), Some(character)) => {
            format!("{}:{}:{}", location.uri, line + 1, character + 1)
        }
        _ => location.uri.clone(),
    }
}
