//! UI-independent application state transitions.
//!
//! Keep terminal events and async protocol details out of this module so modal,
//! selection, refresh, and graph mutations can be tested without a terminal.

use crate::{
    cli::{Cli, Command},
    config::SymbolFilter,
    fetch::{HierarchyQuery, HierarchyResponse},
    state::{
        HierarchyDirection, HierarchyKind, NodeId, SourceLocation, SymbolIdentity, Viewport,
        graph::RelationGraph,
    },
};

mod search;

use search::refresh_search_items;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnalysisBackend {
    Lsp(String),
    TreeSitter(String),
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnalysisPhase {
    Inactive,
    Ready,
    Working,
    Warning,
    Error,
    Disconnected,
}

/// UI-independent status reported by the active source-analysis backend.
///
/// This intentionally does not reuse `SearchStatus`: an LSP can still be
/// indexing after one workspace-symbol request has completed, and Tree-sitter
/// will have initialization work without an LSP request lifecycle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalysisStatus {
    pub backend: AnalysisBackend,
    pub phase: AnalysisPhase,
    pub message: Option<String>,
    pub percentage: Option<u32>,
}

impl AnalysisStatus {
    pub fn inactive(message: impl Into<String>) -> Self {
        Self {
            backend: AnalysisBackend::None,
            phase: AnalysisPhase::Inactive,
            message: Some(message.into()),
            percentage: None,
        }
    }

    pub fn lsp(server: impl Into<String>, phase: AnalysisPhase) -> Self {
        Self {
            backend: AnalysisBackend::Lsp(server.into()),
            phase,
            message: None,
            percentage: None,
        }
    }

    pub fn tree_sitter(language: impl Into<String>, phase: AnalysisPhase) -> Self {
        Self {
            backend: AnalysisBackend::TreeSitter(language.into()),
            phase,
            message: None,
            percentage: None,
        }
    }
}

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

