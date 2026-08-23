use std::{env, path::PathBuf, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use cgraph::{
    fetch::{
        HierarchyQuery,
        lsp::{LspConfig, LspProvider},
    },
    state::{HierarchyDirection, HierarchyKind, SourceLocation, SymbolIdentity},
};
use tower_lsp::lsp_types::Url;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let program = args.next().context(
        "usage: lsp_hierarchy <SERVER> <call|type> <incoming|outgoing> <SYMBOL> [WORKSPACE]",
    )?;
    let kind = match utf8_arg(args.next(), "hierarchy kind")?.as_str() {
        "call" => HierarchyKind::Call,
        "type" => HierarchyKind::Type,
        value => bail!("unknown hierarchy kind {value:?}; expected call or type"),
    };
    let direction = match utf8_arg(args.next(), "direction")?.as_str() {
        "incoming" => HierarchyDirection::Incoming,
        "outgoing" => HierarchyDirection::Outgoing,
        value => bail!("unknown direction {value:?}; expected incoming or outgoing"),
    };
    let symbol = utf8_arg(args.next(), "symbol")?;
    let workspace_root = match args.next() {
        Some(path) => PathBuf::from(path),
        None => env::current_dir().context("failed to determine current directory")?,
    };
    let location = match args.next() {
        Some(file) => {
            let file = PathBuf::from(file)
                .canonicalize()
                .context("failed to resolve source file")?;
            let line = utf8_arg(args.next(), "one-based line")?
                .parse::<u32>()
                .context("line must be a positive integer")?;
            let character = utf8_arg(args.next(), "one-based character")?
                .parse::<u32>()
                .context("character must be a positive integer")?;
            if line == 0 || character == 0 {
                bail!("line and character are one-based and must be positive");
            }
            let uri = Url::from_file_path(&file)
                .map_err(|()| anyhow!("source file cannot be represented as a file URI"))?;
            Some(SourceLocation {
                uri: uri.to_string(),
                line: Some(line - 1),
                character: Some(character - 1),
            })
        }
        None => None,
    };

    let lsp = LspProvider::start(LspConfig::new(program, workspace_root)).await?;
    tokio::time::sleep(Duration::from_secs(12)).await;
    let response = lsp
        .hierarchy_client()
        .query(HierarchyQuery {
            symbol: SymbolIdentity {
                symbol,
                kind,
                location,
            },
            direction,
        })
        .await;

    let shutdown_result = lsp.shutdown().await;
    let response = response?;
    for child in response.children {
        let location = child.location.map_or_else(
            || "unknown location".to_owned(),
            |location| {
                format!(
                    "{}:{}:{}",
                    location.uri,
                    location.line.unwrap_or(0) + 1,
                    location.character.unwrap_or(0) + 1
                )
            },
        );
        println!("{}\t{location}", child.symbol);
    }
    shutdown_result
}

fn utf8_arg(value: Option<std::ffi::OsString>, name: &str) -> Result<String> {
    value
        .with_context(|| format!("missing {name}"))?
        .into_string()
        .map_err(|_| anyhow!("{name} must be valid UTF-8"))
}
