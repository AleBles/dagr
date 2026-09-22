//! The background service, driven the way the launcher plugin drives it:
//! a real `dagr serve` process and a real Unix socket.

mod common;

use common::*;
use dagr::db::Db;

#[test]
fn announces_itself_on_connect() {
    let service = Service::start("hello");
    let mut client = service.connect();

    let hello = client.read();
    assert_eq!(hello["event"], "hello");
    assert_eq!(hello["protocol"], 1);
    // Which database is being served: the answer to having a dev build and a
    // flatpak build on the same machine.
    assert_eq!(hello["db"], service.db_path().display().to_string());
    assert!(hello["pid"].as_u64().unwrap() > 0);
}

#[test]
fn adds_and_lists_a_task() {
    let service = Service::start("roundtrip");
    let mut client = service.connect();
    client.read(); // hello

    let added = client.ok("add_task", serde_json::json!({"title": "!! pay rent"}));
    // The `!!` is read by the service, not the caller: this is what lets the
    // launcher pass its query straight through. Each `!` climbs one level from
    // the default, which starts at the lowest: None -> Low -> Medium.
    assert_eq!(added["title"], "pay rent");
    assert_eq!(added["priority"]["name"], "Medium");

    let listed = client.ok("list_tasks", serde_json::json!({"include_done": false}));
    assert_eq!(listed["tasks"].as_array().unwrap().len(), 1);
    // One round trip gives the plugin everything it needs to draw a row.
    assert_eq!(listed["priorities"].as_array().unwrap().len(), 4);
    assert_eq!(listed["settings"]["priorities_enabled"], true);
    assert!(listed["version"].is_i64());

    let id = added["id"].as_i64().unwrap();
    let done = client.ok("update_task", serde_json::json!({"id": id, "done": true}));
    assert_eq!(done["done"], true);
    assert_eq!(
        client.ok("list_tasks", serde_json::json!({"include_done": false}))["tasks"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn reports_bad_input_without_dropping_the_connection() {
    let service = Service::start("errors");
    let mut client = service.connect();
    client.read(); // hello

    let reply = client.request("add_task", serde_json::json!({"title": "   "}));
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["kind"], "invalid_params");
    assert!(reply["result"].is_null(), "no result key on a failure");

    // Still usable afterwards.
    assert_eq!(client.ok("ping", serde_json::json!({}))["pong"], true);
}

#[test]
fn date_first_travels_over_the_socket() {
    // The socket cannot backdate a task, so this covers the plumbing; the
    // day maths itself is proved by the unit tests in `db.rs`.
    let service = Service::start("date_first");
    let mut client = service.connect();
    client.read(); // hello

    assert_eq!(
        client.ok("get_settings", serde_json::json!({}))["date_first"],
        false
    );

    let updated = client.ok("update_settings", serde_json::json!({"date_first": true}));
    assert_eq!(updated["date_first"], true);
    // A patch leaves everything it does not name alone.
    assert_eq!(updated["sort_order"], "priority_oldest");

    // The window reads the same database, so it orders by the same rule.
    let db = Db::open_at(&service.db_path()).expect("open the database directly");
    assert!(db.settings().unwrap().date_first);

    // And the launcher sees it in the one round trip it makes.
    assert_eq!(
        client.ok("list_tasks", serde_json::json!({}))["settings"]["date_first"],
        true
    );
}

#[test]
fn labels_travel_over_the_socket() {
    let service = Service::start_with_labels("labels");
    let mut client = service.connect();
    client.read(); // hello

    let added = client.ok("add_label", serde_json::json!({"name": "work"}));
    assert_eq!(added["name"], "work");
    assert!(added["color"].as_str().unwrap().starts_with('#'));

    // A leading #tag is read by the service, exactly like `!`, so the launcher
    // can pass a typed query straight through.
    let task = client.ok(
        "add_task",
        serde_json::json!({"title": "#work #q4 pay rent"}),
    );
    assert_eq!(task["title"], "pay rent");
    assert_eq!(task["labels"][0]["name"], "q4");
    assert_eq!(task["labels"][1]["name"], "work");
    let id = task["id"].as_i64().unwrap();

    // One round trip carries the catalogue as well as the assignments.
    let listed = client.ok("list_tasks", serde_json::json!({}));
    assert_eq!(listed["labels"].as_array().unwrap().len(), 2);
    assert_eq!(listed["settings"]["labels_enabled"], true);
    assert_eq!(listed["tasks"][0]["labels"].as_array().unwrap().len(), 2);

    // The window reads the same database.
    let db = Db::open_at(&service.db_path()).expect("open the database directly");
    assert_eq!(db.load_labels().unwrap().len(), 2);

    // Replacing the set with nothing drops the key from the row entirely.
    let cleared = client.ok("update_task", serde_json::json!({"id": id, "labels": []}));
    assert!(
        cleared.get("labels").is_none(),
        "empty labels should be omitted"
    );

    let left = client.ok("delete_label", serde_json::json!({"id": added["id"]}));
    assert_eq!(left.as_array().unwrap().len(), 1);
}

#[test]
fn labels_stay_out_of_the_way_until_asked_for() {
    let service = Service::start("no-labels");
    let mut client = service.connect();
    client.read(); // hello

    let listed = client.ok("list_tasks", serde_json::json!({}));
    assert_eq!(listed["settings"]["labels_enabled"], false);
    assert!(listed["labels"].as_array().unwrap().is_empty());

    // With the feature off a '#' is part of the title, and the row carries no
    // labels key at all - the wire looks exactly as it did before labels.
    let task = client.ok("add_task", serde_json::json!({"title": "#work pay rent"}));
    assert_eq!(task["title"], "#work pay rent");
    assert!(task.get("labels").is_none());
}

#[test]
fn lists_travel_over_the_socket() {
    let service = Service::start_with_lists("lists");
    let mut client = service.connect();
    client.read(); // hello

    // A fresh database already has the one list every task lives in.
    let listed = client.ok("list_tasks", serde_json::json!({}));
    assert_eq!(listed["lists"].as_array().unwrap().len(), 1);
    assert_eq!(listed["settings"]["lists_enabled"], true);

    let work = client.ok("add_list", serde_json::json!({"name": "Work"}));
    assert_eq!(work["name"], "Work");
    let work_id = work["id"].as_i64().unwrap();

    // A leading @list files the task, exactly like `!` and `#tag`.
    let task = client.ok(
        "add_task",
        serde_json::json!({"title": "@work pay the invoice"}),
    );
    assert_eq!(task["title"], "pay the invoice");
    assert_eq!(task["list"]["name"], "Work");

    // Typing a name that is no list never creates one.
    let stray = client.ok("add_task", serde_json::json!({"title": "@nope buy milk"}));
    assert_eq!(stray["title"], "buy milk");
    assert_eq!(stray["list"]["name"], "Tasks");
    assert_eq!(
        client
            .ok("list_lists", serde_json::json!({}))
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // What the window shows is what a client sees, unless it asks for more.
    client.ok(
        "update_settings",
        serde_json::json!({"current_list": "Work"}),
    );
    let showing = client.ok("list_tasks", serde_json::json!({}));
    assert_eq!(showing["tasks"].as_array().unwrap().len(), 1);
    let everything = client.ok("list_tasks", serde_json::json!({"list": "all"}));
    assert_eq!(everything["tasks"].as_array().unwrap().len(), 2);

    // Deleting a list keeps its tasks.
    let left = client.ok("delete_list", serde_json::json!({"id": work_id}));
    assert_eq!(left.as_array().unwrap().len(), 1);
    let after = client.ok("list_tasks", serde_json::json!({"list": "all"}));
    assert_eq!(after["tasks"].as_array().unwrap().len(), 2);
}

#[test]
fn lists_stay_out_of_the_way_until_asked_for() {
    let service = Service::start("no-lists");
    let mut client = service.connect();
    client.read(); // hello

    let task = client.ok("add_task", serde_json::json!({"title": "@work pay rent"}));
    assert_eq!(task["title"], "@work pay rent");
    assert!(
        task.get("list").is_none(),
        "no list key while the feature is off"
    );
    let listed = client.ok("list_tasks", serde_json::json!({}));
    assert_eq!(listed["settings"]["lists_enabled"], false);
}

#[test]
fn a_second_service_steps_aside() {
    let service = Service::start("rival");
    let status = service.start_rival();
    assert!(
        status.success(),
        "a duplicate start must be harmless, got {status}"
    );

    // The original is still the one answering.
    let mut client = service.connect();
    client.read();
    assert_eq!(client.ok("ping", serde_json::json!({}))["pong"], true);
}

#[test]
fn tells_subscribers_about_writes_made_straight_to_the_database() {
    // This is the case that matters: the window never goes through the socket,
    // so the service has to notice its commits on its own.
    let service = Service::start("watch");
    let mut client = service.connect();
    client.read(); // hello
    client.ok("subscribe", serde_json::json!({}));

    let db = Db::open_at(&service.db_path()).expect("open the database directly");
    let priority = db.load_priorities().unwrap()[0].id;
    db.insert("written by the window", priority).unwrap();

    let event = client.wait_for_event("changed");
    assert!(event["version"].is_i64());

    let listed = client.ok("list_tasks", serde_json::json!({}));
    assert_eq!(listed["tasks"][0]["title"], "written by the window");
}

#[test]
fn tells_subscribers_about_its_own_writes_too() {
    // `PRAGMA data_version` does not move for our own commits, so these have
    // to be announced by hand. Easy to break, hence the separate test.
    let service = Service::start("selfwrite");
    let mut client = service.connect();
    client.read(); // hello
    client.ok("subscribe", serde_json::json!({}));

    client.send(
        "add_task",
        serde_json::json!({"title": "added over the socket"}),
    );
    client.wait_for_event("changed");
}

#[test]
fn cleans_up_after_itself_on_sigterm() {
    let mut service = Service::start("shutdown");
    assert!(service.socket.exists());
    assert!(service.info_path().exists());

    service.stop();

    assert!(!service.socket.exists(), "socket left behind");
    assert!(!service.info_path().exists(), "status file left behind");
}

#[test]
fn a_status_file_left_by_a_hard_kill_does_not_look_like_a_running_service() {
    // SIGKILL skips the cleanup path, which is exactly what a Flatpak stop does
    // (`flatpak run --die-with-parent`). The socket and status file survive, so
    // liveness has to be decided by whether anything answers, not by their
    // presence or by the pid they name — a sandboxed pid means nothing here.
    let mut service = Service::start("stale");
    assert!(service.info_path().exists());

    service.kill_hard();

    assert!(service.info_path().exists(), "expected a stale status file");
    assert!(service.socket.exists(), "expected a stale socket");

    let status = service.run_status();
    assert!(
        !status.status.success(),
        "status should fail when nothing is listening, got:\n{}",
        status.stdout
    );
    assert!(
        status.stdout.contains("no dagr service is answering"),
        "unhelpful output:\n{}",
        status.stdout
    );
}
