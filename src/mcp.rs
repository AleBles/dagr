//! The built-in MCP server (`dagr --mcp`), speaking JSON-RPC over stdio.
//!
//! Every action a person can take in the window has a tool here, operating on
//! the same SQLite database. The running GUI notices changes within a second.

use std::sync::{Arc, Mutex, MutexGuard};

use rmcp::{
    handler::server::wrapper::Parameters, model::*, schemars, tool, tool_handler, tool_router,
    transport::stdio, ErrorData as McpError, ServerHandler, ServiceExt,
};
use serde::Serialize;

use crate::db::Db;
use crate::settings::{Settings, SortOrder};
use crate::task::{self, Priority, Task};

/// Runs the server until the client disconnects.
pub async fn serve_stdio(db: Db) -> anyhow::Result<()> {
    let running = TasksServer::new(db).serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}

#[derive(Clone)]
pub struct TasksServer {
    db: Arc<Mutex<Db>>,
}

// --- parameter types -------------------------------------------------------
// Doc comments become the field descriptions the AI sees.

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListTasksParams {
    /// Include completed tasks (default: true).
    pub include_done: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddTaskParams {
    /// The task text. Each leading `!` raises the priority one level above the default.
    pub title: String,
    /// Priority name (case-insensitive) or id. Default: the configured default priority
    /// (see get_settings), or the lowest one.
    pub priority: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateTaskParams {
    pub id: i64,
    /// New title.
    pub title: Option<String>,
    /// New priority, by name (case-insensitive) or id.
    pub priority: Option<String>,
    /// Mark as done (true) or not done (false).
    pub done: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct IdParams {
    pub id: i64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddPriorityParams {
    pub name: String,
    /// Hex color like `#e01b24`. Default: the next color from the palette.
    pub color: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdatePriorityParams {
    pub id: i64,
    pub name: Option<String>,
    /// Hex color like `#e01b24`.
    pub color: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateSettingsParams {
    /// Turn the priority feature on or off. Off: no dots or picker in the app,
    /// tasks sort by creation only, `!` prefixes are plain text.
    pub priorities_enabled: Option<bool>,
    /// Priority for new tasks: a priority name or id, or "lowest" for automatic.
    pub default_priority: Option<String>,
    /// Ordering of the task list. Completed tasks always go last.
    pub sort_order: Option<SortOrder>,
    /// Serve MCP over HTTP (http://127.0.0.1:<port>/mcp) while the app window is open.
    pub mcp_http_enabled: Option<bool>,
    /// Port for the HTTP MCP server (1024–65535).
    pub mcp_http_port: Option<u16>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReorderPrioritiesParams {
    /// Every priority id, highest priority first.
    pub ids: Vec<i64>,
}

// --- output types ------------------------------------------------------------

#[derive(Serialize)]
struct TaskView {
    id: i64,
    title: String,
    /// Omitted when the priority feature is turned off.
    #[serde(skip_serializing_if = "Option::is_none")]
    priority: Option<PriorityRef>,
    done: bool,
    created_at: i64,
    completed_at: Option<i64>,
}

#[derive(Serialize)]
struct PriorityRef {
    id: i64,
    name: String,
    color: String,
}

fn view(task: Task, priorities: &[Priority], settings: &Settings) -> TaskView {
    let p = priorities.iter().find(|p| p.id == task.priority_id);
    TaskView {
        id: task.id,
        title: task.title,
        priority: settings.priorities_enabled.then(|| PriorityRef {
            id: task.priority_id,
            name: p
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Unknown".into()),
            color: p.map(|p| p.color.clone()).unwrap_or_default(),
        }),
        done: task.done,
        created_at: task.created_at,
        completed_at: task.completed_at,
    }
}

// --- tools ---------------------------------------------------------------------

#[tool_router]
impl TasksServer {
    pub fn new(db: Db) -> Self {
        Self {
            db: Arc::new(Mutex::new(db)),
        }
    }

    #[tool(
        description = "List tasks in display order: open tasks first, then by priority (highest first), then oldest first."
    )]
    fn list_tasks(
        &self,
        Parameters(p): Parameters<ListTasksParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        let priorities = db.load_priorities().map_err(internal)?;
        let settings = db.settings().map_err(internal)?;
        let include_done = p.include_done.unwrap_or(true);
        let tasks: Vec<TaskView> = db
            .load_all()
            .map_err(internal)?
            .into_iter()
            .filter(|t| include_done || !t.done)
            .map(|t| view(t, &priorities, &settings))
            .collect();
        json(&tasks)
    }

    #[tool(description = "Add a task. Returns the created task.")]
    fn add_task(
        &self,
        Parameters(p): Parameters<AddTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        let priorities = db.load_priorities().map_err(internal)?;
        let settings = db.settings().map_err(internal)?;
        let (bangs, title) = if settings.priorities_enabled {
            task::split_priority_prefix(&p.title)
        } else {
            (0, p.title.trim().to_string())
        };
        if title.is_empty() {
            return Err(invalid("title must not be empty"));
        }
        let priority_id = match p.priority {
            Some(spec) => {
                require_priorities(&settings)?;
                resolve_priority(&priorities, &spec)?
            }
            None => task::pick_priority(&priorities, settings.default_priority_id, bangs)
                .ok_or_else(|| invalid("no priorities defined"))?,
        };
        let task = db.insert(&title, priority_id).map_err(internal)?;
        json(&view(task, &priorities, &settings))
    }

    #[tool(
        description = "Change a task's title, priority and/or done state. Only the given fields change."
    )]
    fn update_task(
        &self,
        Parameters(p): Parameters<UpdateTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        let priorities = db.load_priorities().map_err(internal)?;
        let settings = db.settings().map_err(internal)?;
        let tasks = db.load_all().map_err(internal)?;
        if !tasks.iter().any(|t| t.id == p.id) {
            return Err(invalid(format!("no task with id {}", p.id)));
        }
        if let Some(title) = p.title {
            let title = title.trim();
            if title.is_empty() {
                return Err(invalid("title must not be empty"));
            }
            db.set_title(p.id, title).map_err(internal)?;
        }
        if let Some(spec) = p.priority {
            require_priorities(&settings)?;
            let priority_id = resolve_priority(&priorities, &spec)?;
            db.set_priority(p.id, priority_id).map_err(internal)?;
        }
        if let Some(done) = p.done {
            db.set_done(p.id, done).map_err(internal)?;
        }
        let updated = db
            .load_all()
            .map_err(internal)?
            .into_iter()
            .find(|t| t.id == p.id)
            .ok_or_else(|| internal_msg("task vanished"))?;
        json(&view(updated, &priorities, &settings))
    }

    #[tool(description = "Delete a task permanently.")]
    fn delete_task(&self, Parameters(p): Parameters<IdParams>) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        if !db
            .load_all()
            .map_err(internal)?
            .iter()
            .any(|t| t.id == p.id)
        {
            return Err(invalid(format!("no task with id {}", p.id)));
        }
        db.delete(p.id).map_err(internal)?;
        text(format!("Deleted task {}", p.id))
    }

    #[tool(description = "Delete every completed task. Returns the removed tasks.")]
    fn clear_completed(&self) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        let priorities = db.load_priorities().map_err(internal)?;
        let settings = db.settings().map_err(internal)?;
        let removed: Vec<TaskView> = db
            .clear_completed()
            .map_err(internal)?
            .into_iter()
            .map(|t| view(t, &priorities, &settings))
            .collect();
        json(&removed)
    }

    #[tool(description = "Read the app settings.")]
    fn get_settings(&self) -> Result<CallToolResult, McpError> {
        json(&self.db()?.settings().map_err(internal)?)
    }

    #[tool(
        description = "Change app settings. Only the given fields change. Returns the new settings."
    )]
    fn update_settings(
        &self,
        Parameters(p): Parameters<UpdateSettingsParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        let mut settings = db.settings().map_err(internal)?;
        if let Some(enabled) = p.priorities_enabled {
            settings.priorities_enabled = enabled;
        }
        if let Some(spec) = p.default_priority {
            settings.default_priority_id = if spec.trim().eq_ignore_ascii_case("lowest") {
                None
            } else {
                let priorities = db.load_priorities().map_err(internal)?;
                Some(resolve_priority(&priorities, &spec)?)
            };
        }
        if let Some(order) = p.sort_order {
            settings.sort_order = order;
        }
        if let Some(enabled) = p.mcp_http_enabled {
            settings.mcp_http_enabled = enabled;
        }
        if let Some(port) = p.mcp_http_port {
            if port < Settings::MIN_PORT {
                return Err(invalid(format!(
                    "mcp_http_port must be at least {}",
                    Settings::MIN_PORT
                )));
            }
            settings.mcp_http_port = port;
        }
        db.save_settings(&settings).map_err(internal)?;
        json(&settings)
    }

    #[tool(
        description = "List the priorities, highest first. The order here is the order of the task list."
    )]
    fn list_priorities(&self) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&db.load_priorities().map_err(internal)?)
    }

    #[tool(
        description = "Add a priority at the bottom (lowest). Use reorder_priorities to move it."
    )]
    fn add_priority(
        &self,
        Parameters(p): Parameters<AddPriorityParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        let name = p.name.trim();
        if name.is_empty() {
            return Err(invalid("name must not be empty"));
        }
        let color = match p.color {
            Some(c) => validate_color(&c)?,
            None => {
                let count = db.load_priorities().map_err(internal)?.len();
                crate::ui::colors::PALETTE[count % crate::ui::colors::PALETTE.len()].to_string()
            }
        };
        json(&db.insert_priority(name, &color).map_err(internal)?)
    }

    #[tool(description = "Rename and/or recolor a priority.")]
    fn update_priority(
        &self,
        Parameters(p): Parameters<UpdatePriorityParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        let priorities = db.load_priorities().map_err(internal)?;
        if !priorities.iter().any(|x| x.id == p.id) {
            return Err(invalid(format!("no priority with id {}", p.id)));
        }
        if let Some(name) = p.name {
            let name = name.trim();
            if name.is_empty() {
                return Err(invalid("name must not be empty"));
            }
            db.rename_priority(p.id, name).map_err(internal)?;
        }
        if let Some(color) = p.color {
            db.set_priority_color(p.id, &validate_color(&color)?)
                .map_err(internal)?;
        }
        json(&db.load_priorities().map_err(internal)?)
    }

    #[tool(
        description = "Set the priority order. Pass every priority id exactly once, highest first."
    )]
    fn reorder_priorities(
        &self,
        Parameters(p): Parameters<ReorderPrioritiesParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        let mut existing: Vec<i64> = db
            .load_priorities()
            .map_err(internal)?
            .iter()
            .map(|x| x.id)
            .collect();
        let mut given = p.ids.clone();
        existing.sort_unstable();
        given.sort_unstable();
        if existing != given {
            return Err(invalid(format!(
                "ids must be a permutation of all priority ids {existing:?}"
            )));
        }
        db.set_priority_order(&p.ids).map_err(internal)?;
        json(&db.load_priorities().map_err(internal)?)
    }

    #[tool(
        description = "Delete a priority. Its tasks move to the next lower priority (or next higher if it was the lowest). The last priority cannot be deleted."
    )]
    fn delete_priority(
        &self,
        Parameters(p): Parameters<IdParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        db.delete_priority(p.id)
            .map_err(|e| invalid(e.to_string()))?;
        json(&db.load_priorities().map_err(internal)?)
    }

    fn db(&self) -> Result<MutexGuard<'_, Db>, McpError> {
        self.db
            .lock()
            .map_err(|_| internal_msg("database lock poisoned"))
    }
}

