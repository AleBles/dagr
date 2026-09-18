//! The MCP endpoint, exercised the way a real client reaches it: against a
//! running `dagr serve`, over TCP.
//!
//! The previous version of this test called into the library directly, which
//! meant the path users actually take — service starts, binds, publishes its
//! url — was never covered.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use common::*;

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

#[test]
fn the_service_serves_mcp() {
    let service = Service::start_with_mcp("http");
    let port = service.mcp_port();

    let init = post(port, initialize_request(), &format!("127.0.0.1:{port}"));
    assert!(
        init.starts_with("HTTP/1.1 200"),
        "unexpected response:\n{init}"
    );
    assert!(
        init.contains(r#""name":"dagr""#),
        "no serverInfo in:\n{init}"
    );
}

#[test]
fn refuses_a_forged_host_header() {
    // DNS-rebinding guard: this is what stops a web page in your browser from
    // driving the endpoint, and it matters more now that it is always on.
    let service = Service::start_with_mcp("http-forged");
    let port = service.mcp_port();

    let forged = post(port, initialize_request(), "evil.example");
    assert!(
        forged.starts_with("HTTP/1.1 403"),
        "expected 403, got:\n{forged}"
    );
}

#[test]
fn stops_listening_when_the_service_stops() {
    let mut service = Service::start_with_mcp("http-stop");
    let port = service.mcp_port();
    assert!(TcpStream::connect(("127.0.0.1", port)).is_ok());

    service.stop();

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("port {port} still accepting connections after the service stopped");
}

#[test]
fn reports_a_busy_port_instead_of_retrying_forever() {
    let blocker = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = blocker.local_addr().unwrap().port();

    let service = Service::start("http-busy");
    let mut client = service.connect();
    client.read(); // hello
    client.ok(
        "update_settings",
        serde_json::json!({"mcp_http_enabled": true, "mcp_http_port": port}),
    );

    // The failure lands in the status file, which is what Preferences reads.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let info: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(service.info_path()).unwrap()).unwrap();
        if let Some(message) = info["mcp_error"].as_str() {
            assert!(
                message.contains(&port.to_string()),
                "unhelpful error: {message}"
            );
            assert!(info["mcp_http"].is_null());
            return;
        }
        assert!(Instant::now() < deadline, "no failure reported: {info}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn switching_the_setting_starts_and_stops_the_endpoint() {
    let service = Service::start("http-toggle");
    let mut client = service.connect();
    client.read(); // hello
    assert!(service.mcp_url().is_none(), "should start switched off");

    let port = free_port();
    client.ok(
        "update_settings",
        serde_json::json!({"mcp_http_enabled": true, "mcp_http_port": port}),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while service.mcp_url().is_none() {
        assert!(Instant::now() < deadline, "endpoint never came up");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(TcpStream::connect(("127.0.0.1", port)).is_ok());

    client.ok(
        "update_settings",
        serde_json::json!({"mcp_http_enabled": false}),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while service.mcp_url().is_some() {
        assert!(Instant::now() < deadline, "endpoint never went away");
        std::thread::sleep(Duration::from_millis(50));
    }
}
