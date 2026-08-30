//! UI-independent application state transitions.
//!
//! Keep terminal events and async protocol details out of this module so modal,
//! selection, refresh, and graph mutations can be tested without a terminal.

use crate::{
    cli::{Cli, Command},
    config::FilterConfig,
    state::{HierarchyKind, NodeId, SymbolIdentity, Viewport, graph::RelationGraph},
};

use std::path::PathBuf;

mod analysis;
mod config;
mod filtering;
mod fuzzy;
mod help;
mod hierarchy;
mod messages;
mod save;
mod search;

pub use analysis::{AnalysisBackend, AnalysisPhase, AnalysisStatus};
pub use help::HelpState;
pub use hierarchy::HierarchyLoadRequest;
pub use save::{SaveState, SaveStatus};
pub use search::{SearchField, SearchItem, SearchKind, SearchRequest, SearchState, SearchStatus};

#[derive(Debug)]
pub struct App {
    pub should_quit: bool,
    pub workspace: PathBuf,
    pub graph: RelationGraph,
    pub selected: Option<NodeId>,
    pub pending_key: Option<char>,
    pub search: Option<SearchState>,
    pub save: Option<SaveState>,
    pub help: Option<HelpState>,
    pub analysis_status: AnalysisStatus,
    pub message_history: Vec<String>,
    pub viewport: Viewport,
    pub canvas_notice: Option<String>,
    canvas_notice_is_error: bool,
    filters: FilterConfig,
    analysis_error: Option<String>,
    next_search_request_id: u64,
    next_hierarchy_request_id: u64,
}

impl App {
    pub fn from_cli(cli: Cli) -> Self {
        let mut graph = RelationGraph::default();
        let workspace = cli.workspace.clone();
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
            workspace,
            graph,
            selected,
            pending_key: None,
            search: None,
            save: None,
            help: None,
            analysis_status: AnalysisStatus::inactive("No analysis backend"),
            message_history: Vec::new(),
            viewport: Viewport::default(),
            canvas_notice: None,
            canvas_notice_is_error: false,
            filters: FilterConfig::from_rules(std::iter::empty::<&str>(), false)
                .expect("empty filter config is valid"),
            analysis_error: None,
            next_search_request_id: 1,
            next_hierarchy_request_id: 1,
        }
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn pan_viewport(&mut self, delta_x: i32, delta_y: i32) {
        self.viewport.offset_x = self.viewport.offset_x.saturating_add(delta_x);
        self.viewport.offset_y = self.viewport.offset_y.saturating_add(delta_y);
    }

    pub fn focus_symbol(&mut self, identity: SymbolIdentity) -> Result<NodeId, String> {
        if identity.symbol.trim().is_empty() {
            return Err("symbol must not be empty".to_owned());
        }
        if let Some(location) = &identity.location
            && (location
                .uri
                .strip_prefix("file://")
                .is_none_or(str::is_empty)
                || location.uri.chars().any(char::is_control)
                || location.line.is_none()
                || location.character.is_none())
        {
            return Err(
                "location must contain a file URI and exact zero-based line/character".to_owned(),
            );
        }

        let node_id = if identity.location.is_some() {
            self.graph.pin_symbol(identity)
        } else {
            match self
                .graph
                .nodes_named(&identity.symbol, identity.kind)
                .as_slice()
            {
                [] => self.graph.pin_symbol(identity),
                [node_id] => {
                    self.graph.pin(*node_id);
                    *node_id
                }
                _ => {
                    return Err(format!(
                        "symbol {:?} is ambiguous; include an exact source location",
                        identity.symbol
                    ));
                }
            }
        };
        self.selected = Some(node_id);
        self.viewport = Viewport::default();
        self.clear_canvas_notice();
        Ok(node_id)
    }

    pub fn delete_selected_anchor(&mut self) -> bool {
        let Some(selected) = self.selected else {
            return false;
        };
        if !self.graph.unpin(selected) {
            self.set_canvas_notice("Selected node is not an anchor");
            return false;
        }
        self.clear_canvas_notice();
        self.selected = self.graph.anchors().last().copied();
        true
    }

