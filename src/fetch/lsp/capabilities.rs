use std::{
    collections::HashMap,
    ffi::OsStr,
    sync::{Mutex, atomic::AtomicBool},
};

use serde_json::{Value, json};
use tower_lsp::lsp_types::{
    CallHierarchyClientCapabilities, CallHierarchyServerCapability, ClientCapabilities,
    DocumentSymbolClientCapabilities, GeneralClientCapabilities, InitializeResult, OneOf,
    PositionEncodingKind, TextDocumentClientCapabilities, TypeHierarchyClientCapabilities,
    WindowClientCapabilities, WorkspaceClientCapabilities, WorkspaceSymbolClientCapabilities,
    request::{CallHierarchyPrepare, Request, TypeHierarchyPrepare},
};

use crate::state::HierarchyKind;

use super::profile::{
    ServerProfile, from_name as server_profile_from_name,
    from_program as server_profile_from_program,
};

#[derive(Debug, Default)]
pub(super) struct ServerHierarchyCapabilities {
    static_call: AtomicBool,
    dynamic_registrations: Mutex<HashMap<String, String>>,
}

impl ServerHierarchyCapabilities {
    pub(super) fn set_static_call(&self, supported: bool) {
        self.static_call
            .store(supported, std::sync::atomic::Ordering::Release);
    }

    pub(super) fn supports(&self, kind: HierarchyKind) -> bool {
        if kind == HierarchyKind::Call
            && self.static_call.load(std::sync::atomic::Ordering::Acquire)
        {
            return true;
        }
        let method = prepare_hierarchy_method(kind);
        self.dynamic_registrations
            .lock()
            .expect("LSP hierarchy capability mutex poisoned")
            .values()
            .any(|registered| registered == method)
    }

    pub(super) fn register(&self, id: &str, method: &str) {
        if is_hierarchy_registration(method) {
            self.dynamic_registrations
                .lock()
                .expect("LSP hierarchy capability mutex poisoned")
                .insert(id.to_owned(), method.to_owned());
        }
    }

    pub(super) fn unregister(&self, id: &str) {
        self.dynamic_registrations
            .lock()
            .expect("LSP hierarchy capability mutex poisoned")
            .remove(id);
    }
}

pub(super) fn hierarchy_name(kind: HierarchyKind) -> &'static str {
    match kind {
        HierarchyKind::Call => "call",
        HierarchyKind::Type => "type",
    }
}

fn prepare_hierarchy_method(kind: HierarchyKind) -> &'static str {
    match kind {
        HierarchyKind::Call => CallHierarchyPrepare::METHOD,
        HierarchyKind::Type => TypeHierarchyPrepare::METHOD,
    }
}

fn is_hierarchy_registration(method: &str) -> bool {
    method == CallHierarchyPrepare::METHOD || method == TypeHierarchyPrepare::METHOD
}

pub(super) fn client_capabilities() -> ClientCapabilities {
    ClientCapabilities {
        text_document: Some(TextDocumentClientCapabilities {
            call_hierarchy: Some(CallHierarchyClientCapabilities {
                dynamic_registration: Some(true),
            }),
            document_symbol: Some(DocumentSymbolClientCapabilities::default()),
            type_hierarchy: Some(TypeHierarchyClientCapabilities {
                dynamic_registration: Some(true),
            }),
            ..TextDocumentClientCapabilities::default()
        }),
        workspace: Some(WorkspaceClientCapabilities {
            symbol: Some(WorkspaceSymbolClientCapabilities::default()),
            workspace_folders: Some(true),
            configuration: Some(true),
            ..WorkspaceClientCapabilities::default()
        }),
        window: Some(WindowClientCapabilities {
            work_done_progress: Some(true),
            ..WindowClientCapabilities::default()
        }),
        general: Some(GeneralClientCapabilities {
            position_encodings: Some(vec![PositionEncodingKind::UTF16]),
            ..GeneralClientCapabilities::default()
        }),
        experimental: Some(json!({
            "serverStatusNotification": true,
        })),
    }
}

pub(super) fn uses_utf16_positions(position_encoding: Option<&PositionEncodingKind>) -> bool {
    position_encoding.is_none_or(|encoding| encoding == &PositionEncodingKind::UTF16)
}

pub(super) fn requested_configuration(section: Option<&str>) -> Value {
    match section {
        Some("rust-analyzer") => json!({
            "workspace": {
                "symbol": {
                    "search": {
                        "kind": "all_symbols",
                        "scope": "workspace",
                    }
                }
            }
        }),
        Some("rust-analyzer.workspace.symbol.search.kind") => json!("all_symbols"),
        Some("rust-analyzer.workspace.symbol.search.scope") => json!("workspace"),
        Some("python") => json!({}),
        _ => Value::Null,
    }
}

pub(super) fn workspace_symbol_supported(initialize_result: &InitializeResult) -> bool {
    match &initialize_result.capabilities.workspace_symbol_provider {
        Some(OneOf::Left(supported)) => *supported,
        Some(OneOf::Right(_)) => true,
        None => false,
    }
}

pub(super) fn call_hierarchy_supported(initialize_result: &InitializeResult) -> bool {
    match initialize_result.capabilities.call_hierarchy_provider {
        Some(CallHierarchyServerCapability::Simple(supported)) => supported,
        Some(CallHierarchyServerCapability::Options(_)) => true,
        None => false,
    }
}

pub(super) fn workspace_symbol_initialization_options(
    program: &OsStr,
    server_name: Option<&str>,
    options: Option<Value>,
) -> Option<Value> {
    let profile = server_name
        .map(server_profile_from_name)
        .unwrap_or_else(|| server_profile_from_program(program));
    if profile != ServerProfile::RustAnalyzer {
        return options;
    }

    let mut options = options.unwrap_or_else(|| json!({}));
    merge_json(
        &mut options,
        json!({
            "workspace": {
                "symbol": {
                    "search": {
                        "kind": "all_symbols",
                        "scope": "workspace",
                    }
                }
            }
        }),
    );
    Some(options)
}

fn merge_json(target: &mut Value, overlay: Value) {
    match (target, overlay) {
        (Value::Object(target), Value::Object(overlay)) => {
            for (key, value) in overlay {
                merge_json(target.entry(key).or_insert(Value::Null), value);
            }
        }
        (target, overlay) => *target = overlay,
    }
}
