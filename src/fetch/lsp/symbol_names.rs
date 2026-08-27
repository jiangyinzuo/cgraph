#![doc = include_str!("README.md")]

use std::ffi::OsStr;

use tower_lsp::lsp_types::SymbolKind;

use super::profile::{
    ServerProfile, from_name as server_profile_from_name,
    from_program as server_profile_from_program,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SymbolNameAdapter {
    Standard,
    RustAnalyzer,
    Pyrefly,
}

impl SymbolNameAdapter {
    pub(super) fn detect(program: &OsStr, server_name: Option<&str>) -> Self {
        match server_name
            .map(server_profile_from_name)
            .unwrap_or_else(|| server_profile_from_program(program))
        {
            ServerProfile::RustAnalyzer => Self::RustAnalyzer,
            ServerProfile::Pyrefly => Self::Pyrefly,
            ServerProfile::Clangd | ServerProfile::Standard => Self::Standard,
        }
    }

    pub(super) fn workspace_symbol(
        self,
        name: &str,
        kind: SymbolKind,
        container_name: Option<&str>,
    ) -> String {
        match self {
            Self::Standard => qualify_callable(name, kind, container_name.and_then(safe_container)),
            Self::RustAnalyzer => rust_callable(name, kind, container_name),
            Self::Pyrefly => python_workspace_callable(name, kind, container_name),
        }
    }

    pub(super) fn uses_document_symbols(self) -> bool {
        self == Self::RustAnalyzer
    }

    pub(super) fn is_pyrefly(self) -> bool {
        self == Self::Pyrefly
    }

    pub(super) fn call_hierarchy_item(
        self,
        name: &str,
        kind: SymbolKind,
        detail: Option<&str>,
        document_container: Option<&str>,
    ) -> String {
        match self {
            // `detail` is deliberately ignored here: unlike workspace-symbol
            // `containerName`, LSP defines it only as arbitrary display text.
            Self::Standard => name.to_owned(),
            Self::RustAnalyzer => {
                let normalized = rust_callable(name, kind, document_container);
                if normalized == name {
                    rust_callable(name, kind, detail)
                } else {
                    normalized
                }
            }
            Self::Pyrefly => pyrefly_hierarchy_callable(name, kind, detail),
        }
    }
}

fn python_workspace_callable(name: &str, kind: SymbolKind, container: Option<&str>) -> String {
    if !matches!(kind, SymbolKind::METHOD | SymbolKind::CONSTRUCTOR) {
        return name.to_owned();
    }

    let short_name = name.rsplit('.').next().unwrap_or(name);
    let owner = container
        .and_then(safe_python_container)
        .or_else(|| name.rsplit_once('.').map(|(owner, _)| owner))
        .and_then(|owner| owner.rsplit('.').next())
        .filter(|owner| is_python_identifier(owner));
    owner.map_or_else(|| name.to_owned(), |owner| format!("{owner}.{short_name}"))
}

fn pyrefly_hierarchy_callable(name: &str, kind: SymbolKind, detail: Option<&str>) -> String {
    if !matches!(kind, SymbolKind::METHOD | SymbolKind::CONSTRUCTOR) {
        return name.to_owned();
    }

    let short_name = name.rsplit('.').next().unwrap_or(name);
    let owner = detail
        .and_then(safe_python_detail_owner)
        .or_else(|| name.rsplit_once('.').map(|(owner, _)| owner))
        .and_then(|owner| owner.rsplit('.').next())
        .filter(|owner| is_python_identifier(owner));
    owner.map_or_else(|| name.to_owned(), |owner| format!("{owner}.{short_name}"))
}

fn safe_python_container(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() || value.contains(['/', '\\', '(', ')', '\n']) {
        return None;
    }
    value
        .rsplit('.')
        .next()
        .filter(|owner| is_python_identifier(owner))
}

fn safe_python_detail_owner(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() || value.contains(['/', '\\', '(', ')', '\n']) {
        return None;
    }
    value
        .rsplit_once('.')
        .map(|(owner, _)| owner)
        .filter(|owner| !owner.is_empty())
}

fn is_python_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_alphabetic())
        && characters.all(|character| character == '_' || character.is_alphanumeric())
}