    pub fn select_node(&mut self, node_id: NodeId) -> bool {
        if let Some(node_id) = self.graph.resolve_id(node_id) {
            self.selected = Some(node_id);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{App, SearchField, SearchItem, SearchKind, SearchStatus};
    use crate::{
        cli::Cli,
        config::FilterConfig,
        fetch::{CachePolicy, FetchSource, HierarchyResponse},
        state::{
            HierarchyDirection, HierarchyKind, LoadState, NodeId, SourceLocation, SymbolIdentity,
        },
    };

    #[test]
    fn lsp_field_queries_on_open_and_sends_its_complete_text() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let open_request = app.open_search(SearchKind::Call, true).unwrap();

        assert_eq!(open_request.query, "");
        assert_eq!(
            app.search.as_ref().unwrap().status,
            SearchStatus::Debouncing
        );
        let first_request = app.push_search_char('F').unwrap();
        let mut current_request = first_request.clone();
        for character in "oo Bar".chars() {
            current_request = app.push_search_char(character).unwrap();
        }
        assert_eq!(first_request.query, "F");
        assert_eq!(current_request.query, "Foo Bar");
        app.start_search(open_request.request_id);
        assert_eq!(
            app.search.as_ref().unwrap().status,
            SearchStatus::Debouncing
        );
        app.start_search(current_request.request_id);
        assert_eq!(app.search.as_ref().unwrap().status, SearchStatus::Loading);

        app.finish_search(open_request.request_id, Ok(vec![item("stale")]));
        app.finish_search(current_request.request_id, Ok(vec![item("FooBar")]));

        let search = app.search.as_ref().unwrap();
        assert_eq!(
            search
                .items
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            ["FooBar"]
        );
        assert_eq!(search.status, SearchStatus::Ready);
    }

    #[test]
    fn ignores_results_from_a_closed_search_session() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let old_request = app.open_search(SearchKind::Call, true).unwrap();
        app.close_search();
        let current_request = app.open_search(SearchKind::Call, true).unwrap();

        app.finish_search(old_request.request_id, Ok(vec![item("old")]));
        assert!(app.search.as_ref().unwrap().items.is_empty());

        app.finish_search(current_request.request_id, Ok(vec![item("current")]));
        assert_eq!(app.search.as_ref().unwrap().items[0].name, "current");
    }

    #[test]
    fn symbol_field_filters_cached_results_without_requesting_provider() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let request = app.open_search(SearchKind::Call, true).unwrap();
        app.finish_search(
            request.request_id,
            Ok(vec![item("my_main"), item("main_loop"), item("main")]),
        );
        app.cycle_search_field();
        assert_eq!(
            app.search.as_ref().unwrap().active_field,
            SearchField::Symbol
        );
        for character in "main".chars() {
            assert!(app.push_search_char(character).is_none());
        }

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
    fn symbol_field_accepts_space_separated_fuzzy_input() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let request = app.open_search(SearchKind::Call, true).unwrap();
        let mut service_run = item("ParserService::thread_worker");
        service_run.container_name = Some("Service".to_owned());
        let mut controller_run = item("ParserController::run");
        controller_run.container_name = Some("Controller".to_owned());
        app.finish_search(request.request_id, Ok(vec![controller_run, service_run]));
        app.cycle_search_field();
        for character in "prs thrd".chars() {
            assert!(app.push_search_char(character).is_none());
        }

