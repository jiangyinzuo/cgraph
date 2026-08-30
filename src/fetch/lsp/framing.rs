use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

// A corrupt Content-Length must not turn into an attacker-controlled allocation.
const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

pub(super) async fn read_message<R>(reader: &mut R) -> Result<Value>
where
    R: AsyncBufRead + Unpin,
{
    let mut content_length = None;

    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).await? == 0 {
            bail!("language server closed its output stream");
        }
        let header = header.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }

        let Some((name, value)) = header.split_once(':') else {
            bail!("malformed LSP header: {header:?}");
        };
        if name.eq_ignore_ascii_case("Content-Length") {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .context("invalid LSP Content-Length header")?,
            );
        }
    }

    let content_length = content_length.context("LSP message has no Content-Length header")?;
    if content_length > MAX_MESSAGE_SIZE {
        bail!("LSP message is too large: {content_length} bytes (limit: {MAX_MESSAGE_SIZE} bytes)");
    }

    let mut body = vec![0; content_length];
    reader
        .read_exact(&mut body)
        .await
        .context("language server closed its output stream mid-message")?;
    serde_json::from_slice(&body).context("language server sent invalid JSON")
}

pub(super) async fn write_message<W>(writer: &mut W, message: &Value) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let body = serde_json::to_vec(message).context("failed to encode LSP message")?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}
