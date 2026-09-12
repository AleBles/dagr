//! Boots the in-app HTTP MCP server on a random port, performs the MCP
//! handshake over a raw TCP socket, and checks it shuts down cleanly.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use common::*;
use dagr::mcp_http::{self, Status};

/// One HTTP/1.1 POST to `/mcp`; returns the full raw response.
fn post(port: u16, body: &str, host: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "POST /mcp HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
}

fn wait_for_running(rx: &async_channel::Receiver<(u16, Status)>) -> u16 {
    loop {
        let (_, status) = rx
            .recv_blocking()
            .expect("status channel closed before Running");
        match status {
            Status::Running { url } => {
                return url
                    .trim_start_matches("http://127.0.0.1:")
                    .trim_end_matches("/mcp")
                    .parse()
                    .expect("port in url");
            }
            Status::Failed(err) => panic!("server failed to start: {err}"),
            Status::Starting | Status::Stopped => {}
        }
    }
}

#[test]
fn http_server_serves_mcp_and_stops_on_drop() {
    let dir = TempDir::new("http");
    let (tx, rx) = async_channel::unbounded();
    let handle = mcp_http::start(0, dir.0.join("dagr.db"), tx);
    let port = wait_for_running(&rx);

    let init = post(port, initialize_request(), &format!("127.0.0.1:{port}"));
    assert!(
        init.starts_with("HTTP/1.1 200"),
        "unexpected response:\n{init}"
    );
    assert!(
        init.contains(r#""name":"dagr""#),
        "no serverInfo in:\n{init}"
    );

    // DNS-rebinding guard: a foreign Host header is refused.
    let forged = post(port, initialize_request(), "evil.example");
    assert!(
        forged.starts_with("HTTP/1.1 403"),
        "expected 403, got:\n{forged}"
    );

    handle.stop();
    // Give the runtime a moment to release the socket, then it must be closed.
    let mut closed = false;
    for _ in 0..50 {
        if TcpStream::connect(("127.0.0.1", port)).is_err() {
            closed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        closed,
        "port {port} still accepting connections after stop()"
    );
}

#[test]
fn http_server_reports_a_busy_port() {
    let blocker = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = blocker.local_addr().unwrap().port();
    let dir = TempDir::new("http-busy");
    let (tx, rx) = async_channel::unbounded();
    let _handle = mcp_http::start(port, dir.0.join("dagr.db"), tx);
    let (reported_port, status) = rx.recv_blocking().unwrap();
    assert_eq!(reported_port, port);
    assert!(matches!(status, Status::Failed(_)), "got {status:?}");
}
