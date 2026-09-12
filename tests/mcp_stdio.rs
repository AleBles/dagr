//! End-to-end: spawn the real binary in `--mcp` mode and talk JSON-RPC to it
//! over stdin/stdout, exactly like an MCP client would.

mod common;

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use common::*;
use serde_json::{json, Value};

/// Runs one stdio session: sends every line, closes stdin, collects responses by id.
fn session(data_home: &std::path::Path, messages: &[String]) -> HashMap<u64, Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dagr"))
        .arg("--mcp")
        .env("XDG_DATA_HOME", data_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn dagr --mcp");

    {
        let mut stdin = child.stdin.take().unwrap();
        for m in messages {
            writeln!(stdin, "{m}").unwrap();
        }
        // Dropping stdin ends the session; the server then exits.
    }

    // Watchdog so a hung server fails the test instead of the whole CI job.
    let pid = child.id();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(30));
        let _ = Command::new("kill").arg(pid.to_string()).status();
    });

    let output = child.wait_with_output().expect("wait for dagr --mcp");
    assert!(
        output.status.success(),
        "server exited with {}",
        output.status
    );

    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).expect("valid JSON line"))
        .filter_map(|v| v["id"].as_u64().map(|id| (id, v)))
        .collect()
}

#[test]
fn stdio_server_lists_tools_and_round_trips_a_task() {
    let dir = TempDir::new("stdio");
    let responses = session(
        &dir.0,
        &[
            initialize_request().to_string(),
            initialized_notification().to_string(),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#.to_string(),
            tool_call(
                3,
                "add_task",
                json!({"title": "Ship it", "priority": "high"}),
            ),
            tool_call(4, "list_tasks", json!({"include_done": false})),
            tool_call(5, "add_task", json!({"title": "", "priority": "high"})),
        ],
    );

    let init = &responses[&1]["result"];
    assert_eq!(init["serverInfo"]["name"], "dagr");
    assert!(init["capabilities"]["tools"].is_object());

    let tools: Vec<&str> = responses[&2]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for expected in [
        "add_task",
        "update_task",
        "list_priorities",
        "update_settings",
    ] {
        assert!(tools.contains(&expected), "missing tool {expected}");
    }

    let added = tool_payload(&responses[&3]);
    assert_eq!(added["title"], "Ship it");
    assert_eq!(added["priority"]["name"], "High");

    let listed = tool_payload(&responses[&4]);
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], added["id"]);

    // Bad input surfaces as a JSON-RPC error, not a crash.
    assert_eq!(responses[&5]["error"]["code"], -32602);
}

#[test]
fn data_persists_between_sessions() {
    let dir = TempDir::new("stdio-persist");
    session(
        &dir.0,
        &[
            initialize_request().to_string(),
            initialized_notification().to_string(),
            tool_call(2, "add_task", json!({"title": "Remember me"})),
        ],
    );
    let second = session(
        &dir.0,
        &[
            initialize_request().to_string(),
            initialized_notification().to_string(),
            tool_call(2, "list_tasks", json!({})),
        ],
    );
    let tasks = tool_payload(&second[&2]);
    assert_eq!(tasks[0]["title"], "Remember me");
}
