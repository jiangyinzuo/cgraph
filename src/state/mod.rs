#![doc = include_str!("README.md")]

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

pub mod graph;

static NEXT_NODE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NodeId(pub u64);

impl NodeId {
    pub(crate) fn next() -> Self {
        Self(NEXT_NODE_ID.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct SourceLocation {
    pub uri: String,
    pub line: Option<u32>,
    pub character: Option<u32>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HierarchyKind {
    Call,
    Type,
}

/// Stable semantic identity shared by cache entries and duplicate node instances.
///
/// `NodeId` cannot fill this role: the product deliberately allows one symbol to
/// appear multiple times on the canvas, and every occurrence needs its own id.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SymbolIdentity {
    pub symbol: String,
    pub kind: HierarchyKind,
    pub location: Option<SourceLocation>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HierarchyDirection {
    Incoming,
    Outgoing,
}

/// Translation from unbounded canvas coordinates to terminal coordinates.
///
/// This intentionally contains no Ratatui `Rect`: layout is a view concern and
/// the state layer must remain usable by IPC and headless tests.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Viewport {
    pub offset_x: i32,
    pub offset_y: i32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LoadState {
    #[default]
    NotLoaded,
    Loading,
    Loaded,
    Failed,
}
