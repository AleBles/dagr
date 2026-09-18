//! The task logic, independent of how it is reached.
//!
//! Every rule about tasks, priorities and settings lives here: resolving a
//! priority by name or id, the `!` prefix, colour validation, and the shape
//! results are reported in. The MCP tools in `mcp.rs` and the socket handlers
//! in `serve.rs` are both thin wrappers over this, so the two front doors can
//! never drift apart.

use serde::Serialize;

use crate::db::Db;
use crate::settings::{Settings, SortOrder};
use crate::task::{self, Priority, Task, PALETTE};

/// Why a call failed. `Invalid` is the caller's fault, `Internal` is ours.
#[derive(Debug)]
pub enum ApiError {
    Invalid(String),
    Internal(String),
}

impl ApiError {
    /// The short tag reported on the socket (`error.kind`).
    pub fn kind(&self) -> &'static str {
        match self {
            ApiError::Invalid(_) => "invalid_params",
            ApiError::Internal(_) => "internal",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            ApiError::Invalid(m) | ApiError::Internal(m) => m,
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ApiError {}

pub type Result<T> = std::result::Result<T, ApiError>;

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Invalid(message.into())
}

/// Anything coming back from the database is our problem, not the caller's.
fn internal(err: anyhow::Error) -> ApiError {
    ApiError::Internal(format!("{err:#}"))
}

// --- parameters ------------------------------------------------------------
// Doc comments become the field descriptions the AI sees, so these types are
// shared with the MCP tools rather than duplicated there.

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
    /// Serve MCP over HTTP (http://127.0.0.1:<port>/mcp).
    pub mcp_http_enabled: Option<bool>,
    /// Port for the HTTP MCP server (1024–65535).
    pub mcp_http_port: Option<u16>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReorderPrioritiesParams {
    /// Every priority id, highest priority first.
    pub ids: Vec<i64>,
}

// --- results ---------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct TaskView {
    pub id: i64,
    pub title: String,
    /// Omitted when the priority feature is turned off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<PriorityRef>,
    pub done: bool,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct PriorityRef {
    pub id: i64,
    pub name: String,
    pub color: String,
}

/// Everything a client needs to draw the list, in one reply.
#[derive(Debug, Serialize)]
pub struct Snapshot {
    /// SQLite's `data_version`, so a client can tell replies apart from events.
    pub version: i64,
    pub tasks: Vec<TaskView>,
    pub priorities: Vec<Priority>,
    pub settings: Settings,
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

// --- the api ---------------------------------------------------------------

pub struct Api<'a> {
    db: &'a Db,
}

impl<'a> Api<'a> {
    pub fn new(db: &'a Db) -> Self {
        Self { db }
    }

    pub fn list_tasks(&self, p: ListTasksParams) -> Result<Vec<TaskView>> {
        let priorities = self.db.load_priorities().map_err(internal)?;
        let settings = self.db.settings().map_err(internal)?;
        let include_done = p.include_done.unwrap_or(true);
        Ok(self
            .db
            .load_all()
            .map_err(internal)?
            .into_iter()
            .filter(|t| include_done || !t.done)
            .map(|t| view(t, &priorities, &settings))
            .collect())
    }

    /// Tasks, priorities and settings together — what the launcher plugin and
    /// the window both want, in one round trip.
    pub fn snapshot(&self, include_done: bool) -> Result<Snapshot> {
        let priorities = self.db.load_priorities().map_err(internal)?;
        let settings = self.db.settings().map_err(internal)?;
        let tasks = self
            .db
            .load_all()
            .map_err(internal)?
            .into_iter()
            .filter(|t| include_done || !t.done)
            .map(|t| view(t, &priorities, &settings))
            .collect();
        Ok(Snapshot {
            version: self.db.data_version().map_err(internal)?,
            tasks,
            priorities,
            settings,
        })
    }

    pub fn add_task(&self, p: AddTaskParams) -> Result<TaskView> {
        let priorities = self.db.load_priorities().map_err(internal)?;
        let settings = self.db.settings().map_err(internal)?;
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
        let task = self.db.insert(&title, priority_id).map_err(internal)?;
        Ok(view(task, &priorities, &settings))
    }

    pub fn update_task(&self, p: UpdateTaskParams) -> Result<TaskView> {
        let priorities = self.db.load_priorities().map_err(internal)?;
        let settings = self.db.settings().map_err(internal)?;
        let tasks = self.db.load_all().map_err(internal)?;
        if !tasks.iter().any(|t| t.id == p.id) {
            return Err(invalid(format!("no task with id {}", p.id)));
        }
        if let Some(title) = p.title {
            let title = title.trim();
            if title.is_empty() {
                return Err(invalid("title must not be empty"));
            }
            self.db.set_title(p.id, title).map_err(internal)?;
        }
        if let Some(spec) = p.priority {
            require_priorities(&settings)?;
            let priority_id = resolve_priority(&priorities, &spec)?;
            self.db.set_priority(p.id, priority_id).map_err(internal)?;
        }
        if let Some(done) = p.done {
            self.db.set_done(p.id, done).map_err(internal)?;
        }
        let updated = self
            .db
            .load_all()
            .map_err(internal)?
            .into_iter()
            .find(|t| t.id == p.id)
            .ok_or_else(|| ApiError::Internal("task vanished".into()))?;
        Ok(view(updated, &priorities, &settings))
    }