#[derive(Debug)]
pub struct SearchState {
    pub kind: SearchKind,
    pub input: String,
    pub items: Vec<SearchItem>,
    pub selected: Option<usize>,
    pub status: SearchStatus,
    candidates: Vec<SearchItem>,
    request_id: u64,
    lsp_available: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchRequest {
    pub request_id: u64,
    pub kind: SearchKind,
    pub query: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HierarchyLoadRequest {
    pub request_id: u64,
    pub node_id: NodeId,
    pub query: HierarchyQuery,
}

#[derive(Debug)]
pub struct App {
    pub should_quit: bool,
    pub graph: RelationGraph,
    pub selected: Option<NodeId>,
    pub pending_key: Option<char>,
    pub search: Option<SearchState>,
    pub analysis_status: AnalysisStatus,
    pub viewport: Viewport,
    pub canvas_notice: Option<String>,
    symbol_filter: SymbolFilter,
    lsp_error: Option<String>,
    next_search_request_id: u64,
    next_hierarchy_request_id: u64,
}

impl App {
    pub fn from_cli(cli: Cli) -> Self {
        let mut graph = RelationGraph::default();
        let selected = cli.command.map(|command| {
            let (symbol, kind) = match command {
                Command::Call { symbol } => (symbol, HierarchyKind::Call),
                Command::Type { symbol } => (symbol, HierarchyKind::Type),
            };
            graph.pin_symbol(SymbolIdentity {
                symbol,
                kind,
                location: None,
            })
        });

        Self {
            should_quit: false,
            graph,
            selected,
            pending_key: None,
            search: None,
            analysis_status: AnalysisStatus::inactive("No analysis backend"),
            viewport: Viewport::default(),
            canvas_notice: None,
            symbol_filter: SymbolFilter::default(),
            lsp_error: None,
            next_search_request_id: 1,
            next_hierarchy_request_id: 1,
        }
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn set_lsp_error(&mut self, error: impl Into<String>) {
        self.lsp_error = Some(error.into());
    }

    pub fn set_analysis_status(&mut self, status: AnalysisStatus) {
        self.analysis_status = status;
    }

    pub fn pan_viewport(&mut self, delta_x: i32, delta_y: i32) {
        self.viewport.offset_x = self.viewport.offset_x.saturating_add(delta_x);
        self.viewport.offset_y = self.viewport.offset_y.saturating_add(delta_y);
    }

    pub fn set_symbol_filter(&mut self, symbol_filter: SymbolFilter) {
        self.symbol_filter = symbol_filter;
    }

    pub fn open_search(&mut self, kind: SearchKind, lsp_available: bool) -> Option<SearchRequest> {
        let status = if lsp_available {
            SearchStatus::Debouncing
        } else {
            SearchStatus::Error(
                self.lsp_error
                    .clone()
                    .unwrap_or_else(|| "LSP is unavailable; start ctree with --lsp".to_owned()),
            )
        };
        self.pending_key = None;
        self.search = Some(SearchState {
            kind,
            input: String::new(),
            items: Vec::new(),
            selected: None,
            status,
            candidates: Vec::new(),
            request_id: 0,
            lsp_available,
        });

        self.request_current_search()
    }

    pub fn close_search(&mut self) {
        self.search = None;
    }

    pub fn push_search_char(&mut self, character: char) -> Option<SearchRequest> {
        let search = self.search.as_mut()?;
        search.input.push(character);
        refresh_search_items(search);
        self.request_current_search()
    }

    pub fn pop_search_char(&mut self) -> Option<SearchRequest> {
        let search = self.search.as_mut()?;
        search.input.pop();
        refresh_search_items(search);
        self.request_current_search()
    }

    pub fn finish_search(&mut self, request_id: u64, result: Result<Vec<SearchItem>, String>) {
        let symbol_filter = &self.symbol_filter;
        let Some(search) = self.search.as_mut() else {
            return;
        };
        // A result may arrive after the modal was closed and reopened. Request
        // ids are global to App so an old session cannot replace a new one.
        if search.request_id != request_id {
            return;
        }

        match result {
            Ok(candidates) => {
                search.candidates = candidates
                    .into_iter()
                    .filter(|candidate| !symbol_filter.is_ignored(&candidate.name))
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

    pub fn delete_selected_anchor(&mut self) -> bool {
        let Some(selected) = self.selected else {
            return false;
        };
        if !self.graph.unpin(selected) {
            self.canvas_notice = Some("Selected node is not an anchor".to_owned());
            return false;
        }
        self.canvas_notice = None;
        self.selected = self.graph.anchors().last().copied();
        true
    }

    pub fn delete_selected_branch(&mut self, direction: HierarchyDirection) -> bool {
        let Some(selected) = self.selected else {
            return false;
        };
        let cleared = self.graph.clear_branch(selected, direction);
        if cleared {
            self.canvas_notice = None;
        }
        cleared
    }

    pub fn select_node(&mut self, node_id: NodeId) -> bool {
        if let Some(node_id) = self.graph.resolve_id(node_id) {
            self.selected = Some(node_id);
            return true;
        }
        false
    }

    pub fn toggle_selected_branch(
        &mut self,
        direction: HierarchyDirection,
        hierarchy_available: bool,
    ) -> Option<HierarchyLoadRequest> {
        let selected = self.selected?;
        self.toggle_node_branch(selected, direction, hierarchy_available)
    }

    pub fn toggle_node_branch(
        &mut self,
        node_id: NodeId,
        direction: HierarchyDirection,
        hierarchy_available: bool,
    ) -> Option<HierarchyLoadRequest> {
        let node_id = self.graph.resolve_id(node_id)?;
        let node = self.graph.node_mut(node_id)?;
        self.selected = Some(node_id);
        let identity = node.identity();
        let branch = node.branch_mut(direction);

        if branch.can_toggle() {
            branch.toggle();
            return None;
        }
        if branch.load_state == crate::state::LoadState::Loading {
            branch.expanded = !branch.expanded;
            return None;
        }
        if branch.load_state == crate::state::LoadState::Loaded {
            return None;
        }
        if !hierarchy_available {
            branch.load_state = crate::state::LoadState::Failed;
            branch.expanded = false;
            branch.failure = Some("Hierarchy requires an available LSP server".to_owned());
            branch.active_request_id = None;
            return None;
        }

        let request_id = self.next_hierarchy_request_id;
        self.next_hierarchy_request_id = self.next_hierarchy_request_id.wrapping_add(1);
        branch.load_state = crate::state::LoadState::Loading;
        branch.expanded = true;
        branch.failure = None;
        branch.active_request_id = Some(request_id);
        Some(HierarchyLoadRequest {
            request_id,
            node_id,
            query: HierarchyQuery {
                symbol: identity,
                direction,
            },
        })
    }

    pub fn finish_hierarchy(
        &mut self,
        request: &HierarchyLoadRequest,
        result: Result<HierarchyResponse, String>,
    ) -> bool {
        let Some(node_id) = self.graph.resolve_id(request.node_id) else {
            return false;
        };
        let branch = self
            .graph
            .node_mut(node_id)
            .expect("resolved graph nodes exist")
            .branch_mut(request.query.direction);
        if branch.active_request_id != Some(request.request_id) {
            return false;
        }
        match result {
            Ok(response) => {
                let was_selected = self.selected == Some(node_id);
                let Some(node_id) = self
                    .graph
                    .resolve_symbol(node_id, response.query.symbol.clone())
                else {
                    return false;
                };
                if was_selected {
                    self.selected = Some(node_id);
                }
                if self
                    .graph
                    .node(node_id)
                    .expect("resolved graph nodes exist")
                    .branch(request.query.direction)
                    .active_request_id
                    != Some(request.request_id)
                {
                    return false;
                }
                let children = response
                    .children
                    .into_iter()
                    .filter(|child| !self.symbol_filter.is_ignored(&child.symbol))
                    .collect();
                self.graph
                    .replace_branch_neighbors(node_id, request.query.direction, children);
                let branch = self
                    .graph
                    .node_mut(node_id)
                    .expect("resolved graph nodes exist")
                    .branch_mut(request.query.direction);
                branch.active_request_id = None;
                branch.load_state = crate::state::LoadState::Loaded;
                branch.failure = None;
                if branch.neighbors.is_empty() {
                    branch.expanded = false;
                }
            }
            Err(error) => {
                let branch = self
                    .graph
                    .node_mut(node_id)
                    .expect("resolved graph nodes exist")
                    .branch_mut(request.query.direction);
                branch.active_request_id = None;
                branch.load_state = crate::state::LoadState::Failed;
                branch.expanded = false;
                branch.failure = Some(error);
            }
        }
        true
    }

    fn request_current_search(&mut self) -> Option<SearchRequest> {
        let search = self.search.as_ref()?;
        if !search.lsp_available {
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
            query: search.input.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{App, SearchItem, SearchKind, SearchStatus};
    use crate::{
        cli::Cli,
        config::SymbolFilter,
        fetch::{FetchSource, HierarchyResponse},
        state::{
            HierarchyDirection, HierarchyKind, LoadState, NodeId, SourceLocation, SymbolIdentity,
        },
    };

    #[test]
    fn queries_on_open_and_after_each_text_change() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
        let open_request = app.open_search(SearchKind::Call, true).unwrap();

        assert_eq!(open_request.query, "");
        assert_eq!(
            app.search.as_ref().unwrap().status,
            SearchStatus::Debouncing
        );
        let first_request = app.push_search_char('F').unwrap();
        let current_request = app.push_search_char('B').unwrap();
        assert_eq!(first_request.query, "F");
        assert_eq!(current_request.query, "FB");
        app.start_search(open_request.request_id);
        assert_eq!(
            app.search.as_ref().unwrap().status,
            SearchStatus::Debouncing
        );
        app.start_search(current_request.request_id);
        assert_eq!(app.search.as_ref().unwrap().status, SearchStatus::Loading);

        app.finish_search(open_request.request_id, Ok(vec![item("stale")]));
        app.finish_search(
            current_request.request_id,
            Ok(vec![item("Bar"), item("FooBar"), item("FastBuffer")]),
        );

        let search = app.search.as_ref().unwrap();
        assert_eq!(
            search
                .items
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            ["FooBar", "FastBuffer"]
        );
        assert_eq!(search.status, SearchStatus::Ready);
    }

    #[test]
    fn ignores_results_from_a_closed_search_session() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
        let old_request = app.open_search(SearchKind::Call, true).unwrap();
        app.close_search();
        let current_request = app.open_search(SearchKind::Call, true).unwrap();

        app.finish_search(old_request.request_id, Ok(vec![item("old")]));
        assert!(app.search.as_ref().unwrap().items.is_empty());

        app.finish_search(current_request.request_id, Ok(vec![item("current")]));
        assert_eq!(app.search.as_ref().unwrap().items[0].name, "current");
    }

    #[test]
    fn ranks_exact_prefix_and_subsequence_matches() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
        app.open_search(SearchKind::Call, true).unwrap();
        let mut request = None;
        for character in "main".chars() {
            request = app.push_search_char(character);
        }
        app.finish_search(
            request.unwrap().request_id,
            Ok(vec![item("my_main"), item("main_loop"), item("main")]),
        );

        let names = app
            .search
            .as_ref()
            .unwrap()
            .items
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["main", "main_loop", "my_main"]);
    }

    #[test]
    fn matches_remaining_query_parts_against_container_and_path() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
        app.open_search(SearchKind::Call, true).unwrap();
        let mut request = None;
        for character in "run service".chars() {
            request = app.push_search_char(character);
        }
        let mut service_run = item("run");
        service_run.container_name = Some("Service".to_owned());
        let mut controller_run = item("run");
        controller_run.container_name = Some("Controller".to_owned());
        app.finish_search(
            request.unwrap().request_id,
            Ok(vec![controller_run, service_run]),
        );

        let names = app
            .search
            .as_ref()
            .unwrap()
            .items
            .iter()
            .map(|item| item.container_name.as_deref().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, ["Service"]);
    }

    #[test]
    fn applies_project_symbol_filter_to_search_and_hierarchy_results() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
        app.set_symbol_filter(
            SymbolFilter::from_patterns(["*::into", "Option::is_some", "*::Some"]).unwrap(),
        );
        let search = app.open_search(SearchKind::Call, true).unwrap();
        app.finish_search(
            search.request_id,
            Ok(vec![item("Vec::into"), item("main"), item("Option::Some")]),
        );

        assert_eq!(
            app.search
                .as_ref()
                .unwrap()
                .items
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            ["main"]
        );
        app.accept_search_selection();
        let hierarchy = app
            .toggle_selected_branch(HierarchyDirection::Outgoing, true)
            .unwrap();
        assert!(app.finish_hierarchy(
            &hierarchy,
            Ok(HierarchyResponse {
                query: hierarchy.query.clone(),
                children: vec![
                    identity("Option::is_some", HierarchyKind::Call),
                    identity("work", HierarchyKind::Call),
                    identity("Option::some", HierarchyKind::Call),
                ],
                source: FetchSource::Lsp,
            })
        ));

        let root = app.selected.unwrap();
        assert_eq!(
            branch_names(&app, root, HierarchyDirection::Outgoing),
            ["work", "Option::some"]
        );
    }

    #[test]
    fn accepts_a_result_as_a_deduplicated_anchor() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
        app.open_search(SearchKind::Type, true).unwrap();
        app.push_search_char('S').unwrap();
        let request = app.push_search_char('t').unwrap();
        app.finish_search(request.request_id, Ok(vec![item("Student")]));
        app.accept_search_selection();

        assert!(app.search.is_none());
        assert_eq!(app.graph.anchors().len(), 1);
        let existing_root = app.graph.anchors()[0];
        assert_eq!(app.graph.node(existing_root).unwrap().symbol, "Student");
        assert_eq!(
            app.graph
                .node(existing_root)
                .unwrap()
                .location
                .as_ref()
                .unwrap()
                .uri,
            "file:///workspace/main.rs"
        );

        app.pan_viewport(12, -4);
        app.open_search(SearchKind::Type, true).unwrap();
        app.push_search_char('S').unwrap();
        let request = app.push_search_char('t').unwrap();
        app.finish_search(request.request_id, Ok(vec![item("Student")]));
        app.accept_search_selection();

        assert_eq!(app.graph.anchors().len(), 1);
        assert_eq!(app.selected, Some(existing_root));
        assert_eq!(app.viewport.offset_x, 0);
        assert_eq!(app.viewport.offset_y, 0);
    }

    #[test]
    fn only_deletes_selected_anchors_and_selects_a_remaining_anchor() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
        let first_root = app.graph.pin_symbol(identity("first", HierarchyKind::Call));
        let child_id = app
            .graph
            .replace_branch_neighbors(
                first_root,
                HierarchyDirection::Outgoing,
                vec![identity("child", HierarchyKind::Call)],
            )
            .unwrap()[0];
        let second_root = app
            .graph
            .pin_symbol(identity("second", HierarchyKind::Type));
        app.selected = Some(child_id);

        assert!(!app.delete_selected_anchor());
        assert_eq!(
            app.canvas_notice.as_deref(),
            Some("Selected node is not an anchor")
        );
        assert_eq!(app.graph.anchors(), [first_root, second_root]);

        app.selected = Some(first_root);
        assert!(app.delete_selected_anchor());
        assert_eq!(app.graph.anchors(), [second_root]);
        assert_eq!(app.selected, Some(second_root));

        assert!(app.delete_selected_anchor());
        assert!(app.graph.anchors().is_empty());
        assert_eq!(app.selected, None);
    }

