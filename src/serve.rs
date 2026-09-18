//! The background service: one socket, one database connection, many clients.
//!
//! It exists so the DankMaterialShell launcher plugin has something to talk to
//! that is not the database file, and so the MCP endpoint outlives the window.
//!
//! The window still opens the database directly, which is why this watches
//! `PRAGMA data_version` on a timer: that pragma only moves when *another*
//! connection commits, so it is exactly the "somebody else changed something"
//! signal we need. Our own writes do not move it, so mutating handlers
//! announce themselves.

use std::fs::{self, File, OpenOptions, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::api::{self, Api};
use crate::db::Db;
use crate::mcp_http;
use crate::paths;
use crate::proto::{line, Event, Method, Request, Response, PROTOCOL};

/// How often we look for writes made by the window or any other connection.
const WATCH_INTERVAL: Duration = Duration::from_millis(250);

/// Runs until SIGTERM or SIGINT. Returns `Ok` if another service already owns
/// the socket, so racing starts are harmless.
pub fn run() -> Result<()> {
    let dir = paths::dir();
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    // The socket carries full control of your task list, so keep the whole
    // directory to ourselves rather than relying on the inherited mode.
    fs::set_permissions(&dir, Permissions::from_mode(0o700))
        .with_context(|| format!("securing {}", dir.display()))?;

    // Held for the lifetime of the process. The kernel drops it even if we are
    // killed, so a stale lock file is never a problem.
    let Some(_lock) = take_lock(&paths::lock())? else {
        match fs::read_to_string(paths::info()) {
            Ok(info) => print!("{info}"),
            Err(_) => println!("another dagr service already owns this socket"),
        }
        return Ok(());
    };

    let db_path = Db::default_path();
    let db = Db::open()?;

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(serve(db, db_path))
}

async fn serve(db: Db, db_path: PathBuf) -> Result<()> {
    let socket = paths::socket();
    // A leftover socket from a crash refuses connections rather than hanging,
    // but it still blocks bind, so clear it. The lock above means we cannot be
    // removing a live one.
    let _ = fs::remove_file(&socket);
    let listener =
        UnixListener::bind(&socket).with_context(|| format!("binding {}", socket.display()))?;
    fs::set_permissions(&socket, Permissions::from_mode(0o600))?;
    write_info(&db_path, None, None)?;

    let version = AtomicI64::new(db.data_version().unwrap_or_default());
    let state = Arc::new(State {
        db: Mutex::new(db),
        db_path,
        events: broadcast::channel(64).0,
        version,
    });

    watch(Arc::clone(&state));

    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    tokio::spawn(handle(stream, Arc::clone(&state)));
                }
                Err(err) => eprintln!("dagr: accept failed: {err}"),
            },
            _ = term.recv() => break,
            _ = int.recv() => break,
        }
    }

    let _ = fs::remove_file(&socket);
    let _ = fs::remove_file(paths::info());
    Ok(())
}

struct State {
    db: Mutex<Db>,
    db_path: PathBuf,
    events: broadcast::Sender<Event>,
    /// Last `data_version` we have told subscribers about.
    version: AtomicI64,
}

/// One timer, two jobs: spotting commits made outside the socket, and keeping
/// the MCP server in step with the settings.
fn watch(state: Arc<State>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(WATCH_INTERVAL);
        let mut mcp: Option<Mcp> = None;
        loop {
            tick.tick().await;
            announce_outside_writes(&state);
            sync_mcp(&state, &mut mcp).await;
        }
    });
}

/// The window writes straight to the database, so poll for its commits.
fn announce_outside_writes(state: &State) {
    let current = match state.db.lock() {
        Ok(db) => db.data_version().ok(),
        Err(_) => None,
    };
    if let Some(current) = current {
        if state.version.swap(current, Ordering::Relaxed) != current {
            let _ = state.events.send(Event::Changed { version: current });
        }
    }
}

/// The MCP server as it currently stands. `cancel` is `None` when the port
/// could not be claimed, which stops us retrying four times a second: the
/// settings have to change before we try again, and Preferences shows why.
struct Mcp {
    port: u16,
    cancel: Option<CancellationToken>,
}

