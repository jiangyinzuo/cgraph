use serde::{Deserialize, Serialize};

use crate::state::{HierarchyKind, SourceLocation};

pub const PROTOCOL_VERSION: u16 = 1;

/// Versioned wrapper independent of the eventual socket framing strategy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Envelope<T> {
    pub version: u16,
    pub request_id: Option<u64>,
    pub payload: T,
}

impl<T> Envelope<T> {
    pub fn new(request_id: Option<u64>, payload: T) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            payload,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcRequest {
    FocusSymbol {
        hierarchy: HierarchyKind,
        symbol: String,
        location: Option<SourceLocation>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcResponse {
    Accepted,
    Error { message: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcEvent {
    /// Opens an LSP source position. Line and UTF-16 character coordinates are
    /// zero-based; editor adapters convert them to their UI coordinate system.
    OpenLocation {
        uri: String,
        line: u32,
        character: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::{Envelope, IpcEvent, IpcRequest, PROTOCOL_VERSION};
    use crate::state::HierarchyKind;

    #[test]
    fn serializes_a_versioned_tagged_request() {
        let message = Envelope::new(
            Some(7),
            IpcRequest::FocusSymbol {
                hierarchy: HierarchyKind::Call,
                symbol: "run".to_owned(),
                location: None,
            },
        );

        let value = serde_json::to_value(message).unwrap();
        assert_eq!(value["version"], PROTOCOL_VERSION);
        assert_eq!(value["request_id"], 7);
        assert_eq!(value["payload"]["type"], "focus_symbol");
    }

    #[test]
    fn serializes_open_location_with_zero_based_coordinates() {
        let message = Envelope::new(
            None,
            IpcEvent::OpenLocation {
                uri: "file:///workspace/main.py".to_owned(),
                line: 4,
                character: 2,
            },
        );

        let encoded = serde_json::to_string(&message).unwrap();
        assert!(!encoded.contains('\n'));
        let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["version"], PROTOCOL_VERSION);
        assert!(value["request_id"].is_null());
        assert_eq!(value["payload"]["type"], "open_location");
        assert_eq!(value["payload"]["line"], 4);
        assert_eq!(value["payload"]["character"], 2);
    }
}
