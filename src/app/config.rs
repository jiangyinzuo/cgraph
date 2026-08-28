use crate::{
    config::{FilterConfig, PathFilter, SymbolFilter},
    fetch::CachePolicy,
    state::{HierarchyDirection, LoadState},
};

use super::{App, HierarchyLoadRequest};

impl App {
    pub fn set_symbol_filter(&mut self, symbol_filter: SymbolFilter) {
        self.symbol_filter = symbol_filter;
    }

    pub fn set_path_filter(&mut self, path_filter: PathFilter) {
        self.path_filter = path_filter;
    }

    pub fn set_filters(&mut self, filters: FilterConfig) {
        self.symbol_filter = filters.symbol_filter();
        self.path_filter = filters.path_filter();
        self.filters = filters;
    }

    pub fn reload_filters(
        &mut self,
        symbol_filter: SymbolFilter,
        path_filter: PathFilter,
        hierarchy_available: bool,
    ) -> Vec<HierarchyLoadRequest> {
        self.path_filter = path_filter;
        self.reload_symbol_filter(symbol_filter, hierarchy_available)
    }

    pub fn reload_filter_config(
        &mut self,
        filters: FilterConfig,
        hierarchy_available: bool,
    ) -> Vec<HierarchyLoadRequest> {
        self.path_filter = filters.path_filter();
        self.filters = filters.clone();
        self.reload_symbol_filter(filters.symbol_filter(), hierarchy_available)
    }

    pub fn reload_symbol_filter(
        &mut self,
        symbol_filter: SymbolFilter,
        hierarchy_available: bool,
    ) -> Vec<HierarchyLoadRequest> {
        self.symbol_filter = symbol_filter;
        let mut targets = Vec::new();
        for node_id in self.graph.known_graph().nodes {
            let Some(node) = self.graph.node(node_id) else {
                continue;
            };
            for direction in [HierarchyDirection::Incoming, HierarchyDirection::Outgoing] {
                if matches!(
                    node.branch(direction).load_state,
                    LoadState::Loaded | LoadState::Loading
                ) {
                    targets.push((node_id, node.identity(), direction));
                }
            }
        }

        if !hierarchy_available {
            self.set_canvas_notice(
                "Project config reloaded; graph refresh requires an analysis provider".to_owned(),
            );
            return Vec::new();
        }

        let requests = targets
            .into_iter()
            .filter_map(|(node_id, identity, direction)| {
                self.begin_hierarchy_load(node_id, identity, direction, CachePolicy::Refresh, false)
            })
            .collect::<Vec<_>>();
        self.set_canvas_notice(if requests.is_empty() {
            "Project config reloaded; no loaded graph branches to refresh".to_owned()
        } else {
            format!(
                "Project config reloaded; refreshing {} graph branches",
                requests.len()
            )
        });
        requests
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::super::App;
    use crate::{
        cli::Cli,
        config::SymbolFilter,
        fetch::{FetchSource, HierarchyResponse},
        state::{HierarchyDirection, HierarchyKind, NodeId, SourceLocation, SymbolIdentity},
    };

    #[test]
    fn reloads_filters_across_loaded_branches_and_supersedes_old_requests() {
        let mut app = App::from_cli(Cli::try_parse_from(["cgraph", "call", "root"]).unwrap());
        for direction in [HierarchyDirection::Incoming, HierarchyDirection::Outgoing] {
            let request = app.toggle_selected_branch(direction, true).unwrap();
            assert!(app.finish_hierarchy(&request, Ok(response(&request, Vec::new()))));
        }
        let stale = app.refresh_selected_branches(true);

        let current =
            app.reload_symbol_filter(SymbolFilter::from_patterns(["noise"]).unwrap(), true);

        assert_eq!(current.len(), 2);
        assert!(current.iter().all(|request| {
            stale
                .iter()
                .find(|stale| stale.query.direction == request.query.direction)
                .is_some_and(|stale| stale.request_id != request.request_id)
        }));
        assert!(!app.finish_hierarchy(&stale[0], Ok(response(&stale[0], vec![identity("stale")]))));
        for request in &current {
            assert!(app.finish_hierarchy(
                request,
                Ok(response(request, vec![identity("noise"), identity("keep")]))
            ));
        }
        let root = app.selected.unwrap();
        assert_eq!(
            branch_names(&app, root, HierarchyDirection::Incoming),
            ["keep"]
        );
        assert_eq!(
            branch_names(&app, root, HierarchyDirection::Outgoing),
            ["keep"]
        );
        assert!(app.graph.is_anchor(root));
    }

    fn response(
        request: &super::HierarchyLoadRequest,
        children: Vec<SymbolIdentity>,
    ) -> HierarchyResponse {
        HierarchyResponse {
            query: request.query.clone(),
            children,
            source: FetchSource::Lsp,
        }
    }

    fn identity(symbol: &str) -> SymbolIdentity {
        SymbolIdentity {
            symbol: symbol.to_owned(),
            kind: HierarchyKind::Call,
            location: Some(SourceLocation {
                uri: format!("file:///workspace/{symbol}.rs"),
                line: Some(0),
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
