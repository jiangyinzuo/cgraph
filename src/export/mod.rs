#![doc = include_str!("README.md")]

use std::{collections::HashMap, fmt::Write as _, fs::OpenOptions, io::Write as _, path::Path};

use anyhow::{Context, Result, bail};

use crate::{
    state::graph::RelationGraph,
    state::{HierarchyKind, SourceLocation},
};

const FORMAT_HEADER: &str = "cgraph graph · text v1";

/// Serializes all known relations reachable from an anchor.
///
/// Local node numbers come from semantic sorting instead of process-global
/// `NodeId` allocation order, so repeated exports of the same graph state are
/// byte-for-byte stable. Control characters are escaped so a symbol cannot
/// break the deliberately simple one-record-per-line presentation.
pub fn render_text(graph: &RelationGraph) -> String {
    let known = graph.known_graph();
    let mut node_ids = known.nodes;
    node_ids.sort_by(|left, right| {
        let left = graph.node(*left).expect("known graph nodes exist");
        let right = graph.node(*right).expect("known graph nodes exist");
        node_sort_key(left).cmp(&node_sort_key(right))
    });
    let local_ids = node_ids
        .iter()
        .enumerate()
        .map(|(index, node_id)| (*node_id, index + 1))
        .collect::<HashMap<_, _>>();

    let mut output = String::new();
    writeln!(output, "{FORMAT_HEADER}\n").expect("writing to String cannot fail");
    writeln!(output, "Nodes ({})", node_ids.len()).expect("writing to String cannot fail");
    for node_id in &node_ids {
        let node = graph.node(*node_id).expect("known graph nodes exist");
        let kind = match node.kind {
            HierarchyKind::Call => "call",
            HierarchyKind::Type => "type",
        };
        let anchor = if graph.is_anchor(*node_id) {
            "  [anchor]"
        } else {
            ""
        };
        writeln!(
            output,
            "  [{}] {}  {}{}",
            local_ids[node_id],
            kind,
            inline_text(&node.symbol),
            anchor,
        )
        .expect("writing to String cannot fail");
        writeln!(output, "      {}", display_location(node.location.as_ref()))
            .expect("writing to String cannot fail");
    }

    let mut edges = known
        .edges
        .into_iter()
        .map(|edge| {
            (
                local_ids[&edge.source],
                edge.source,
                local_ids[&edge.target],
                edge.target,
            )
        })
        .collect::<Vec<_>>();
    edges.sort_unstable_by_key(|(source, _, target, _)| (*source, *target));
    writeln!(output, "\nRelations ({})", edges.len()).expect("writing to String cannot fail");
    for (source, source_id, target, target_id) in edges {
        let source_name = &graph
            .node(source_id)
            .expect("known graph nodes exist")
            .symbol;
        let target_name = &graph
            .node(target_id)
            .expect("known graph nodes exist")
            .symbol;
        writeln!(
            output,
            "  [{source}] {}  →  [{target}] {}",
            inline_text(source_name),
            inline_text(target_name)
        )
        .expect("writing to String cannot fail");
    }
    output
}

/// Creates a new export file without ever opening an existing target for
/// truncation. `create_new` makes the non-overwrite guarantee atomic with
/// respect to another process creating the same path concurrently.
pub fn write_text(graph: &RelationGraph, path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        bail!("destination path is empty");
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("failed to create export {}", path.display()))?;
    file.write_all(render_text(graph).as_bytes())
        .and_then(|()| file.flush())
        .with_context(|| format!("failed to write export {}", path.display()))
}

fn node_sort_key(
    node: &crate::state::graph::GraphNode,
) -> (u8, Option<&str>, Option<u32>, Option<u32>, &str, u64) {
    let kind = match node.kind {
        HierarchyKind::Call => 0,
        HierarchyKind::Type => 1,
    };
    (
        kind,
        node.location.as_ref().map(|location| location.uri.as_str()),
        node.location.as_ref().and_then(|location| location.line),
        node.location
            .as_ref()
            .and_then(|location| location.character),
        node.symbol.as_str(),
        node.id.0,
    )
}

