use std::{fs, path::Path, sync::atomic::Ordering, time::Duration};

use anyhow::{Context, Result};
use tower_lsp::lsp_types::{
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, TextDocumentIdentifier,
    TextDocumentItem, Url,
};

use super::JsonRpcClient;

impl JsonRpcClient {
    pub(super) async fn mark_document_open(&self, uri: &Url) {
        self.opened_documents.lock().await.insert(uri.clone());
    }

    pub(super) fn enable_document_opening(&self) {
        self.auto_open_documents.store(true, Ordering::Release);
    }

    pub(super) async fn ensure_document_open(&self, uri: &Url) -> Result<()> {
        if !self.auto_open_documents.load(Ordering::Acquire) || uri.scheme() != "file" {
            return Ok(());
        }
        let mut opened_documents = self.opened_documents.lock().await;
        if opened_documents.contains(uri) {
            return Ok(());
        }

        let path = uri
            .to_file_path()
            .map_err(|()| anyhow::anyhow!("cannot open non-file URI {uri}"))?;
        let Some(language_id) = language_id_for_path(&path) else {
            return Ok(());
        };
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read LSP document {}", path.display()))?;
        self.notify(
            "textDocument/didOpen",
            DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: language_id.to_owned(),
                    version: 0,
                    text,
                },
            },
        )
        .await
        .with_context(|| format!("failed to open LSP document {uri}"))?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        opened_documents.insert(uri.clone());
        Ok(())
    }

    pub(super) async fn close_open_documents(&self) -> Result<()> {
        let documents = self
            .opened_documents
            .lock()
            .await
            .drain()
            .collect::<Vec<_>>();
        let mut first_error = None;
        for uri in documents {
            if let Err(error) = self
                .notify(
                    "textDocument/didClose",
                    DidCloseTextDocumentParams {
                        text_document: TextDocumentIdentifier::new(uri),
                    },
                )
                .await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

fn language_id_for_path(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("rs") => Some("rust"),
        Some("py") | Some("pyi") => Some("python"),
        Some("c") => Some("c"),
        Some("cc") | Some("cpp") | Some("cxx") | Some("h") | Some("hh") | Some("hpp")
        | Some("hxx") | Some("ixx") | Some("cppm") => Some("cpp"),
        _ => None,
    }
}
