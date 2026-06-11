//! MCP stdio transport — JSON-RPC 2.0 line-oriented reader/writer (spec §11.1).
//!
//! Each message is a single JSON object on one line, terminated by newline.
//! All diagnostic output goes to stderr via tracing; stdout is reserved for
//! JSON-RPC messages only.

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

/// Maximum line size for JSON-RPC messages over stdio (1 MiB).
const MAX_LINE_BYTES: u64 = 1_048_576;

/// Stdio transport for MCP JSON-RPC communication.
///
/// Uses `tokio::sync::Mutex` for thread-safe access to stdin/stdout.
pub struct McpTransport {
    stdin: Mutex<BufReader<tokio::io::Stdin>>,
    stdout: Mutex<tokio::io::Stdout>,
}

impl McpTransport {
    /// Create a new transport using process stdin and stdout.
    pub fn new() -> Self {
        Self {
            stdin: Mutex::new(BufReader::new(tokio::io::stdin())),
            stdout: Mutex::new(tokio::io::stdout()),
        }
    }

    /// Read a single line from stdin.
    ///
    /// Returns `None` when stdin reaches EOF (client disconnected).
    /// Strips trailing newline/carriage-return. Skips empty lines.
    pub async fn read_line(&self) -> Result<Option<String>> {
        let mut stdin = self.stdin.lock().await;
        loop {
            let mut line = String::new();
            let bytes_read = stdin.read_line(&mut line).await?;
            if bytes_read == 0 {
                return Ok(None); // EOF
            }
            if bytes_read as u64 > MAX_LINE_BYTES {
                return Err(anyhow::anyhow!(
                    "JSON-RPC line exceeds maximum size of {} bytes",
                    MAX_LINE_BYTES
                ));
            }
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            if trimmed.is_empty() {
                continue; // Skip empty lines
            }
            tracing::debug!("← {}", trimmed);
            return Ok(Some(trimmed.to_string()));
        }
    }

    /// Write a serializable message to stdout as a single JSON line.
    ///
    /// Flushes stdout after each write to ensure the client receives it.
    pub async fn write_message(&self, msg: &impl serde::Serialize) -> Result<()> {
        let json = serde_json::to_string(msg)?;
        tracing::debug!("→ {}", json);
        let mut stdout = self.stdout.lock().await;
        stdout.write_all(json.as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
        Ok(())
    }
}

impl Default for McpTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_transport_can_be_created() {
        // Just verify the constructor works without panicking
        let _transport = McpTransport::new();
    }

    #[test]
    fn mcp_transport_default_works() {
        let _transport = McpTransport::default();
    }

    #[tokio::test]
    async fn write_message_serializes_and_writes() {
        // We can't easily test stdout in unit tests, but we can verify
        // that the serialization works without error.
        let transport = McpTransport::new();
        let msg = serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": {}});
        // This will write to actual stdout — that's OK in tests
        let result = transport.write_message(&msg).await;
        assert!(result.is_ok());
    }
}