    #[test]
    fn deletes_only_the_selected_nodes_requested_branch() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
        let selected = app.graph.pin_symbol(identity("root", HierarchyKind::Call));
        app.graph
            .replace_branch_neighbors(
                selected,
                HierarchyDirection::Incoming,
                vec![identity("caller", HierarchyKind::Call)],
            )
            .unwrap();
        app.graph
            .replace_branch_neighbors(
                selected,
                HierarchyDirection::Outgoing,
                vec![identity("callee", HierarchyKind::Call)],
            )
            .unwrap();
        app.selected = Some(selected);

        assert!(app.delete_selected_branch(HierarchyDirection::Incoming));
        assert!(
            app.graph
                .node(selected)
                .unwrap()
                .incoming
                .neighbors
                .is_empty()
        );
        assert_eq!(
            app.graph.node(selected).unwrap().outgoing.neighbors.len(),
            1
        );
    }

    #[test]
    fn selects_nested_nodes_and_toggles_one_requested_branch() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree"]).unwrap());
        let root = app.graph.pin_symbol(identity("root", HierarchyKind::Call));
        let child_id = app
            .graph
            .replace_branch_neighbors(
                root,
                HierarchyDirection::Incoming,
                vec![identity("child", HierarchyKind::Call)],
            )
            .unwrap()[0];
        app.graph
            .replace_branch_neighbors(
                child_id,
                HierarchyDirection::Outgoing,
                vec![identity("grandchild", HierarchyKind::Call)],
            )
            .unwrap();
        app.graph.node_mut(root).unwrap().incoming.expanded = true;

        assert!(app.select_node(child_id));
        assert_eq!(app.selected, Some(child_id));
        assert_eq!(
            app.toggle_selected_branch(HierarchyDirection::Outgoing, false),
            None
        );
        assert!(app.graph.node(child_id).unwrap().outgoing.expanded);
        assert!(!app.select_node(NodeId(u64::MAX)));
        assert_eq!(app.selected, Some(child_id));
    }

    #[test]
    fn lazily_loads_a_branch_once_and_reuses_its_children() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree", "call", "root"]).unwrap());
        let request = app
            .toggle_selected_branch(HierarchyDirection::Outgoing, true)
            .unwrap();
        let root = app.selected.unwrap();

        assert_eq!(
            app.graph.node(root).unwrap().outgoing.load_state,
            LoadState::Loading
        );
        assert!(app.graph.node(root).unwrap().outgoing.expanded);
        assert_eq!(
            app.toggle_selected_branch(HierarchyDirection::Outgoing, true),
            None
        );
        assert!(!app.graph.node(root).unwrap().outgoing.expanded);

        assert!(app.finish_hierarchy(
            &request,
            Ok(HierarchyResponse {
                query: request.query.clone(),
                children: vec![identity("child", HierarchyKind::Call)],
                source: FetchSource::Lsp,
            })
        ));
        assert_eq!(
            app.graph.node(root).unwrap().outgoing.load_state,
            LoadState::Loaded
        );
        assert_eq!(app.graph.node(root).unwrap().outgoing.neighbors.len(), 1);
        assert!(!app.graph.node(root).unwrap().outgoing.expanded);

        assert_eq!(
            app.toggle_selected_branch(HierarchyDirection::Outgoing, true),
            None
        );
        assert!(app.graph.node(root).unwrap().outgoing.expanded);
    }

    #[test]
    fn retries_failed_hierarchy_and_ignores_stale_results() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree", "type", "Root"]).unwrap());
        let failed = app
            .toggle_selected_branch(HierarchyDirection::Incoming, true)
            .unwrap();
        let root = app.selected.unwrap();
        assert!(app.finish_hierarchy(&failed, Err("not supported".to_owned())));
        assert_eq!(
            app.graph.node(root).unwrap().incoming.load_state,
            LoadState::Failed
        );
        assert_eq!(
            app.graph.node(root).unwrap().incoming.failure(),
            Some("not supported")
        );

        let retry = app
            .toggle_selected_branch(HierarchyDirection::Incoming, true)
            .unwrap();
        assert_ne!(retry.request_id, failed.request_id);
        assert!(!app.finish_hierarchy(
            &failed,
            Ok(HierarchyResponse {
                query: failed.query.clone(),
                children: vec![identity("stale", HierarchyKind::Type)],
                source: FetchSource::Lsp,
            })
        ));
        assert!(app.graph.node(root).unwrap().incoming.neighbors.is_empty());
        assert!(app.finish_hierarchy(
            &retry,
            Ok(HierarchyResponse {
                query: retry.query.clone(),
                children: vec![identity("Parent", HierarchyKind::Type)],
                source: FetchSource::Lsp,
            })
        ));
        assert_eq!(
            branch_names(&app, root, HierarchyDirection::Incoming),
            ["Parent"]
        );
    }

    #[test]
    fn keeps_successful_empty_hierarchy_distinct_from_failure() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree", "call", "leaf"]).unwrap());
        let request = app
            .toggle_selected_branch(HierarchyDirection::Outgoing, true)
            .unwrap();
        let root = app.selected.unwrap();

        assert!(app.finish_hierarchy(
            &request,
            Ok(HierarchyResponse {
                query: request.query.clone(),
                children: Vec::new(),
                source: FetchSource::Lsp,
            })
        ));
        assert_eq!(
            app.graph.node(root).unwrap().outgoing.load_state,
            LoadState::Loaded
        );
        assert!(app.graph.node(root).unwrap().outgoing.neighbors.is_empty());
        assert_eq!(app.graph.node(root).unwrap().outgoing.failure(), None);
        assert_eq!(
            app.toggle_selected_branch(HierarchyDirection::Outgoing, true),
            None
        );
        assert_eq!(
            app.graph.node(root).unwrap().outgoing.load_state,
            LoadState::Loaded
        );
    }

    #[test]
    fn deduplicates_children_globally_but_preserves_both_direction_relations() {
        let mut app = App::from_cli(Cli::try_parse_from(["ctree", "call", "root"]).unwrap());
        let incoming = app
            .toggle_selected_branch(HierarchyDirection::Incoming, true)
            .unwrap();
        let shared = identity("shared", HierarchyKind::Call);
        assert!(app.finish_hierarchy(
            &incoming,
            Ok(HierarchyResponse {
                query: incoming.query.clone(),
                children: vec![
                    shared.clone(),
                    shared.clone(),
                    identity("left-only", HierarchyKind::Call),
                ],
                source: FetchSource::Lsp,
            })
        ));

        let outgoing = app
            .toggle_selected_branch(HierarchyDirection::Outgoing, true)
            .unwrap();
        assert!(app.finish_hierarchy(
            &outgoing,
            Ok(HierarchyResponse {
                query: outgoing.query.clone(),
                children: vec![
                    shared.clone(),
                    shared,
                    identity("right-only", HierarchyKind::Call),
                ],
                source: FetchSource::Lsp,
            })
        ));

        let root = app.selected.unwrap();
        let incoming_names = branch_names(&app, root, HierarchyDirection::Incoming);
        let outgoing_names = branch_names(&app, root, HierarchyDirection::Outgoing);
        assert_eq!(incoming_names, ["shared", "left-only"]);
        assert_eq!(outgoing_names, ["shared", "right-only"]);
        assert_eq!(app.graph.node_count(), 4);
    }

    fn item(name: &str) -> SearchItem {
        SearchItem {
            name: name.to_owned(),
            container_name: None,
            location: "file:///workspace/main.rs:1".to_owned(),
            source: Some(crate::state::SourceLocation {
                uri: "file:///workspace/main.rs".to_owned(),
                line: Some(0),
                character: Some(0),
            }),
        }
    }

    fn identity(symbol: &str, kind: HierarchyKind) -> SymbolIdentity {
        SymbolIdentity {
            symbol: symbol.to_owned(),
            kind,
            location: Some(SourceLocation {
                uri: "file:///workspace/main.rs".to_owned(),
                line: Some(symbol.bytes().map(u32::from).sum()),
                character: Some(0),
            }),
        }
    }

    fn branch_names(app: &App, node_id: NodeId, direction: HierarchyDirection) -> Vec<&str> {
        app.graph
            .node(node_id)
            .unwrap()
            .branch(direction)
            .neighbors
            .iter()
            .map(|neighbor| app.graph.node(*neighbor).unwrap().symbol.as_str())
            .collect()
    }
}