        let names = app
            .search
            .as_ref()
            .unwrap()
            .items
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["ParserService::thread_worker"]);
    }

    #[test]
    fn empty_query_keeps_all_candidates_visible() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let request = app.open_search(SearchKind::Call, true).unwrap();
        assert_eq!(request.query, "");
        app.finish_search(
            request.request_id,
            Ok(vec![item("zeta"), item("alpha"), item("中间")]),
        );

        let names = app
            .search
            .as_ref()
            .unwrap()
            .items
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["alpha", "zeta", "中间"]);
    }

    #[test]
    fn uri_field_filters_cached_results_without_changing_lsp_query() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let request = app.open_search(SearchKind::Call, true).unwrap();
        let mut candidate = item("run");
        candidate.location = "/workspace/服务.rs:1".to_owned();
        candidate.source.as_mut().unwrap().uri = "file:///workspace/服务.rs".to_owned();
        app.finish_search(request.request_id, Ok(vec![item("other"), candidate]));
        app.cycle_search_field();
        app.cycle_search_field();
        assert_eq!(app.search.as_ref().unwrap().active_field, SearchField::Uri);
        for character in "file 服务".chars() {
            assert!(app.push_search_char(character).is_none());
        }

        assert_eq!(
            app.search.as_ref().unwrap().items[0].location,
            "/workspace/服务.rs:1"
        );
    }

    #[test]
    fn tab_cycles_three_search_fields_without_changing_their_text() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        app.open_search(SearchKind::Call, true).unwrap();
        app.push_search_char('m').unwrap();

        app.cycle_search_field();
        assert_eq!(
            app.search.as_ref().unwrap().active_field,
            SearchField::Symbol
        );
        assert!(app.push_search_char('s').is_none());
        app.cycle_search_field();
        assert_eq!(app.search.as_ref().unwrap().active_field, SearchField::Uri);
        assert!(app.push_search_char('u').is_none());
        app.cycle_search_field();

        let search = app.search.as_ref().unwrap();
        assert_eq!(search.active_field, SearchField::LspQuery);
        assert_eq!(search.lsp_query, "m");
        assert_eq!(search.symbol_query, "s");
        assert_eq!(search.uri_query, "u");
    }

    #[test]
    fn applies_project_symbol_filter_to_search_and_hierarchy_results() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        app.set_filters(
            FilterConfig::from_rules(["#*::into", "#Option::is_some", "#*::Some"], false).unwrap(),
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
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
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
    fn external_focus_reuses_semantic_nodes_and_rejects_ambiguous_names() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
        let first = identity("run", HierarchyKind::Call);
        let first_id = app.graph.insert_symbol(first.clone());
        app.viewport.offset_x = 17;

        assert_eq!(app.focus_symbol(first.clone()).unwrap(), first_id);
        assert_eq!(app.graph.node_count(), 1);
        assert_eq!(app.graph.anchors(), [first_id]);
        assert_eq!(app.selected, Some(first_id));
        assert_eq!(app.viewport, crate::state::Viewport::default());

        let second = SymbolIdentity {
            symbol: "run".to_owned(),
            kind: HierarchyKind::Call,
            location: Some(SourceLocation {
                uri: "file:///workspace/src/other.rs".to_owned(),
                line: Some(3),
                character: Some(1),
            }),
        };
        app.graph.insert_symbol(second);
        let error = app
            .focus_symbol(SymbolIdentity {
                symbol: "run".to_owned(),
                kind: HierarchyKind::Call,
                location: None,
            })
            .unwrap_err();
        assert!(error.contains("ambiguous"));
        assert_eq!(app.graph.node_count(), 2);

        let created = app
            .focus_symbol(SymbolIdentity {
                symbol: "new_type".to_owned(),
                kind: HierarchyKind::Type,
                location: None,
            })
            .unwrap();
        assert_eq!(app.graph.node(created).unwrap().symbol, "new_type");
        assert!(app.graph.is_anchor(created));

        for location in [
            SourceLocation {
                uri: "file://".to_owned(),
                line: Some(0),
                character: Some(0),
            },
            SourceLocation {
                uri: "file:///workspace/src/main.py".to_owned(),
                line: Some(0),
                character: None,
            },
        ] {
            let error = app
                .focus_symbol(SymbolIdentity {
                    symbol: "invalid".to_owned(),
                    kind: HierarchyKind::Call,
                    location: Some(location),
                })
                .unwrap_err();
            assert!(error.contains("exact zero-based line/character"));
        }
    }

    #[test]
    fn only_deletes_selected_anchors_and_selects_a_remaining_anchor() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
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
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
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
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph"]).unwrap());
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
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "call", "root"]).unwrap());
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
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "type", "Root"]).unwrap());
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
        assert_eq!(
            app.message_history.last().map(String::as_str),
            Some("Hierarchy query failed for Root (incoming/parent): not supported")
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
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "call", "leaf"]).unwrap());
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
    fn tree_sitter_hierarchy_reports_its_syntactic_confidence() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "call", "root"]).unwrap());
        let request = app
            .toggle_selected_branch(HierarchyDirection::Outgoing, true)
            .unwrap();

        assert!(app.finish_hierarchy(
            &request,
            Ok(HierarchyResponse {
                query: request.query.clone(),
                children: vec![identity("child", HierarchyKind::Call)],
                source: FetchSource::TreeSitter,
            })
        ));

        assert_eq!(
            app.canvas_notice.as_deref(),
            Some("Tree-sitter: syntactic relations only; dynamic dispatch may be omitted")
        );
    }

    #[test]
    fn deduplicates_children_globally_but_preserves_both_direction_relations() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "call", "root"]).unwrap());
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

    #[test]
    fn refreshes_both_branches_and_preserves_existing_descendant_state() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "call", "root"]).unwrap());
        let incoming = app
            .toggle_selected_branch(HierarchyDirection::Incoming, true)
            .unwrap();
        let caller_identity = identity("caller", HierarchyKind::Call);
        assert!(app.finish_hierarchy(
            &incoming,
            Ok(HierarchyResponse {
                query: incoming.query.clone(),
                children: vec![caller_identity.clone()],
                source: FetchSource::Lsp,
            })
        ));
        let outgoing = app
            .toggle_selected_branch(HierarchyDirection::Outgoing, true)
            .unwrap();
        let removed_identity = identity("removed", HierarchyKind::Call);
        assert!(app.finish_hierarchy(
            &outgoing,
            Ok(HierarchyResponse {
                query: outgoing.query.clone(),
                children: vec![removed_identity],
                source: FetchSource::Lsp,
            })
        ));

        let root = app.selected.unwrap();
        let caller = app.graph.node(root).unwrap().incoming.neighbors[0];
        app.graph
            .replace_branch_neighbors(
                caller,
                HierarchyDirection::Outgoing,
                vec![identity("grandchild", HierarchyKind::Call)],
            )
            .unwrap();
        let caller_branch = &mut app.graph.node_mut(caller).unwrap().outgoing;
        caller_branch.load_state = LoadState::Loaded;
        caller_branch.expanded = true;

        let requests = app.refresh_selected_branches(true);
        assert_eq!(requests.len(), 2);
        assert!(
            requests
                .iter()
                .all(|request| request.cache_policy == CachePolicy::Refresh)
        );
        assert_eq!(
            requests
                .iter()
                .map(|request| request.query.direction)
                .collect::<Vec<_>>(),
            [HierarchyDirection::Incoming, HierarchyDirection::Outgoing]
        );
        assert!(app.graph.node(root).unwrap().incoming.expanded);
        assert!(app.graph.node(root).unwrap().outgoing.expanded);

        for request in &requests {
            let children = match request.query.direction {
                HierarchyDirection::Incoming => vec![
                    caller_identity.clone(),
                    identity("new-caller", HierarchyKind::Call),
                ],
                HierarchyDirection::Outgoing => {
                    vec![identity("new-callee", HierarchyKind::Call)]
                }
            };
            assert!(app.finish_hierarchy(
                request,
                Ok(HierarchyResponse {
                    query: request.query.clone(),
                    children,
                    source: FetchSource::Lsp,
                })
            ));
        }

        assert_eq!(app.graph.node(root).unwrap().incoming.neighbors[0], caller);
        assert!(app.graph.node(caller).unwrap().outgoing.expanded);
        assert_eq!(
            branch_names(&app, caller, HierarchyDirection::Outgoing),
            ["grandchild"]
        );
        assert_eq!(
            branch_names(&app, root, HierarchyDirection::Incoming),
            ["caller", "new-caller"]
        );
        assert_eq!(
            branch_names(&app, root, HierarchyDirection::Outgoing),
            ["new-callee"]
        );
        let new_callee = app.graph.node(root).unwrap().outgoing.neighbors[0];
        assert_eq!(
            app.graph.node(new_callee).unwrap().outgoing.load_state,
            LoadState::NotLoaded
        );
        assert!(
            app.graph.visible_graph().nodes.iter().all(|node_id| app
                .graph
                .node(*node_id)
                .unwrap()
                .symbol
                != "removed")
        );
    }

    #[test]
    fn failed_refresh_keeps_cached_neighbors_and_rejects_older_results() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "call", "root"]).unwrap());
        let initial = app
            .toggle_selected_branch(HierarchyDirection::Outgoing, true)
            .unwrap();
        assert!(app.finish_hierarchy(
            &initial,
            Ok(HierarchyResponse {
                query: initial.query.clone(),
                children: vec![identity("cached", HierarchyKind::Call)],
                source: FetchSource::Lsp,
            })
        ));
        let root = app.selected.unwrap();

        let older = app.refresh_selected_branches(true);
        let current = app.refresh_selected_branches(true);
        let older_outgoing = older
            .iter()
            .find(|request| request.query.direction == HierarchyDirection::Outgoing)
            .unwrap();
        assert!(!app.finish_hierarchy(
            older_outgoing,
            Ok(HierarchyResponse {
                query: older_outgoing.query.clone(),
                children: vec![identity("stale", HierarchyKind::Call)],
                source: FetchSource::Lsp,
            })
        ));

        for request in &current {
            assert!(app.finish_hierarchy(request, Err("refresh failed".to_owned())));
        }
        assert_eq!(
            branch_names(&app, root, HierarchyDirection::Outgoing),
            ["cached"]
        );
        let outgoing = &app.graph.node(root).unwrap().outgoing;
        assert_eq!(outgoing.load_state, LoadState::Loaded);
        assert!(outgoing.expanded);
        assert_eq!(outgoing.failure(), Some("refresh failed"));
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
