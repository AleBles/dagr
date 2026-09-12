//! The optional in-app MCP server over Streamable HTTP.
//!
//! Switched on in Preferences, it runs on a background thread with its own
//! tokio runtime for as long as the window is open, listening on
//! `127.0.0.1:<port>/mcp`. Tools are the same as for `dagr --mcp`; each HTTP
//! session gets its own database connection to the shared file.
//!
//! Status flows back to the window through an `async_channel`, tagged with the
//! port so a late message from a stopped server can be told apart.

use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::Duration;

use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use tokio_util::sync::CancellationToken;

use crate::db::Db;
use crate::mcp::TasksServer;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Stopped,
    Starting,
    Running { url: String },
    Failed(String),
}

/// A running server. Dropping it (or calling `stop`) shuts the server down.
pub struct Handle {
    port: u16,
    cancel: CancellationToken,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn url_for(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

/// Starts the server on a background thread, serving the database at
/// `db_path`. `status` receives `(port, Status)` updates tagged with the
/// *requested* port: `Running` once bound (its URL carries the actual port,
/// which matters when `port` is 0), or `Failed` if the port could not be
/// opened, then `Stopped` when it winds down.
pub fn start(port: u16, db_path: PathBuf, status: async_channel::Sender<(u16, Status)>) -> Handle {
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let thread = std::thread::Builder::new()
        .name(format!("mcp-http:{port}"))
        .spawn(move || run(port, db_path, token, status))
        .ok();
    Handle {
        port,
        cancel,
        thread,
    }
}

fn run(
    port: u16,
    db_path: PathBuf,
    cancel: CancellationToken,
    status: async_channel::Sender<(u16, Status)>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            let _ = status.send_blocking((port, Status::Failed(err.to_string())));
            return;
        }
    };

    runtime.block_on(async {
        let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => listener,
            Err(err) => {
                let _ = status
                    .send((port, Status::Failed(format!("Port {port}: {err}"))))
                    .await;
                return;
            }
        };

        let bound_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);

        let mut config = StreamableHttpServerConfig::default()
            // Host validation (loopback only) is on by default; also refuse
            // browser pages from other origins, in case one probes localhost.
            .with_allowed_origins([
                format!("http://127.0.0.1:{bound_port}"),
                format!("http://localhost:{bound_port}"),
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

        let _ = status
            .send((
                port,
                Status::Running {
                    url: url_for(bound_port),
                },
            ))
            .await;

        tokio::select! {
            result = axum::serve(listener, app) => {
                if let Err(err) = result {
                    let _ = status.send((port, Status::Failed(err.to_string()))).await;
                }
            }
            _ = cancel.cancelled() => {}
        }
        let _ = status.send((port, Status::Stopped)).await;
    });

    // Cut off any lingering connections instead of waiting on them.
    runtime.shutdown_timeout(Duration::from_secs(1));
}
