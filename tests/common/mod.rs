//! Shared helpers for integration tests.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A fresh, unique directory under the system temp dir. Removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("tasks-test-{label}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Newline-delimited JSON-RPC messages for a minimal MCP session.
pub fn initialize_request() -> &'static str {
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#
}

pub fn initialized_notification() -> &'static str {
    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
}

pub fn tool_call(id: u32, name: &str, arguments: serde_json::Value) -> String {
    serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": {"name": name, "arguments": arguments}
    })
    .to_string()
}

/// The text of the first content block of a tools/call result, parsed as JSON.
pub fn tool_payload(response: &serde_json::Value) -> serde_json::Value {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content in {response}"));
    serde_json::from_str(text).unwrap_or(serde_json::Value::String(text.to_string()))
}

// --- the background service ------------------------------------------------

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// A real `dagr serve` in its own temp directory. Stopped on drop.
pub struct Service {
    pub dir: TempDir,
    pub socket: PathBuf,
    child: Child,
}

impl Service {
    /// A service with the MCP endpoint switched off, so tests never fight over
    /// a TCP port or collide with a real service on the default one.
    pub fn start(label: &str) -> Self {
        Self::spawn(label, None)
    }

    /// A service with the MCP endpoint on a free port, ready to answer.
    pub fn start_with_mcp(label: &str) -> Self {
        let port = free_port();
        let service = Self::spawn(label, Some(port));
        service.wait_for_mcp_url();
        service
    }

    /// A service with the label feature already switched on, since it is off
    /// by default and the setting has to be in place before the first write.
    pub fn start_with_labels(label: &str) -> Self {
        Self::spawn_with(label, None, true, false)
    }

    /// The same for lists.
    pub fn start_with_lists(label: &str) -> Self {
        Self::spawn_with(label, None, false, true)
    }

    fn spawn(label: &str, mcp_port: Option<u16>) -> Self {
        Self::spawn_with(label, mcp_port, false, false)
    }

    fn spawn_with(
        label: &str,
        mcp_port: Option<u16>,
        labels_enabled: bool,
        lists_enabled: bool,
    ) -> Self {
        let dir = TempDir::new(label);
        let socket = dir.0.join("run").join("dagr.sock");
        let data = dir.0.join("data");
        std::fs::create_dir_all(&data).unwrap();

        // Settle the MCP settings before the service opens the database, so it
        // never briefly listens on the default port.
        {
            let db = dagr::db::Db::open_at(&data.join("dagr").join("dagr.db")).unwrap();
            let mut settings = db.settings().unwrap();
            settings.mcp_http_enabled = mcp_port.is_some();
            if let Some(port) = mcp_port {
                settings.mcp_http_port = port;
            }
            settings.labels_enabled = labels_enabled;
            settings.lists_enabled = lists_enabled;
            db.save_settings(&settings).unwrap();
        }

        let child = Command::new(env!("CARGO_BIN_EXE_dagr"))
            .arg("serve")
            .env("XDG_DATA_HOME", &data)
            .env("DAGR_SOCKET", &socket)
            .spawn()
            .expect("spawn dagr serve");

        let service = Self { dir, socket, child };
        service.wait_for_socket();
        service
    }

    /// The MCP url from the status file, once the service has bound the port.
    pub fn mcp_url(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.info_path()).ok()?;
        let info: serde_json::Value = serde_json::from_str(&text).ok()?;
        info["mcp_http"].as_str().map(str::to_string)
    }

    pub fn mcp_port(&self) -> u16 {
        self.mcp_url()
            .expect("no MCP url")
            .trim_start_matches("http://127.0.0.1:")
            .trim_end_matches("/mcp")
            .parse()
            .expect("port in url")
    }

    fn wait_for_mcp_url(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.mcp_url().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("service never reported an MCP url");
    }

    /// Starts a second service against the same socket and waits for it to
    /// give up, returning its exit status.
    pub fn start_rival(&self) -> std::process::ExitStatus {
        Command::new(env!("CARGO_BIN_EXE_dagr"))
            .arg("serve")
            .env("XDG_DATA_HOME", self.dir.0.join("data"))
            .env("DAGR_SOCKET", &self.socket)
            .output()
            .expect("spawn rival")
            .status
    }

    pub fn db_path(&self) -> PathBuf {
        self.dir.0.join("data").join("dagr").join("dagr.db")
    }

    pub fn info_path(&self) -> PathBuf {
        self.socket.with_file_name("dagr.json")
    }

    pub fn connect(&self) -> Client {
        let stream = UnixStream::connect(&self.socket).expect("connect to service");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        Client {
            reader: BufReader::new(stream.try_clone().unwrap()),
            stream,
            next_id: 0,
        }
    }

    /// SIGKILL, so the cleanup path does *not* run and the socket and status
    /// file are left behind — what a Flatpak stop does.
    pub fn kill_hard(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // The listener dies with the process; give the kernel a moment.
        std::thread::sleep(Duration::from_millis(100));
    }

    /// Runs `dagr status` against this service's paths.
    pub fn run_status(&self) -> StatusRun {
        let out = Command::new(env!("CARGO_BIN_EXE_dagr"))
            .arg("status")
            .env("XDG_DATA_HOME", self.dir.0.join("data"))
            .env("DAGR_SOCKET", &self.socket)
            .output()
            .expect("run dagr status");
        StatusRun {
            status: out.status,
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        }
    }

    /// SIGTERM, then wait, so the cleanup path is what runs.
    pub fn stop(&mut self) {
        // SAFETY: a pid we spawned and have not reaped.
        unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        let _ = self.child.wait();
    }

    fn wait_for_socket(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.socket.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("service never created {}", self.socket.display());
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One connection, speaking newline-delimited JSON.
pub struct Client {
    reader: BufReader<UnixStream>,
    stream: UnixStream,
    next_id: u64,
}

impl Client {
    /// The next line, whatever it is.
    pub fn read(&mut self) -> serde_json::Value {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line).expect("read from service");
        assert!(n > 0, "service closed the connection");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad line {line:?}: {e}"))
    }

    pub fn send(&mut self, method: &str, params: serde_json::Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let request = serde_json::json!({"id": id, "method": method, "params": params});
        writeln!(self.stream, "{request}").expect("write to service");
        id
    }

    /// Sends, then reads past any events until the matching reply arrives.
    pub fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.send(method, params);
        loop {
            let message = self.read();
            if message["id"].as_u64() == Some(id) {
                return message;
            }
        }
    }

    /// Like `request`, but insists the call succeeded and returns the result.
    pub fn ok(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let reply = self.request(method, params);
        assert_eq!(reply["ok"], true, "call to {method} failed: {reply}");
        reply["result"].clone()
    }

    /// Reads until an event of this name shows up.
    pub fn wait_for_event(&mut self, name: &str) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let message = self.read();
            if message["event"] == name {
                return message;
            }
        }
        panic!("no {name} event arrived");
    }
}

/// A port nothing is listening on right now.
pub fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub struct StatusRun {
    pub status: std::process::ExitStatus,
    pub stdout: String,
}
