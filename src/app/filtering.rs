use std::path::Path;

use tower_lsp::lsp_types::Url;

use crate::{config::FilterConfig, state::SourceLocation};

pub(super) fn candidate_is_visible(
    symbol: &str,
    location: Option<&SourceLocation>,
    filters: &FilterConfig,
    workspace: &Path,
) -> bool {
    let Some(location) = location else {
        return !filters.is_ignored(Some(symbol), None, workspace);
    };
    let Ok(uri) = Url::parse(&location.uri) else {
        return true;
    };
    uri.to_file_path()
        .map(|path| filters.is_visible_symbol_path(symbol, &path, workspace))
        .unwrap_or(!filters.workspace_only())
}
