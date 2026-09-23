//! Where the service keeps its socket and its status file.
//!
//! On Linux that directory is under `$XDG_RUNTIME_DIR`; macOS has no such
//! thing, so there it is under the per-user `$TMPDIR`.
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

#[cfg(not(target_os = "macos"))]
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

/// macOS has no runtime directory; the per-user temporary directory is the
/// closest thing, private to the user and cleaned at reboot.
#[cfg(target_os = "macos")]
fn runtime_dir() -> PathBuf {
    runtime_dir_in(std::env::var_os("TMPDIR"))
}

/// Split out so the test need not change TMPDIR under every other test.
#[cfg(target_os = "macos")]
fn runtime_dir_in(tmpdir: Option<std::ffi::OsString>) -> PathBuf {
    if let Some(dir) = tmpdir {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return dir.join("dagr");
        }
    }
    // A launchd agent is not promised a TMPDIR, but the window and the agent
    // must agree on the socket, so ask the system where TMPDIR would point.
    if let Some(dir) = darwin_user_temp_dir() {
        return dir.join("dagr");
    }
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/dagr-{uid}"))
}

#[cfg(target_os = "macos")]
fn darwin_user_temp_dir() -> Option<PathBuf> {
    use std::ffi::CStr;
    let mut buf = [0 as libc::c_char; libc::PATH_MAX as usize];
    // SAFETY: confstr writes at most buf.len() bytes, NUL-terminated.
    let len = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr(), buf.len()) };
    if len == 0 || len > buf.len() {
        return None;
    }
    let dir = unsafe { CStr::from_ptr(buf.as_ptr()) };
    Some(PathBuf::from(dir.to_str().ok()?))
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
        #[cfg(not(target_os = "macos"))]
        {
            std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1234");
            assert_eq!(socket(), PathBuf::from("/run/user/1234/dagr/dagr.sock"));
            assert_eq!(info(), PathBuf::from("/run/user/1234/dagr/dagr.json"));
        }
        #[cfg(target_os = "macos")]
        assert_eq!(
            runtime_dir_in(Some("/var/folders/x/T/".into())),
            PathBuf::from("/var/folders/x/T/dagr")
        );
    }
}