async fn sync_mcp(state: &Arc<State>, mcp: &mut Option<Mcp>) {
    let wanted = wanted_port(state);
    if wanted == mcp.as_ref().map(|m| m.port) {
        return;
    }
    if let Some(previous) = mcp.take() {
        if let Some(cancel) = previous.cancel {
            cancel.cancel();
        }
    }

    let Some(port) = wanted else {
        let _ = write_info(&state.db_path, None, None);
        return;
    };

    match mcp_http::bind(port).await {
        Ok(listener) => {
            let bound = listener.local_addr().map(|a| a.port()).unwrap_or(port);
            let cancel = CancellationToken::new();
            let db_path = state.db_path.clone();
            let token = cancel.clone();
            tokio::spawn(async move {
                if let Err(err) = mcp_http::serve(listener, db_path, token).await {
                    eprintln!("dagr: MCP server stopped: {err:#}");
                }
            });
            *mcp = Some(Mcp {
                port,
                cancel: Some(cancel),
            });
            let _ = write_info(&state.db_path, Some(&mcp_http::url_for(bound)), None);
        }
        Err(err) => {
            *mcp = Some(Mcp { port, cancel: None });
            let _ = write_info(&state.db_path, None, Some(&format!("{err:#}")));
        }
    }
}

/// The port the settings ask for, or `None` when the endpoint is switched off.
fn wanted_port(state: &State) -> Option<u16> {
    let db = state.db.lock().ok()?;
    let settings = db.settings().ok()?;
    settings.mcp_http_enabled.then_some(settings.mcp_http_port)
}

async fn handle(stream: UnixStream, state: Arc<State>) {
    let (read, mut write) = stream.into_split();

    // Replies and events share one outbound queue, so a subscription can never
    // interleave halfway through a reply.
    let (out, mut queued) = mpsc::channel::<String>(64);
    let writer = tokio::spawn(async move {
        while let Some(text) = queued.recv().await {
            if write.write_all(text.as_bytes()).await.is_err() {
                break;
            }
        }
    });

    let hello = Event::Hello {
        protocol: PROTOCOL,
        version: env!("CARGO_PKG_VERSION").to_string(),
        db: state.db_path.display().to_string(),
        pid: std::process::id(),
    };
    if out.send(line(&hello)).await.is_err() {
        return;
    }

    let mut lines = BufReader::new(read).lines();
    let mut subscribed = false;
    while let Ok(Some(text)) = lines.next_line().await {
        if text.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Request>(&text) {
            Ok(request) => {
                if matches!(request.method, Method::Subscribe) && !subscribed {
                    subscribed = true;
                    forward_events(state.events.subscribe(), out.clone());
                }
                dispatch(request, &state)
            }
            // Without a parsed request there is no id to answer under; 0 is
            // never a real one, and the message says what went wrong.
            Err(err) => Response::err(0, "invalid_params", format!("bad request: {err}")),
        };
        if out.send(line(&reply)).await.is_err() {
            break;
        }
    }

    drop(out);
    let _ = writer.await;
}

fn forward_events(mut events: broadcast::Receiver<Event>, out: mpsc::Sender<String>) {
    tokio::spawn(async move {
        loop {
            let message = match events.recv().await {
                Ok(event) => line(&event),
                // Dropped events rather than a broken client: tell it to start
                // over instead of closing the connection under it.
                Err(broadcast::error::RecvError::Lagged(_)) => line(&Event::Resync),
                Err(broadcast::error::RecvError::Closed) => break,
            };
            if out.send(message).await.is_err() {
                break;
            }
        }
    });
}

fn dispatch(request: Request, state: &State) -> Response {
    let id = request.id;
    let Ok(db) = state.db.lock() else {
        return Response::err(id, "internal", "database lock poisoned");
    };
    let api = Api::new(&db);

    // `mutated` drives the change event: our own commits do not move
    // `data_version`, so the watcher above will never see them.
    let (reply, mutated) = match request.method {
        Method::ListTasks(p) => (
            encode(id, api.snapshot(p.include_done.unwrap_or(true))),
            false,
        ),
        Method::AddTask(p) => (encode(id, api.add_task(p)), true),
        Method::UpdateTask(p) => (encode(id, api.update_task(p)), true),
        Method::DeleteTask(p) => (
            encode(id, api.delete_task(p).map(|()| serde_json::json!({}))),
            true,
        ),
        Method::ClearCompleted => (encode(id, api.clear_completed()), true),
        Method::GetSettings => (encode(id, api.get_settings()), false),
        Method::UpdateSettings(p) => (encode(id, api.update_settings(p)), true),
        Method::ListPriorities => (encode(id, api.list_priorities()), false),
        Method::AddPriority(p) => (encode(id, api.add_priority(p)), true),
        Method::UpdatePriority(p) => (encode(id, api.update_priority(p)), true),
        Method::ReorderPriorities(p) => (encode(id, api.reorder_priorities(p)), true),
        Method::DeletePriority(p) => (encode(id, api.delete_priority(p)), true),
        Method::Subscribe => (
            Response::ok(id, serde_json::json!({"subscribed": true})),
            false,
        ),
        Method::Ping => (Response::ok(id, serde_json::json!({"pong": true})), false),
    };

    if mutated && reply.ok {
        let current = db.data_version().unwrap_or_default();
        state.version.store(current, Ordering::Relaxed);
        let _ = state.events.send(Event::Changed { version: current });
    }
    reply
}

