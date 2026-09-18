//! The MCP server's transport: Streamable HTTP on loopback.
//!
//! This used to run on a thread of its own with a private tokio runtime,
//! because the window had no runtime to lend it. The background service does,
//! so this is now just two async functions it drives.
//!
//! Each HTTP session gets its own connection to the database, which is why
//! only the path is passed in rather than an open `Db`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::db::Db;
use crate::mcp::TasksServer;

/// What the window shows in Preferences. Read from the service's status file,
/// so it is accurate whether or not this process is the one serving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Stopped,
    Running { url: String },
    Failed(String),
}

pub fn url_for(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

/// Claims the port. Separate from `serve` so the caller learns the real port
/// — and any bind failure — before anything starts listening.
pub async fn bind(port: u16) -> Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("port {port}"))
}

/// Serves until `cancel` is triggered.
pub async fn serve(
    listener: TcpListener,
    db_path: PathBuf,
    cancel: CancellationToken,
) -> Result<()> {
    let port = listener.local_addr()?.port();

    let mut config = StreamableHttpServerConfig::default()
        // Host validation (loopback only) is on by default; also refuse
        // browser pages from other origins, in case one probes localhost.
        .with_allowed_origins([
            format!("http://127.0.0.1:{port}"),
            format!("http://localhost:{port}"),
        ]);
    config.cancellation_token = cancel.clone();

    let service = StreamableHttpService::new(
        move || {
            Db::open_at(&db_path)
                .map(TasksServer::new)
                .map_err(std::io::Error::other)
        },
        LocalSessionManager::default().into(),
        config,
    );
    let app = axum::Router::new().nest_service("/mcp", service);

    tokio::select! {
        result = axum::serve(listener, app) => result.context("serving MCP over HTTP"),
        _ = cancel.cancelled() => Ok(()),
    }
}