    pub fn delete_task(&self, p: IdParams) -> Result<()> {
        if !self
            .db
            .load_all()
            .map_err(internal)?
            .iter()
            .any(|t| t.id == p.id)
        {
            return Err(invalid(format!("no task with id {}", p.id)));
        }
        self.db.delete(p.id).map_err(internal)
    }

    pub fn clear_completed(&self) -> Result<Vec<TaskView>> {
        let priorities = self.db.load_priorities().map_err(internal)?;
        let settings = self.db.settings().map_err(internal)?;
        Ok(self
            .db
            .clear_completed()
            .map_err(internal)?
            .into_iter()
            .map(|t| view(t, &priorities, &settings))
            .collect())
    }

    pub fn get_settings(&self) -> Result<Settings> {
        self.db.settings().map_err(internal)
    }

    pub fn update_settings(&self, p: UpdateSettingsParams) -> Result<Settings> {
        let mut settings = self.db.settings().map_err(internal)?;
        if let Some(enabled) = p.priorities_enabled {
            settings.priorities_enabled = enabled;
        }
        if let Some(spec) = p.default_priority {
            settings.default_priority_id = if spec.trim().eq_ignore_ascii_case("lowest") {
                None
            } else {
                let priorities = self.db.load_priorities().map_err(internal)?;
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
        self.db.save_settings(&settings).map_err(internal)?;
        Ok(settings)
    }

    pub fn list_priorities(&self) -> Result<Vec<Priority>> {
        self.db.load_priorities().map_err(internal)
    }

    pub fn add_priority(&self, p: AddPriorityParams) -> Result<Priority> {
        let name = p.name.trim();
        if name.is_empty() {
            return Err(invalid("name must not be empty"));
        }
        let color = match p.color {
            Some(c) => validate_color(&c)?,
            None => {
                let count = self.db.load_priorities().map_err(internal)?.len();
                PALETTE[count % PALETTE.len()].to_string()
            }
        };
        self.db.insert_priority(name, &color).map_err(internal)
    }

    pub fn update_priority(&self, p: UpdatePriorityParams) -> Result<Vec<Priority>> {
        let priorities = self.db.load_priorities().map_err(internal)?;
        if !priorities.iter().any(|x| x.id == p.id) {
            return Err(invalid(format!("no priority with id {}", p.id)));
        }
        if let Some(name) = p.name {
            let name = name.trim();
            if name.is_empty() {
                return Err(invalid("name must not be empty"));
            }
            self.db.rename_priority(p.id, name).map_err(internal)?;
        }
        if let Some(color) = p.color {
            self.db
                .set_priority_color(p.id, &validate_color(&color)?)
                .map_err(internal)?;
        }
        self.db.load_priorities().map_err(internal)
    }

    pub fn reorder_priorities(&self, p: ReorderPrioritiesParams) -> Result<Vec<Priority>> {
        let mut existing: Vec<i64> = self
            .db
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
        self.db.set_priority_order(&p.ids).map_err(internal)?;
        self.db.load_priorities().map_err(internal)
    }

    pub fn delete_priority(&self, p: IdParams) -> Result<Vec<Priority>> {
        // The "last priority" rule lives in the database layer; report it as
        // the caller's mistake rather than an internal fault.
        self.db
            .delete_priority(p.id)
            .map_err(|e| invalid(e.to_string()))?;
        self.db.load_priorities().map_err(internal)
    }
}

// --- helpers ---------------------------------------------------------------

/// Accepts a priority id or a case-insensitive name.
fn resolve_priority(priorities: &[Priority], spec: &str) -> Result<i64> {
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

fn require_priorities(settings: &Settings) -> Result<()> {
    if settings.priorities_enabled {
        Ok(())
    } else {
        Err(invalid(
            "the priority feature is turned off; enable it with update_settings {priorities_enabled: true}",
        ))
    }
}

fn validate_color(color: &str) -> Result<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    /// All-`None` settings patch, so each test only names what it changes.
    fn patch() -> UpdateSettingsParams {
        UpdateSettingsParams {
            priorities_enabled: None,
            default_priority: None,
            sort_order: None,
            mcp_http_enabled: None,
            mcp_http_port: None,
        }
    }

    fn add(api: &Api, title: &str, priority: Option<&str>) -> Result<TaskView> {
        api.add_task(AddTaskParams {
            title: title.into(),
            priority: priority.map(Into::into),
        })
    }

    #[test]
    fn add_and_list_tasks() {
        let db = db();
        let api = Api::new(&db);

        let added = add(&api, "Ship it", Some("high")).unwrap();
        assert_eq!(added.priority.as_ref().unwrap().name, "High");

        add(&api, "!! bangs", None).unwrap();

        let updated = api
            .update_task(UpdateTaskParams {
                id: added.id,
                title: None,
                priority: None,
                done: Some(true),
            })
            .unwrap();
        assert!(updated.done);

        let open = api
            .list_tasks(ListTasksParams {
                include_done: Some(false),
            })
            .unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].title, "bangs");
        assert_eq!(open[0].priority.as_ref().unwrap().name, "Medium");
    }