#[tool_handler]
impl ServerHandler for TasksServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("dagr", env!("CARGO_PKG_VERSION"))
                    .with_title("Dagr")
                    .with_description("Priority-ordered personal task list"),
            )
            .with_instructions(
                "Dagr: a personal to-do list. Tasks have a title, a done flag and one \
                 priority. Priorities are user-defined, ordered highest first, and that \
                 order sorts the list; the feature can be switched off in settings. Use \
                 list_tasks / list_priorities / get_settings first to see ids and state. \
                 Changes appear in the running app immediately."
                    .to_string(),
            )
    }
}

// --- helpers -------------------------------------------------------------------

/// Accepts a priority id or a case-insensitive name.
fn resolve_priority(priorities: &[Priority], spec: &str) -> Result<i64, McpError> {
    let spec = spec.trim();
    if let Ok(id) = spec.parse::<i64>() {
        if priorities.iter().any(|p| p.id == id) {
            return Ok(id);
        }
    }
    priorities
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(spec))
        .map(|p| p.id)
        .ok_or_else(|| {
            let names: Vec<&str> = priorities.iter().map(|p| p.name.as_str()).collect();
            invalid(format!("unknown priority {spec:?}; available: {names:?}"))
        })
}

fn require_priorities(settings: &Settings) -> Result<(), McpError> {
    if settings.priorities_enabled {
        Ok(())
    } else {
        Err(invalid(
            "the priority feature is turned off; enable it with update_settings {priorities_enabled: true}",
        ))
    }
}