fn encode<T: Serialize>(id: u64, result: api::Result<T>) -> Response {
    match result {
        Ok(value) => match serde_json::to_value(value) {
            Ok(value) => Response::ok(id, value),
            Err(err) => Response::err(id, "internal", format!("could not encode result: {err}")),
        },
        Err(err) => Response::from_api(id, err),
    }
}

// --- installing ------------------------------------------------------------

/// How a shell reaches this binary.
///
/// This matters more than it looks: a Flatpak install puts *nothing* called
/// `dagr` on your PATH, so instructions that just say "run `dagr serve`" are
/// wrong for exactly the people most likely to be reading them.
enum Reached {
    Flatpak { id: String },
    Native { exe: PathBuf },
}

fn reached() -> Reached {
    match std::env::var("FLATPAK_ID") {
        Ok(id) if !id.is_empty() => Reached::Flatpak { id },
        _ => Reached::Native {
            exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("dagr")),
        },
    }
}

/// The MCP url the settings currently ask for, so printed instructions match
/// the port actually in use.
fn mcp_url() -> String {
    Db::open()
        .ok()
        .and_then(|db| db.settings().ok())
        .map(|s| s.mcp_http_url())
        .unwrap_or_else(|| {
            crate::mcp_http::url_for(crate::settings::Settings::default().mcp_http_port)
        })
}

/// The command a user types to get the setup instructions, which differs
/// between installs. Shown in Preferences, where "run `dagr setup`" would be
/// useless advice to anyone on a Flatpak.
pub fn setup_command() -> String {
    match reached() {
        Reached::Flatpak { id } => format!("flatpak run {id} setup"),
        Reached::Native { .. } => "dagr setup".to_string(),
    }
}

/// `dagr setup`: everything needed to get the service running at login, spelled
/// out for whichever install this is.
pub fn setup() -> Result<()> {
    let url = mcp_url();
    match reached() {
        Reached::Flatpak { id } => print!(
            "\
Dagr is running from a Flatpak, which puts no `dagr` command on your PATH.

1. Put one there. Every other instruction, and the plugin's docs, assume it:

     mkdir -p ~/.local/bin
     printf '#!/bin/sh\\nexec flatpak run {id} \"$@\"\\n' > ~/.local/bin/dagr
     chmod +x ~/.local/bin/dagr

   (~/.local/bin is on PATH by default on most distributions.)

2. Start the background service with your session, so the MCP endpoint and the
   socket are there without opening the window:

     mkdir -p ~/.config/systemd/user
     flatpak run {id} serve --print-unit > ~/.config/systemd/user/dagr.service
     systemctl --user enable --now dagr

3. Point an AI client at the MCP endpoint:

     claude mcp add --transport http dagr {url}

Check it worked with:  dagr status
"
        ),
        Reached::Native { exe } => print!(
            "\
1. Start the background service with your session, so the MCP endpoint and the
   socket are there without opening the window:

     mkdir -p ~/.config/systemd/user
     {exe} serve --print-unit > ~/.config/systemd/user/dagr.service
     systemctl --user enable --now dagr

2. Point an AI client at the MCP endpoint:

     claude mcp add --transport http dagr {url}

Check it worked with:  dagr status
",
            exe = exe.display()
        ),
    }
    Ok(())
}

/// Prints a systemd user unit pointing at however this binary is reached, so
/// the service (and with it the MCP endpoint) starts with the session.
///
/// Socket activation would be neater but does not work here: `flatpak run`
/// does not pass listening file descriptors into the sandbox.
///
/// `--die-with-parent` is not optional on the Flatpak side. Without it,
/// `systemctl --user stop` kills the `flatpak run` wrapper while the sandboxed
/// process carries on holding the lock and the socket: systemd reports the
/// service stopped, a restart finds the lock taken and exits 0, and an
/// orphaned copy quietly keeps serving.
pub fn print_unit() -> Result<()> {
    let exec = match reached() {
        Reached::Flatpak { id } => {
            format!("/usr/bin/flatpak run --die-with-parent --command=dagr {id} serve")
        }
        Reached::Native { exe } => format!("{} serve", exe.display()),
    };
    print!(
        "\
[Unit]
Description=Dagr background service
Documentation=https://dagr.bles.nu

[Service]
Type=simple
ExecStart={exec}
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
"
    );
    Ok(())
}

// --- status ----------------------------------------------------------------