    #[test]
    fn snapshot_carries_everything_needed_to_draw_the_list() {
        let db = db();
        let api = Api::new(&db);
        add(&api, "open", None).unwrap();
        let done = add(&api, "closed", None).unwrap();
        api.update_task(UpdateTaskParams {
            id: done.id,
            title: None,
            priority: None,
            done: Some(true),
        })
        .unwrap();

        let all = api.snapshot(true).unwrap();
        assert_eq!(all.tasks.len(), 2);
        assert_eq!(all.priorities.len(), 4);
        assert!(all.settings.priorities_enabled);

        assert_eq!(api.snapshot(false).unwrap().tasks.len(), 1);
    }

    #[test]
    fn rejects_bad_input() {
        let db = db();
        let api = Api::new(&db);
        assert!(add(&api, "x", Some("urgent")).is_err());
        assert!(api
            .reorder_priorities(ReorderPrioritiesParams { ids: vec![1, 2] })
            .is_err());
        assert!(api
            .add_priority(AddPriorityParams {
                name: "Bad".into(),
                color: Some("red".into()),
            })
            .is_err());
        assert!(api.delete_task(IdParams { id: 999 }).is_err());
        assert!(add(&api, "   ", None).is_err());
    }

    #[test]
    fn errors_are_tagged_for_the_caller() {
        let db = db();
        let api = Api::new(&db);
        let err = add(&api, "x", Some("urgent")).unwrap_err();
        assert_eq!(err.kind(), "invalid_params");
        assert!(err.message().contains("unknown priority"));
    }

    #[test]
    fn priorities_can_be_switched_off() {
        let db = db();
        let api = Api::new(&db);
        assert!(api.get_settings().unwrap().priorities_enabled);

        let off = api
            .update_settings(UpdateSettingsParams {
                priorities_enabled: Some(false),
                ..patch()
            })
            .unwrap();
        assert!(!off.priorities_enabled);

        assert!(api
            .update_settings(UpdateSettingsParams {
                mcp_http_enabled: Some(true),
                mcp_http_port: Some(80),
                ..patch()
            })
            .is_err());

        // `!` is plain text now and priority arguments are refused.
        let added = add(&api, "!! literal bangs", None).unwrap();
        assert_eq!(added.title, "!! literal bangs");
        assert!(added.priority.is_none());
        assert!(add(&api, "x", Some("high")).is_err());
    }

    #[test]
    fn default_priority_and_sort_order_are_configurable() {
        let db = db();
        let api = Api::new(&db);
        let changed = api
            .update_settings(UpdateSettingsParams {
                default_priority: Some("medium".into()),
                sort_order: Some(SortOrder::Newest),
                ..patch()
            })
            .unwrap();
        assert_eq!(changed.default_priority_id, Some(2));
        assert_eq!(changed.sort_order, SortOrder::Newest);

        let plain = add(&api, "uses default", None).unwrap();
        assert_eq!(plain.priority.as_ref().unwrap().name, "Medium");
        let bumped = add(&api, "! one above default", None).unwrap();
        assert_eq!(bumped.priority.as_ref().unwrap().name, "High");

        // Newest first: the bumped task was added last, so it leads.
        let listed = api
            .list_tasks(ListTasksParams { include_done: None })
            .unwrap();
        assert_eq!(listed[0].title, "one above default");

        assert!(api
            .update_settings(UpdateSettingsParams {
                default_priority: Some("nonexistent".into()),
                ..patch()
            })
            .is_err());
        let back = api
            .update_settings(UpdateSettingsParams {
                default_priority: Some("lowest".into()),
                ..patch()
            })
            .unwrap();
        assert!(back.default_priority_id.is_none());
    }

    #[test]
    fn manages_priorities() {
        let db = db();
        let api = Api::new(&db);
        let added = api
            .add_priority(AddPriorityParams {
                name: "Someday".into(),
                color: None,
            })
            .unwrap();
        let all = api.list_priorities().unwrap();
        assert_eq!(all.last().unwrap().id, added.id);

        let mut ids: Vec<i64> = all.iter().map(|p| p.id).collect();
        ids.rotate_right(1); // the new one becomes the highest
        let reordered = api
            .reorder_priorities(ReorderPrioritiesParams { ids })
            .unwrap();
        assert_eq!(reordered[0].name, "Someday");

        let renamed = api
            .update_priority(UpdatePriorityParams {
                id: added.id,
                name: Some("Urgent".into()),
                color: Some("#ABCDEF".into()),
            })
            .unwrap();
        assert_eq!(renamed[0].name, "Urgent");
        assert_eq!(renamed[0].color, "#abcdef");

        let after = api.delete_priority(IdParams { id: added.id }).unwrap();
        assert_eq!(after.len(), 4);
    }
}