fn qualify_callable(name: &str, kind: SymbolKind, container: Option<&str>) -> String {
    if !matches!(kind, SymbolKind::METHOD | SymbolKind::CONSTRUCTOR) || name.contains("::") {
        return name.to_owned();
    }
    container.map_or_else(
        || name.to_owned(),
        |container| format!("{container}::{name}"),
    )
}

fn rust_callable(name: &str, kind: SymbolKind, container: Option<&str>) -> String {
    if name.contains("::") {
        return name.to_owned();
    }
    let Some(container_text) = container else {
        return name.to_owned();
    };
    let associated_function =
        matches!(kind, SymbolKind::FUNCTION) && is_rust_owner_description(container_text);
    if !matches!(kind, SymbolKind::METHOD | SymbolKind::CONSTRUCTOR) && !associated_function {
        return name.to_owned();
    }
    rust_container(container_text).map_or_else(
        || name.to_owned(),
        |container| format!("{container}::{name}"),
    )
}

fn is_rust_owner_description(value: &str) -> bool {
    let value = value.trim();
    value.starts_with("impl") || value.starts_with("trait ") || value.starts_with('<')
}

fn safe_container(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()
        && !value.contains(['/', '\\', '(', ')', '\n'])
        && value.split_whitespace().count() == 1)
        .then_some(value)
}

fn rust_container(value: &str) -> Option<&str> {
    let mut value = value.trim();
    if value.is_empty() || value.contains(['/', '\\', '(', ')', '\n']) {
        return None;
    }

    if let Some(rest) = value.strip_prefix("impl") {
        value = strip_impl_generics(rest.trim_start())?;
        if let Some((_, self_type)) = split_top_level(value, " for ") {
            value = self_type;
        }
    } else if let Some(rest) = value.strip_prefix("trait ") {
        value = rest;
    }

    if let Some((self_type, _)) = value
        .strip_prefix('<')
        .and_then(|value| value.strip_suffix('>'))
        .and_then(|value| split_top_level(value, " as "))
    {
        value = self_type;
    }
    if let Some((self_type, _)) = split_top_level(value, " where ") {
        value = self_type;
    }

    let value = value.trim();
    let type_name = last_path_segment(value);
    let type_name = type_name.split('<').next().unwrap_or(type_name).trim();
    is_rust_identifier(type_name).then_some(type_name)
}

fn strip_impl_generics(value: &str) -> Option<&str> {
    if !value.starts_with('<') {
        return Some(value);
    }
    let mut depth = 0_u32;
    for (index, character) in value.char_indices() {
        match character {
            '<' => depth += 1,
            '>' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(value[index + character.len_utf8()..].trim_start());
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level<'a>(value: &'a str, separator: &str) -> Option<(&'a str, &'a str)> {
    let mut angle_depth = 0_u32;
    for (index, character) in value.char_indices() {
        match character {
            '<' => angle_depth += 1,
            '>' => angle_depth = angle_depth.saturating_sub(1),
            _ if angle_depth == 0 && value[index..].starts_with(separator) => {
                return Some((&value[..index], &value[index + separator.len()..]));
            }
            _ => {}
        }
    }
    None
}

fn last_path_segment(value: &str) -> &str {
    let mut angle_depth = 0_u32;
    let mut segment_start = 0;
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'<' => angle_depth += 1,
            b'>' => angle_depth = angle_depth.saturating_sub(1),
            b':' if angle_depth == 0 && bytes.get(index + 1) == Some(&b':') => {
                segment_start = index + 2;
                index += 1;
            }
            _ => {}
        }
        index += 1;
    }
    &value[segment_start..]
}