/// `dagr status`: what is running, and which database it serves.
///
/// Goes through `info()` rather than reading the file, so a status file left
/// behind by a hard kill reports honestly instead of describing a service that
/// stopped. A Flatpak stop always leaves one: `--die-with-parent` kills the
/// sandboxed process outright, so the cleanup path never runs.
pub fn status() -> Result<()> {
    match info() {
        Some(info) => {
            println!("{}", serde_json::to_string_pretty(&info)?);
            Ok(())
        }
        None => {
            println!(
                "no dagr service is answering on {}",
                paths::socket().display()
            );
            match reached() {
                Reached::Flatpak { id } => {
                    println!("start one with: flatpak run {id} serve");
                    println!("to start it at login:  flatpak run {id} setup");
                }
                Reached::Native { .. } => {
                    println!("start one with: dagr serve");
                    println!("to start it at login:  dagr setup");
                }
            }
            std::process::exit(1);
        }
    }
}

// --- helpers ---------------------------------------------------------------

/// `None` when another process already holds the lock.
fn take_lock(path: &Path) -> Result<Option<File>> {
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    // SAFETY: a valid fd we own, and flock has no other preconditions.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(file));
    }
    let err = std::io::Error::last_os_error();
    match err.kind() {
        std::io::ErrorKind::WouldBlock => Ok(None),
        _ => Err(err).with_context(|| format!("locking {}", path.display())),
    }
}

fn write_info(db_path: &Path, mcp_http: Option<&str>, mcp_error: Option<&str>) -> Result<()> {
    let info = serde_json::json!({
        "pid": std::process::id(),
        "protocol": PROTOCOL,
        "version": env!("CARGO_PKG_VERSION"),
        "db": db_path.display().to_string(),
        "socket": paths::socket().display().to_string(),
        "mcp_http": mcp_http,
        "mcp_error": mcp_error,
    });
    let path = paths::info();
    fs::write(&path, serde_json::to_string_pretty(&info)? + "\n")
        .with_context(|| format!("writing {}", path.display()))?;
    fs::set_permissions(&path, Permissions::from_mode(0o600))?;
    Ok(())
}

// --- what the window asks about ---------------------------------------------

/// The running service's status file, or `None` if nothing is answering.
///
/// A file left behind by a crash is ignored, but *not* by checking the pid it
/// names. A service inside a Flatpak has its own pid namespace and writes a
/// sandbox-local pid — often 2, which on the host is `kthreadd`. Signalling it
/// answers a question nobody asked: `kill(2, 0)` returns EPERM, so a pid check
/// calls a perfectly healthy service dead, and the window then starts a second
/// one on every refresh.
///
/// Whether the socket accepts a connection is the real question, and it means
/// the same thing from inside or outside a sandbox.
pub fn info() -> Option<serde_json::Value> {
    info_at(&paths::socket(), &paths::info())
}

fn info_at(socket: &Path, info: &Path) -> Option<serde_json::Value> {
    let text = fs::read_to_string(info).ok()?;
    let parsed = serde_json::from_str(&text).ok()?;
    // A stale socket file refuses connections rather than hanging, so this
    // fails fast without needing a timeout.
    std::os::unix::net::UnixStream::connect(socket).ok()?;
    Some(parsed)
}

/// What Preferences shows about the MCP endpoint.
pub fn mcp_status() -> crate::mcp_http::Status {
    use crate::mcp_http::Status;
    let Some(info) = info() else {
        return Status::Stopped;
    };
    if let Some(url) = info["mcp_http"].as_str() {
        Status::Running {
            url: url.to_string(),
        }
    } else if let Some(message) = info["mcp_error"].as_str() {
        Status::Failed(message.to_string())
    } else {
        Status::Stopped
    }
}

/// Starts a service if none is running, so switching the endpoint on in
/// Preferences works before anyone has set up the systemd unit. Detached on
/// purpose: it has to outlive the window.
pub fn ensure_running() -> Result<()> {
    if info().is_some() {
        return Ok(());
    }
    let exe = std::env::current_exe().context("finding our own binary")?;
    std::process::Command::new(exe)
        .arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("starting the background service")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The case a pid check got wrong: the status file is still there, but
    /// nothing is listening. Before this was a socket probe, a Flatpak service
    /// reported dead while healthy, and a dead one could report alive.
    #[test]
    fn a_status_file_with_no_live_socket_reports_nothing() {
        let dir = std::env::temp_dir().join(format!("dagr-info-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let info = dir.join("dagr.json");
        let socket = dir.join("dagr.sock");
        fs::write(&info, r#"{"pid":2,"db":"/tmp/x.db"}"#).unwrap();

        // No socket at all.
        assert!(info_at(&socket, &info).is_none());

        // A leftover socket file that nothing is listening on.
        fs::write(&socket, "").unwrap();
        assert!(info_at(&socket, &info).is_none());

        // Something actually listening.
        fs::remove_file(&socket).unwrap();
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert_eq!(info_at(&socket, &info).unwrap()["db"], "/tmp/x.db");

        let _ = fs::remove_dir_all(&dir);
    }
}
