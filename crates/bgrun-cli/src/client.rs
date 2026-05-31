use std::path::Path;

use anyhow::{Context, Result};
use bgrun_proto::{Command, Request, Response};
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// A client for communicating with the bgrun daemon over a Unix socket.
pub struct DaemonClient {
    reader: BufReader<tokio::io::ReadHalf<UnixStream>>,
    writer: tokio::io::WriteHalf<UnixStream>,
}

impl DaemonClient {
    /// Connects to the daemon at the given socket path.
    pub async fn connect(socket_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path)
            .await
            .context("failed to connect to daemon socket")?;
        let (reader, writer) = tokio::io::split(stream);
        Ok(DaemonClient {
            reader: BufReader::new(reader),
            writer,
        })
    }

    /// Sends a command to the daemon and returns the parsed response.
    pub async fn send<T: DeserializeOwned>(&mut self, command: Command) -> Result<Response<T>> {
        let request = Request {
            id: uuid::Uuid::new_v4().to_string(),
            command,
        };

        let json = serde_json::to_string(&request)?;
        self.writer.write_all(json.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;

        let mut buf = Vec::new();
        self.reader.read_until(b'\n', &mut buf).await?;

        let response: Response<T> = serde_json::from_slice(&buf)?;
        Ok(response)
    }
}