fn inline_text(value: &str) -> String {
    if value.is_empty() {
        return "<unnamed>".to_owned();
    }
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn display_location(location: Option<&SourceLocation>) -> String {
    let Some(location) = location else {
        return "location unknown".to_owned();
    };
    let mut display = inline_text(&location.uri);
    if let Some(line) = location.line {
        write!(display, ":{}", line.saturating_add(1)).expect("writing to String cannot fail");
        if let Some(character) = location.character {
            write!(display, ":{}", character.saturating_add(1))
                .expect("writing to String cannot fail");
        }
    } else if let Some(character) = location.character {
        write!(display, " · character {}", character.saturating_add(1))
            .expect("writing to String cannot fail");
    }
    display
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, time::SystemTime};

    use super::{render_text, write_text};
    use crate::state::{
        HierarchyDirection, HierarchyKind, SourceLocation, SymbolIdentity, graph::RelationGraph,
    };

    #[test]
    fn renders_stable_nodes_shared_edges_and_cycles() {
        let mut graph = RelationGraph::default();
        let root = graph.pin_symbol(identity("root", HierarchyKind::Call, 1));
        let left = graph
            .replace_branch_neighbors(
                root,
                HierarchyDirection::Outgoing,
                vec![
                    identity("right", HierarchyKind::Call, 3),
                    identity("left", HierarchyKind::Call, 2),
                ],
            )
            .unwrap();
        let shared = graph
            .replace_branch_neighbors(
                left[0],
                HierarchyDirection::Outgoing,
                vec![identity("shared", HierarchyKind::Call, 4)],
            )
            .unwrap()[0];
        graph.replace_branch_neighbors(
            left[1],
            HierarchyDirection::Outgoing,
            vec![identity("shared", HierarchyKind::Call, 4)],
        );
        graph.replace_branch_neighbors(
            shared,
            HierarchyDirection::Outgoing,
            vec![identity("root", HierarchyKind::Call, 1)],
        );
        graph.pin_symbol(identity("TypeA", HierarchyKind::Type, 10));

        let first = render_text(&graph);
        let second = render_text(&graph);
        assert_eq!(first, second);
        assert!(first.starts_with("cgraph graph · text v1\n\nNodes (5)\n"));
        assert_eq!(
            first.lines().filter(|line| line.contains("shared")).count(),
            4
        );
        assert!(first.contains("] call  root  [anchor]"));
        assert!(first.contains("] type  TypeA  [anchor]"));
        assert!(first.contains("Relations (5)"));
        assert_eq!(first.matches("  →  ").count(), 5);
        assert!(first.contains("file:///workspace/src/main.rs:2:1"));
    }

    #[test]
    fn creates_new_files_and_never_truncates_existing_targets() {
        let workspace = temporary_workspace("write");
        let target = workspace.join("graph.txt");
        let mut graph = RelationGraph::default();
        graph.pin_symbol(identity("root", HierarchyKind::Call, 1));

        write_text(&graph, &target).unwrap();
        let written = fs::read_to_string(&target).unwrap();
        assert_eq!(written, render_text(&graph));

        fs::write(&target, "keep me\n").unwrap();
        let error = write_text(&graph, &target).unwrap_err();
        assert!(error.to_string().contains("failed to create export"));
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep me\n");
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn rejects_empty_destination_paths() {
        let graph = RelationGraph::default();
        let error = write_text(&graph, PathBuf::new().as_path()).unwrap_err();
        assert_eq!(error.to_string(), "destination path is empty");
    }

    fn identity(symbol: &str, kind: HierarchyKind, line: u32) -> SymbolIdentity {
        SymbolIdentity {
            symbol: symbol.to_owned(),
            kind,
            location: Some(SourceLocation {
                uri: "file:///workspace/src/main.rs".to_owned(),
                line: Some(line),
                character: Some(0),
            }),
        }
    }

    fn temporary_workspace(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("cgraph-export-{name}-{unique}"));
        fs::create_dir(&path).unwrap();
        path
    }
}