fn is_rust_identifier(value: &str) -> bool {
    let value = value.strip_prefix("r#").unwrap_or(value);
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_alphabetic())
        && characters.all(|character| character == '_' || character.is_alphanumeric())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use tower_lsp::lsp_types::SymbolKind;

    use super::SymbolNameAdapter;

    #[test]
    fn detects_rust_analyzer_from_program_or_server_name() {
        assert_eq!(
            SymbolNameAdapter::detect(OsStr::new("/tools/rust-analyzer"), None),
            SymbolNameAdapter::RustAnalyzer
        );
        assert_eq!(
            SymbolNameAdapter::detect(OsStr::new("lsp-wrapper"), Some("rust-analyzer")),
            SymbolNameAdapter::RustAnalyzer
        );
        assert_eq!(
            SymbolNameAdapter::detect(OsStr::new("clangd"), Some("clangd")),
            SymbolNameAdapter::Standard
        );
    }

    #[test]
    fn detects_and_qualifies_pyrefly_methods() {
        assert_eq!(
            SymbolNameAdapter::detect(OsStr::new("/tools/pyrefly"), None),
            SymbolNameAdapter::Pyrefly
        );
        assert_eq!(
            SymbolNameAdapter::detect(OsStr::new("lsp-wrapper"), Some("pyrefly-lsp")),
            SymbolNameAdapter::Pyrefly
        );

        let adapter = SymbolNameAdapter::Pyrefly;
        assert_eq!(
            adapter.workspace_symbol("run", SymbolKind::METHOD, Some("worker.Worker")),
            "Worker.run"
        );
        assert_eq!(
            adapter.call_hierarchy_item("run", SymbolKind::METHOD, Some("worker.Worker.run"), None),
            "Worker.run"
        );
        assert_eq!(
            adapter.call_hierarchy_item("run", SymbolKind::FUNCTION, Some("worker.run"), None),
            "run"
        );
        assert_eq!(
            adapter.call_hierarchy_item(
                "run",
                SymbolKind::METHOD,
                Some("/workspace/worker.py"),
                None
            ),
            "run"
        );
    }

    #[test]
    fn qualifies_rust_methods_with_their_impl_type() {
        let adapter = SymbolNameAdapter::RustAnalyzer;
        assert_eq!(
            adapter.workspace_symbol("run", SymbolKind::METHOD, Some("impl worker::Worker")),
            "Worker::run"
        );
        assert_eq!(
            adapter.workspace_symbol("new", SymbolKind::FUNCTION, Some("impl worker::Worker")),
            "Worker::new"
        );
        assert_eq!(
            adapter.call_hierarchy_item(
                "run",
                SymbolKind::FUNCTION,
                Some("pub fn run(&self)"),
                Some("impl worker::Worker")
            ),
            "Worker::run"
        );
        for (detail, expected) in [
            ("impl Worker", "Worker::run"),
            ("impl<T> Worker<T>", "Worker::run"),
            ("impl Job for crate::worker::Worker", "Worker::run"),
            ("impl<T> Job<T> for worker::Worker<T>", "Worker::run"),
            ("<worker::Worker as Job>", "Worker::run"),
            ("trait Job", "Job::run"),
        ] {
            assert_eq!(
                adapter.call_hierarchy_item("run", SymbolKind::FUNCTION, Some(detail), None),
                expected
            );
        }
    }

    #[test]
    fn preserves_names_without_a_safe_rust_type() {
        let adapter = SymbolNameAdapter::RustAnalyzer;
        for detail in [
            None,
            Some("fn run(&self)"),
            Some("/workspace/src/main.rs"),
            Some("impl<T Worker<T>"),
        ] {
            assert_eq!(
                adapter.call_hierarchy_item("run", SymbolKind::METHOD, detail, None),
                "run"
            );
        }
        assert_eq!(
            adapter.call_hierarchy_item(
                "Worker::run",
                SymbolKind::METHOD,
                Some("impl Worker"),
                None
            ),
            "Worker::run"
        );
        assert_eq!(
            adapter.call_hierarchy_item("run", SymbolKind::FUNCTION, Some("fn run(&self)"), None),
            "run"
        );
    }

    #[test]
    fn does_not_interpret_generic_call_hierarchy_detail() {
        let adapter = SymbolNameAdapter::Standard;
        assert_eq!(
            adapter.call_hierarchy_item("run", SymbolKind::METHOD, Some("Service"), None),
            "run"
        );
        assert_eq!(
            adapter.workspace_symbol("run", SymbolKind::METHOD, Some("Service")),
            "Service::run"
        );
    }
}
