//! Golden tests for the socket wire format.
//!
//! These literal strings are the contract the DankMaterialShell plugin is
//! written against. If a rename or a serde attribute changes the shape, this
//! fails here in Rust rather than silently in QML.

use dagr::proto::{Event, Method, Request, Response, PROTOCOL};

fn parse(line: &str) -> Request {
    serde_json::from_str(line).unwrap_or_else(|e| panic!("could not parse {line}: {e}"))
}

#[test]
fn parses_a_request_with_params() {
    let r = parse(r#"{"id":1,"method":"list_tasks","params":{"include_done":false}}"#);
    assert_eq!(r.id, 1);
    match r.method {
        Method::ListTasks(p) => assert_eq!(p.include_done, Some(false)),
        other => panic!("wrong method: {other:?}"),
    }
}

#[test]
fn parses_the_add_task_the_plugin_sends() {
    let r = parse(r#"{"id":7,"method":"add_task","params":{"title":"!! pay rent"}}"#);
    match r.method {
        Method::AddTask(p) => {
            assert_eq!(p.title, "!! pay rent");
            // The plugin passes the query through untouched; the `!!` is read
            // later by the api, not by the plugin.
            assert!(p.priority.is_none());
        }
        other => panic!("wrong method: {other:?}"),
    }
}

#[test]
fn parses_the_done_toggle_the_plugin_sends() {
    let r = parse(r#"{"id":8,"method":"update_task","params":{"id":42,"done":true}}"#);
    match r.method {
        Method::UpdateTask(p) => {
            assert_eq!(p.id, 42);
            assert_eq!(p.done, Some(true));
            assert!(p.title.is_none());
        }
        other => panic!("wrong method: {other:?}"),
    }
}

#[test]
fn parses_the_label_methods() {
    assert!(matches!(
        parse(r#"{"id":9,"method":"list_labels"}"#).method,
        Method::ListLabels
    ));
    match parse(r#"{"id":10,"method":"add_label","params":{"name":"work"}}"#).method {
        Method::AddLabel(p) => {
            assert_eq!(p.name, "work");
            assert!(p.color.is_none());
        }
        other => panic!("wrong method: {other:?}"),
    }
    match parse(r#"{"id":11,"method":"delete_label","params":{"id":3}}"#).method {
        Method::DeleteLabel(p) => assert_eq!(p.id, 3),
        other => panic!("wrong method: {other:?}"),
    }
}

#[test]
fn parses_labels_on_a_task_update() {
    let r =
        parse(r#"{"id":12,"method":"update_task","params":{"id":42,"labels":["work","home"]}}"#);
    match r.method {
        Method::UpdateTask(p) => {
            assert_eq!(p.id, 42);
            assert_eq!(p.labels, Some(vec!["work".to_string(), "home".to_string()]));
            assert!(p.done.is_none());
        }
        other => panic!("wrong method: {other:?}"),
    }
}

#[test]
fn parses_the_list_methods() {
    assert!(matches!(
        parse(r#"{"id":13,"method":"list_lists"}"#).method,
        Method::ListLists
    ));
    match parse(r#"{"id":14,"method":"add_list","params":{"name":"Work"}}"#).method {
        Method::AddList(p) => assert_eq!(p.name, "Work"),
        other => panic!("wrong method: {other:?}"),
    }
    match parse(r#"{"id":15,"method":"delete_list","params":{"id":2,"move_to":"Tasks"}}"#).method {
        Method::DeleteList(p) => {
            assert_eq!(p.id, 2);
            assert_eq!(p.move_to.as_deref(), Some("Tasks"));
        }
        other => panic!("wrong method: {other:?}"),
    }
    match parse(r#"{"id":16,"method":"reorder_lists","params":{"ids":[3,1]}}"#).method {
        Method::ReorderLists(p) => assert_eq!(p.ids, vec![3, 1]),
        other => panic!("wrong method: {other:?}"),
    }
}

#[test]
fn parses_methods_that_take_no_params() {
    assert!(matches!(
        parse(r#"{"id":2,"method":"subscribe"}"#).method,
        Method::Subscribe
    ));
    assert!(matches!(
        parse(r#"{"id":3,"method":"ping"}"#).method,
        Method::Ping
    ));
    assert!(matches!(
        parse(r#"{"id":4,"method":"clear_completed"}"#).method,
        Method::ClearCompleted
    ));
}

#[test]
fn rejects_an_unknown_method() {
    let bad = serde_json::from_str::<Request>(r#"{"id":1,"method":"drop_database"}"#);
    assert!(bad.is_err());
}

#[test]
fn renders_a_successful_reply() {
    let line = dagr::proto::line(&Response::ok(1, serde_json::json!({"version": 12})));
    assert_eq!(line, "{\"id\":1,\"ok\":true,\"result\":{\"version\":12}}\n");
}

#[test]
fn renders_a_failed_reply_without_a_result_key() {
    let line = dagr::proto::line(&Response::err(
        2,
        "invalid_params",
        "title must not be empty",
    ));
    assert_eq!(
        line,
        "{\"id\":2,\"ok\":false,\"error\":{\"kind\":\"invalid_params\",\
         \"message\":\"title must not be empty\"}}\n"
    );
}

#[test]
fn renders_the_events() {
    let hello = dagr::proto::line(&Event::Hello {
        protocol: PROTOCOL,
        version: "0.2.0".into(),
        db: "/home/x/dagr.db".into(),
        pid: 99,
    });
    assert_eq!(
        hello,
        "{\"event\":\"hello\",\"protocol\":1,\"version\":\"0.2.0\",\
         \"db\":\"/home/x/dagr.db\",\"pid\":99}\n"
    );

    assert_eq!(
        dagr::proto::line(&Event::Changed { version: 1236 }),
        "{\"event\":\"changed\",\"version\":1236}\n"
    );
    assert_eq!(
        dagr::proto::line(&Event::Resync),
        "{\"event\":\"resync\"}\n"
    );
}

#[test]
fn every_line_is_exactly_one_line() {
    // A title containing a newline must not break framing.
    let reply = Response::ok(1, serde_json::json!({ "title": "two\nlines" }));
    let line = dagr::proto::line(&reply);
    assert_eq!(line.matches('\n').count(), 1);
    assert!(line.ends_with('\n'));
}

#[test]
fn is_forgiving_about_the_params_key() {
    // Methods whose fields are all optional may omit `params` entirely...
    assert!(matches!(
        parse(r#"{"id":1,"method":"list_tasks"}"#).method,
        Method::ListTasks(_)
    ));
    assert!(matches!(
        parse(r#"{"id":1,"method":"list_tasks","params":null}"#).method,
        Method::ListTasks(_)
    ));
    // ...and methods taking none may send an empty object anyway.
    assert!(matches!(
        parse(r#"{"id":2,"method":"subscribe","params":{}}"#).method,
        Method::Subscribe
    ));
}

#[test]
fn still_reports_the_real_problem_with_bad_params() {
    let err = serde_json::from_str::<Request>(r#"{"id":1,"method":"add_task","params":{}}"#)
        .expect_err("add_task needs a title");
    assert!(err.to_string().contains("title"), "unhelpful error: {err}");
}
