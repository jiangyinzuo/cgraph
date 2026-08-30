use super::fuzzy;
use super::{App, candidate_is_visible};
use crate::state::{HierarchyKind, SourceLocation, SymbolIdentity, Viewport};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchKind {
    Call,
    Type,
}

impl SearchKind {
    pub fn hierarchy_kind(self) -> HierarchyKind {
        match self {
            Self::Call => HierarchyKind::Call,
            Self::Type => HierarchyKind::Type,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchItem {
    pub name: String,
    pub container_name: Option<String>,
    pub location: String,
    pub source: Option<SourceLocation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SearchStatus {
    Debouncing,
    Loading,
    Ready,
    Error(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchField {
    LspQuery,
    Symbol,
    Uri,
}

impl SearchField {
    fn next(self) -> Self {
        match self {
            Self::LspQuery => Self::Symbol,
            Self::Symbol => Self::Uri,
            Self::Uri => Self::LspQuery,
        }
    }
}

#[derive(Debug)]
pub struct SearchState {
    pub kind: SearchKind,
    pub lsp_query: String,
    pub symbol_query: String,
    pub uri_query: String,
    pub active_field: SearchField,
    pub items: Vec<SearchItem>,
    pub selected: Option<usize>,
    pub status: SearchStatus,
    candidates: Vec<SearchItem>,
    request_id: u64,
    provider_available: bool,
}

impl SearchState {
    fn active_input_mut(&mut self) -> &mut String {
        match self.active_field {
            SearchField::LspQuery => &mut self.lsp_query,
            SearchField::Symbol => &mut self.symbol_query,
            SearchField::Uri => &mut self.uri_query,
        }
    }

    pub fn candidate_count(&self) -> usize {
        self.candidates.len()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchRequest {
    pub request_id: u64,
    pub kind: SearchKind,
    pub query: String,
}

impl App {
    pub fn open_search(
        &mut self,
        kind: SearchKind,
        provider_available: bool,
    ) -> Option<SearchRequest> {
        let status = if provider_available {
            SearchStatus::Debouncing
        } else {
            let error = self
                .analysis_error
                .clone()
                .unwrap_or_else(|| "No workspace-symbol provider is available".to_owned());
            self.set_canvas_error(format!("Workspace symbol search unavailable: {error}"));
            SearchStatus::Error(error)
        };
        self.pending_key = None;
        self.search = Some(SearchState {
            kind,
            lsp_query: String::new(),
            symbol_query: String::new(),
            uri_query: String::new(),
            active_field: SearchField::LspQuery,
            items: Vec::new(),
            selected: None,
            status,
            candidates: Vec::new(),
            request_id: 0,
            provider_available,
        });

        self.request_current_search()
    }

    pub fn close_search(&mut self) {
        self.search = None;
    }

    pub fn push_search_char(&mut self, character: char) -> Option<SearchRequest> {
        let search = self.search.as_mut()?;
        let query_lsp = search.active_field == SearchField::LspQuery;
        search.active_input_mut().push(character);
        refresh_search_items(search);
        if query_lsp {
            self.request_current_search()
        } else {
            None
        }
    }

    pub fn pop_search_char(&mut self) -> Option<SearchRequest> {
        let search = self.search.as_mut()?;
        let query_lsp = search.active_field == SearchField::LspQuery;
        search.active_input_mut().pop();
        refresh_search_items(search);
        if query_lsp {
            self.request_current_search()
        } else {
            None
        }
    }

    pub fn cycle_search_field(&mut self) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        search.active_field = search.active_field.next();
    }

    pub fn finish_search(&mut self, request_id: u64, result: Result<Vec<SearchItem>, String>) {
        let Some(current_request_id) = self.search.as_ref().map(|search| search.request_id) else {
            return;
        };
        if current_request_id != request_id {
            return;
        }

        if let Err(error) = &result {
            self.set_canvas_error(format!("Workspace symbol query failed: {error}"));
        }

        let Some(search) = self.search.as_mut() else {
            return;
        };

        match result {
            Ok(candidates) => {
                let symbol_filter = &self.symbol_filter;
                let filters = &self.filters;
                let workspace = &self.workspace;
                search.candidates = candidates
                    .into_iter()
                    .filter(|candidate| {
                        !symbol_filter.is_ignored(&candidate.name)
                            && candidate_is_visible(
                                &candidate.name,
                                candidate.source.as_ref(),
                                filters,
                                workspace,
                            )
                    })
                    .collect();
                search.status = SearchStatus::Ready;
                refresh_search_items(search);
            }
            Err(error) => {
                search.candidates.clear();
                search.items.clear();
                search.selected = None;
                search.status = SearchStatus::Error(error);
            }
        }
    }

    pub fn start_search(&mut self, request_id: u64) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        if search.request_id == request_id {
            search.status = SearchStatus::Loading;
        }
    }

    pub fn select_search_item(&mut self, index: usize) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        if index < search.items.len() {
            search.selected = Some(index);
        }
    }

    pub fn move_search_selection(&mut self, offset: isize) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        if search.items.is_empty() {
            search.selected = None;
            return;
        }

        let current = search.selected.unwrap_or(0);
        let last = search.items.len() - 1;
        search.selected = Some(current.saturating_add_signed(offset).min(last));
    }

    pub fn accept_search_selection(&mut self) {
        let Some(search) = self.search.as_ref() else {
            return;
        };
        let Some(item) = search.selected.and_then(|index| search.items.get(index)) else {
            return;
        };
        let node_id = self.graph.pin_symbol(SymbolIdentity {
            symbol: item.name.clone(),
            kind: search.kind.hierarchy_kind(),
            location: item.source.clone(),
        });
        self.selected = Some(node_id);
        self.viewport = Viewport::default();
        self.close_search();
    }

    fn request_current_search(&mut self) -> Option<SearchRequest> {
        let search = self.search.as_ref()?;
        if !search.provider_available {
            return None;
        }

        let request_id = self.next_search_request_id;
        self.next_search_request_id = self.next_search_request_id.wrapping_add(1);
        let search = self.search.as_mut().expect("search was checked above");
        search.request_id = request_id;
        search.status = SearchStatus::Debouncing;
        Some(SearchRequest {
            request_id,
            kind: search.kind,
            query: search.lsp_query.clone(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SearchScore {
    symbol: Option<u32>,
    uri: Option<u32>,
}

pub(super) fn refresh_search_items(search: &mut SearchState) {
    let mut matches = search
        .candidates
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            search_score(&search.symbol_query, &search.uri_query, item).map(|score| (index, score))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| {
                search.candidates[left.0]
                    .name
                    .to_lowercase()
                    .cmp(&search.candidates[right.0].name.to_lowercase())
            })
            .then_with(|| left.0.cmp(&right.0))
    });

    search.items = matches
        .into_iter()
        .map(|(index, _)| search.candidates[index].clone())
        .collect();
    search.selected = (!search.items.is_empty()).then_some(0);
}

fn search_score(symbol_query: &str, uri_query: &str, item: &SearchItem) -> Option<SearchScore> {
    Some(SearchScore {
        symbol: optional_score(symbol_query, &item.name)?,
        uri: optional_uri_score(uri_query, item)?,
    })
}

fn optional_score(query: &str, candidate: &str) -> Option<Option<u32>> {
    if query.split_whitespace().next().is_none() {
        Some(None)
    } else {
        fuzzy::score(query, candidate).map(Some)
    }
}

fn optional_uri_score(query: &str, item: &SearchItem) -> Option<Option<u32>> {
    if query.split_whitespace().next().is_none() {
        return Some(None);
    }
    let location_score = fuzzy::score(query, &item.location);
    let source_score = item
        .source
        .as_ref()
        .and_then(|source| fuzzy::score(query, &source.uri));
    location_score.max(source_score).map(Some)
}
