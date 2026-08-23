use std::{env, path::PathBuf, time::Duration};

use anyhow::{Context, Result, anyhow};
use ctree::fetch::lsp::{LspConfig, LspProvider};

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let program = args
        .next()
        .context("usage: lsp_workspace_symbols <SERVER> <QUERY> [WORKSPACE]")?;
    let query = args
        .next()
        .context("usage: lsp_workspace_symbols <SERVER> <QUERY> [WORKSPACE]")?
        .into_string()
        .map_err(|_| anyhow!("query must be valid UTF-8"))?;
    let workspace_root = match args.next() {
        Some(path) => PathBuf::from(path),
        None => env::current_dir().context("failed to determine current directory")?,
    };

    let lsp = LspProvider::start(LspConfig::new(program, workspace_root)).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let symbols = lsp.workspace_symbols(&query).await?;

    for symbol in symbols {
        let line = symbol
            .range
            .map(|range| (range.start.line + 1).to_string())
            .unwrap_or_else(|| "?".to_owned());
        println!("{}\t{}:{line}", symbol.name, symbol.uri);
    }

    lsp.shutdown().await
}