fn validate_color(color: &str) -> Result<String, McpError> {
    let c = color.trim().to_ascii_lowercase();
    let ok = c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit());
    if ok {
        Ok(c)
    } else {
        Err(invalid(format!(
            "color must look like #rrggbb, got {color:?}"
        )))
    }
}

fn json<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let body = serde_json::to_string_pretty(value).map_err(|e| internal_msg(e.to_string()))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(body)]))
}

fn text(message: String) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::text(message)]))
}

fn invalid(message: impl Into<String>) -> McpError {
    McpError::invalid_params(message.into(), None)
}

fn internal(err: anyhow::Error) -> McpError {
    internal_msg(format!("{err:#}"))
}

fn internal_msg(message: impl Into<String>) -> McpError {
    McpError::internal_error(message.into(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> TasksServer {
        TasksServer::new(Db::open_in_memory().unwrap())
    }

    /// The text of the first content block, parsed as JSON.
    fn payload(result: CallToolResult) -> serde_json::Value {
        let v = serde_json::to_value(&result).unwrap();
        let text = v["content"][0]["text"].as_str().expect("text content");
        serde_json::from_str(text).unwrap_or(serde_json::Value::String(text.to_string()))
    }

    #[test]
    fn exposes_every_action_as_a_tool() {
        let names: Vec<String> = TasksServer::tool_router()
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        for expected in [
            "list_tasks",
            "add_task",
            "update_task",
            "delete_task",
            "clear_completed",
            "list_priorities",
            "add_priority",
            "update_priority",
            "reorder_priorities",
            "delete_priority",
        ] {
            assert!(
                names.contains(&expected.to_string()),
                "missing tool {expected}"
            );
        }
    }

    #[test]
    fn add_and_list_tasks() {
        let s = server();
        let added = payload(
            s.add_task(Parameters(AddTaskParams {
                title: "Ship it".into(),
                priority: Some("high".into()),
            }))
            .unwrap(),
        );
        assert_eq!(added["priority"]["name"], "High");
        let id = added["id"].as_i64().unwrap();

        payload(
            s.add_task(Parameters(AddTaskParams {
                title: "!! bangs".into(),
                priority: None,
            }))
            .unwrap(),
        );

        let updated = payload(
            s.update_task(Parameters(UpdateTaskParams {
                id,
                title: None,
                priority: None,
                done: Some(true),
            }))
            .unwrap(),
        );
        assert_eq!(updated["done"], true);

        let open = payload(
            s.list_tasks(Parameters(ListTasksParams {
                include_done: Some(false),
            }))
            .unwrap(),
        );
        assert_eq!(open.as_array().unwrap().len(), 1);
        assert_eq!(open[0]["title"], "bangs");
        assert_eq!(open[0]["priority"]["name"], "Medium");
    }

    #[test]
    fn rejects_bad_input() {
        let s = server();
        assert!(s
            .add_task(Parameters(AddTaskParams {
                title: "x".into(),
                priority: Some("urgent".into()),
            }))
            .is_err());
        assert!(s
            .reorder_priorities(Parameters(ReorderPrioritiesParams { ids: vec![1, 2] }))
            .is_err());
        assert!(s
            .add_priority(Parameters(AddPriorityParams {
                name: "Bad".into(),
                color: Some("red".into()),
            }))
            .is_err());
        assert!(s.delete_task(Parameters(IdParams { id: 999 })).is_err());
    }

    #[test]
    fn priorities_can_be_switched_off() {
        let s = server();
        let on = payload(s.get_settings().unwrap());
        assert_eq!(on["priorities_enabled"], true);

        let off = payload(
            s.update_settings(Parameters(UpdateSettingsParams {
                priorities_enabled: Some(false),
                default_priority: None,
                sort_order: None,
                mcp_http_enabled: None,
                mcp_http_port: None,
            }))
            .unwrap(),
        );
        assert_eq!(off["priorities_enabled"], false);
        assert!(s
            .update_settings(Parameters(UpdateSettingsParams {
                priorities_enabled: None,
                default_priority: None,
                sort_order: None,
                mcp_http_enabled: Some(true),
                mcp_http_port: Some(80),
            }))
            .is_err());

        // `!` is plain text now and priority params are refused.
        let added = payload(
            s.add_task(Parameters(AddTaskParams {
                title: "!! literal bangs".into(),
                priority: None,
            }))
            .unwrap(),
        );
        assert_eq!(added["title"], "!! literal bangs");
        assert!(added.get("priority").is_none());
        assert!(s
            .add_task(Parameters(AddTaskParams {
                title: "x".into(),
                priority: Some("high".into()),
            }))
            .is_err());
    }

    #[test]
    fn default_priority_and_sort_order_are_configurable() {
        let s = server();
        let changed = payload(
            s.update_settings(Parameters(UpdateSettingsParams {
                priorities_enabled: None,
                default_priority: Some("medium".into()),
                sort_order: Some(SortOrder::Newest),
                mcp_http_enabled: None,
                mcp_http_port: None,
            }))
            .unwrap(),
        );
        assert_eq!(changed["default_priority_id"], 2);
        assert_eq!(changed["sort_order"], "newest");

        let plain = payload(
            s.add_task(Parameters(AddTaskParams {
                title: "uses default".into(),
                priority: None,
            }))
            .unwrap(),
        );
        assert_eq!(plain["priority"]["name"], "Medium");
        let bumped = payload(
            s.add_task(Parameters(AddTaskParams {
                title: "! one above default".into(),
                priority: None,
            }))
            .unwrap(),
        );
        assert_eq!(bumped["priority"]["name"], "High");

        // Newest first: the bumped task was added last, so it leads.
        let listed = payload(
            s.list_tasks(Parameters(ListTasksParams { include_done: None }))
                .unwrap(),
        );
        assert_eq!(listed[0]["title"], "one above default");

        assert!(s
            .update_settings(Parameters(UpdateSettingsParams {
                priorities_enabled: None,
                default_priority: Some("nonexistent".into()),
                sort_order: None,
                mcp_http_enabled: None,
                mcp_http_port: None,
            }))
            .is_err());
        let back = payload(
            s.update_settings(Parameters(UpdateSettingsParams {
                priorities_enabled: None,
                default_priority: Some("lowest".into()),
                sort_order: None,
                mcp_http_enabled: None,
                mcp_http_port: None,
            }))
            .unwrap(),
        );
        assert!(back["default_priority_id"].is_null());
    }

    #[test]
    fn manages_priorities() {
        let s = server();
        let added = payload(
            s.add_priority(Parameters(AddPriorityParams {
                name: "Someday".into(),
                color: None,
            }))
            .unwrap(),
        );
        let new_id = added["id"].as_i64().unwrap();
        let all = payload(s.list_priorities().unwrap());
        let mut ids: Vec<i64> = all
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_i64().unwrap())
            .collect();
        assert_eq!(*ids.last().unwrap(), new_id);

        ids.rotate_right(1); // new one becomes the highest
        let reordered = payload(
            s.reorder_priorities(Parameters(ReorderPrioritiesParams { ids: ids.clone() }))
                .unwrap(),
        );
        assert_eq!(reordered[0]["name"], "Someday");

        let renamed = payload(
            s.update_priority(Parameters(UpdatePriorityParams {
                id: new_id,
                name: Some("Urgent".into()),
                color: Some("#ABCDEF".into()),
            }))
            .unwrap(),
        );
        assert_eq!(renamed[0]["name"], "Urgent");
        assert_eq!(renamed[0]["color"], "#abcdef");

        let after = payload(
            s.delete_priority(Parameters(IdParams { id: new_id }))
                .unwrap(),
        );
        assert_eq!(after.as_array().unwrap().len(), 4);
    }
}
