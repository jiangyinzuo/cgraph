use std::{
    fs,
    path::{Path, PathBuf},
};

use tower_lsp::lsp_types::Url;

const MAX_BOOTSTRAP_FILE_SIZE: u64 = 4 * 1024 * 1024;
const MAX_SCANNED_ENTRIES: usize = 10_000;

pub(super) struct BootstrapDocument {
    pub(super) uri: Url,
    pub(super) text: String,
    pub(super) language_id: &'static str,
}

pub(super) fn bootstrap_document(
    workspace_root: &Path,
    file_extensions: &[String],
) -> Option<BootstrapDocument> {
    let path = first_source(workspace_root, file_extensions)?;
    let text = fs::read_to_string(&path).ok()?;
    let language_id = match path.extension().and_then(|extension| extension.to_str()) {
        Some("c") => "c",
        Some(_) => "cpp",
        None => return None,
    };
    let uri = Url::from_file_path(path).ok()?;
    Some(BootstrapDocument {
        uri,
        text,
        language_id,
    })
}

fn first_source(workspace_root: &Path, file_extensions: &[String]) -> Option<PathBuf> {
    let mut directories = vec![workspace_root.to_path_buf()];
    let mut scanned_entries = 0;
    while let Some(directory) = directories.pop() {
        let mut entries = fs::read_dir(directory)
            .ok()?
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());

        let mut child_directories = Vec::new();
        for entry in entries {
            scanned_entries += 1;
            if scanned_entries > MAX_SCANNED_ENTRIES {
                return None;
            }

            let file_type = entry.file_type().ok()?;
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_file()
                && has_configured_extension(&path, file_extensions)
                && entry.metadata().ok()?.len() <= MAX_BOOTSTRAP_FILE_SIZE
            {
                return path.canonicalize().ok();
            }
            if file_type.is_dir() && !ignored_directory(&entry.file_name().to_string_lossy()) {
                child_directories.push(path);
            }
        }
        child_directories.reverse();
        directories.extend(child_directories);
    }
    None
}

fn has_configured_extension(path: &Path, file_extensions: &[String]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            file_extensions
                .iter()
                .any(|configured| extension.eq_ignore_ascii_case(configured))
        })
}

fn ignored_directory(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "build" | "cmake-build-debug" | "cmake-build-release" | "node_modules" | "target"
        )
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::bootstrap_document;

    #[test]
    fn bootstraps_clangd_from_cpp_headers_and_custom_extensions() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("cgraph-clangd-bootstrap-{unique}"));
        fs::create_dir_all(workspace.join("include")).unwrap();
        fs::write(workspace.join("include/worker.hpp"), "struct Worker {};\n").unwrap();

        let document = bootstrap_document(&workspace, &["hpp".to_owned()]).unwrap();
        assert!(document.uri.path().ends_with("/include/worker.hpp"));
        assert_eq!(document.language_id, "cpp");

        fs::remove_file(workspace.join("include/worker.hpp")).unwrap();
        fs::write(
            workspace.join("include/module.ixx"),
            "export module demo;\n",
        )
        .unwrap();
        assert!(bootstrap_document(&workspace, &["hpp".to_owned()]).is_none());
        let document = bootstrap_document(&workspace, &["ixx".to_owned()]).unwrap();
        assert!(document.uri.path().ends_with("/include/module.ixx"));
        assert_eq!(document.language_id, "cpp");

        fs::remove_dir_all(workspace).unwrap();
    }
}
