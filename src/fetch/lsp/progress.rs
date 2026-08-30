use std::collections::HashMap;

use serde_json::{Value, from_value};
use tokio::sync::mpsc;
use tower_lsp::lsp_types::{NumberOrString, ProgressParams, ProgressParamsValue, WorkDoneProgress};

/// A provider-level status event, kept separate from individual request results.
///
/// Language servers may run several work-done tasks concurrently. The JSON-RPC
/// actor collapses those protocol tokens into the most useful current update;
/// the TUI then maps this LSP-specific type into its backend-neutral status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LspStatusUpdate {
    Ready {
        message: Option<String>,
    },
    Progress {
        title: String,
        message: Option<String>,
        percentage: Option<u32>,
    },
    Warning(String),
    Error(String),
    Disconnected(String),
    Diagnostic(String),
}

#[derive(Clone, Debug)]
struct ActiveProgress {
    sequence: u64,
    title: String,
    message: Option<String>,
    percentage: Option<u32>,
}

#[derive(Default)]
pub(super) struct LspProgressTracker {
    next_sequence: u64,
    active: HashMap<String, ActiveProgress>,
}

pub(super) fn handle_server_notification(
    message: &Value,
    tracker: &mut LspProgressTracker,
    sender: &mpsc::UnboundedSender<LspStatusUpdate>,
) {
    match message.get("method").and_then(Value::as_str) {
        Some("$/progress") => {
            let Some(params) = message.get("params").cloned() else {
                return;
            };
            let Ok(params) = from_value::<ProgressParams>(params) else {
                return;
            };
            let ProgressParamsValue::WorkDone(progress) = params.value;
            tracker.update(params.token, progress, sender);
        }
        Some("experimental/serverStatus") => {
            let Some(params) = message.get("params") else {
                return;
            };
            let health = params.get("health").and_then(Value::as_str).unwrap_or("ok");
            let quiescent = params
                .get("quiescent")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let message = params
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned);

            let update = match health {
                "warning" => LspStatusUpdate::Warning(
                    message.unwrap_or_else(|| "Language server reported a warning".to_owned()),
                ),
                "error" => LspStatusUpdate::Error(
                    message.unwrap_or_else(|| "Language server reported an error".to_owned()),
                ),
                _ if quiescent => {
                    if tracker.emit_latest(sender) {
                        return;
                    }
                    LspStatusUpdate::Ready { message }
                }
                _ => {
                    if tracker.emit_latest(sender) {
                        return;
                    }
                    LspStatusUpdate::Progress {
                        title: "rust-analyzer".to_owned(),
                        message: message.or_else(|| Some("Background work in progress".to_owned())),
                        percentage: None,
                    }
                }
            };
            let _ = sender.send(update);
        }
        _ => {}
    }
}

impl LspProgressTracker {
    fn update(
        &mut self,
        token: NumberOrString,
        progress: WorkDoneProgress,
        sender: &mpsc::UnboundedSender<LspStatusUpdate>,
    ) {
        let token = progress_token_key(token);
        self.next_sequence = self.next_sequence.wrapping_add(1);
        match progress {
            WorkDoneProgress::Begin(progress) => {
                self.active.insert(
                    token,
                    ActiveProgress {
                        sequence: self.next_sequence,
                        title: progress.title,
                        message: progress.message,
                        percentage: progress.percentage,
                    },
                );
                self.emit_latest(sender);
            }
            WorkDoneProgress::Report(progress) => {
                if let Some(active) = self.active.get_mut(&token) {
                    active.sequence = self.next_sequence;
                    if progress.message.is_some() {
                        active.message = progress.message;
                    }
                    if progress.percentage.is_some() {
                        active.percentage = progress.percentage;
                    }
                    self.emit_latest(sender);
                }
            }
            WorkDoneProgress::End(progress) => {
                self.active.remove(&token);
                if !self.emit_latest(sender) {
                    let _ = sender.send(LspStatusUpdate::Ready {
                        message: progress.message,
                    });
                }
            }
        }
    }

    fn emit_latest(&self, sender: &mpsc::UnboundedSender<LspStatusUpdate>) -> bool {
        let Some(progress) = self
            .active
            .values()
            .max_by_key(|progress| progress.sequence)
        else {
            return false;
        };
        let _ = sender.send(LspStatusUpdate::Progress {
            title: progress.title.clone(),
            message: progress.message.clone(),
            percentage: progress.percentage,
        });
        true
    }
}

fn progress_token_key(token: NumberOrString) -> String {
    match token {
        NumberOrString::Number(number) => format!("number:{number}"),
        NumberOrString::String(string) => format!("string:{string}"),
    }
}
