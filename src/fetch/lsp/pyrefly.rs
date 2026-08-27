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
}

pub(super) fn bootstrap_document(
    workspace_root: &Path,
    file_extensions: &[String],
) -> Option<BootstrapDocument> {
    let path = first_python_source(workspace_root, file_extensions)?;
    let text = fs::read_to_string(&path).ok()?;
    let uri = Url::from_file_path(path).ok()?;
    Some(BootstrapDocument { uri, text })
}

fn first_python_source(workspace_root: &Path, file_extensions: &[String]) -> Option<PathBuf> {
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
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        file_extensions
                            .iter()
                            .any(|configured| extension.eq_ignore_ascii_case(configured))
                    })
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

fn ignored_directory(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "target" | "node_modules" | "__pycache__" | "venv" | ".venv"
        )
}
