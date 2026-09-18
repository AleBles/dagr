//! Where the service keeps its socket and its status file.
//!
//! All three live in one directory so a development instance can be moved
//! aside wholesale by setting `DAGR_SOCKET`.

use std::path::PathBuf;

/// The socket clients connect to. `$DAGR_SOCKET` overrides it entirely, which
/// is how a `cargo run` instance stays out of the way of the installed one.
pub fn socket() -> PathBuf {
    if let Some(path) = std::env::var_os("DAGR_SOCKET") {
        return PathBuf::from(path);
    }
    runtime_dir().join("dagr.sock")
}

/// The directory holding the socket, status file and lock.
pub fn dir() -> PathBuf {
    socket()
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(runtime_dir)
}

/// What the service is: pid, protocol, which database, the MCP url. Written
/// on startup, removed on a clean exit.
pub fn info() -> PathBuf {
    dir().join("dagr.json")
}

/// Held open for the process lifetime so only one service runs at a time.
pub fn lock() -> PathBuf {
    dir().join("dagr.lock")
}

fn runtime_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return dir.join("dagr");
        }
    }
    // Falls back the way most user services do, then to /tmp so that a
    // session without a runtime directory still works.
    let uid = unsafe { libc::getuid() };
    let run = PathBuf::from(format!("/run/user/{uid}"));
    if run.is_dir() {
        run.join("dagr")
    } else {
        PathBuf::from(format!("/tmp/dagr-{uid}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_socket_override_moves_everything_together() {
        // One test, because these read process-wide environment variables.
        std::env::set_var("DAGR_SOCKET", "/tmp/somewhere/else.sock");
        assert_eq!(socket(), PathBuf::from("/tmp/somewhere/else.sock"));
        assert_eq!(dir(), PathBuf::from("/tmp/somewhere"));
        assert_eq!(info(), PathBuf::from("/tmp/somewhere/dagr.json"));
        assert_eq!(lock(), PathBuf::from("/tmp/somewhere/dagr.lock"));

        std::env::remove_var("DAGR_SOCKET");
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1234");
        assert_eq!(socket(), PathBuf::from("/run/user/1234/dagr/dagr.sock"));
        assert_eq!(info(), PathBuf::from("/run/user/1234/dagr/dagr.json"));
    }
}
