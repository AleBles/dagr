//! The task logic, independent of how it is reached.
//!
//! Every rule about tasks, priorities and settings lives here: resolving a
//! priority by name or id, the `!` prefix, colour validation, and the shape
//! results are reported in. The MCP tools in `mcp.rs` and the socket handlers
//! in `serve.rs` are both thin wrappers over this, so the two front doors can
//! never drift apart.

use serde::Serialize;

use crate::db::Db;
use crate::settings::{OpenOn, Settings, SortOrder};
use crate::task::{self, Label, List, Priority, Task};

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
pub struct AddListParams {
    pub name: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateListParams {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReorderListsParams {
    /// Every list id exactly once, in sidebar order.
    pub ids: Vec<i64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DeleteListParams {
    pub id: i64,
    /// Where its tasks go, by name or id. Default: the default list, or the
    /// first other list when that is the one being deleted.
    pub move_to: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListTasksParams {
    /// Include completed tasks (default: true).
    pub include_done: Option<bool>,
    /// Which list, by name (case-insensitive) or id; "all" for every list.
    /// Default: the list the window is showing.
    pub list: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AddTaskParams {
    /// The task text. Each leading `!` raises the priority one level above the
    /// default, and each leading `#tag` attaches that label, creating it if it
    /// is new. A `#` later in the text is ordinary text.
    pub title: String,
    /// Priority name (case-insensitive) or id. Default: the configured default priority
    /// (see get_settings), or the lowest one.
    pub priority: Option<String>,
    /// Labels by name (case-insensitive) or id, on top of any `#tags` in the
    /// title. These must already exist.
    pub labels: Option<Vec<String>>,
    /// List to file it under, by name (case-insensitive) or id. Overrides an
    /// `@list` in the title. Default: the list currently showing, or the
    /// default list.
    pub list: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateTaskParams {
    pub id: i64,
    /// New title. `#tags` are not read here: renaming a task never changes its
    /// labels.
    pub title: Option<String>,
    /// New priority, by name (case-insensitive) or id.
    pub priority: Option<String>,
    /// Replaces every label on the task, by name (case-insensitive) or id.
    /// `[]` removes them all; leaving the field out keeps them as they are.
    pub labels: Option<Vec<String>>,
    /// Move it to another list, by name (case-insensitive) or id.
    pub list: Option<String>,
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
pub struct AddLabelParams {
    pub name: String,
    /// Hex color like `#3584e4`. Default: the next color from the palette.
    pub color: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateLabelParams {
    pub id: i64,
    pub name: Option<String>,
    /// Hex color like `#3584e4`.
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
    /// Put the day a task was added ahead of priority: whole calendar days move
    /// as a block, oldest day first (newest first for the "newest" orders), and
    /// the sort order decides within a day. Off by default.
    pub date_first: Option<bool>,
    /// Turn the list feature on or off. Off: no sidebar, `@list` prefixes are
    /// plain text, and tasks are reported without their list. Off by default.
    pub lists_enabled: Option<bool>,
    /// List new tasks go to: a list name or id, or "first" for automatic.
    pub default_list: Option<String>,
    /// List to show, by name or id, or "all" for every list at once.
    pub current_list: Option<String>,
    /// Which list the window opens on.
    pub open_on: Option<OpenOn>,
    /// Turn the label feature on or off. Off: no label dots or picker in the
    /// app, `#tag` prefixes are plain text, and tasks report no labels.
    /// Assignments are kept, so turning it back on restores them. Off by
    /// default.
    pub labels_enabled: Option<bool>,
    /// Width the window opens at, in logical pixels (360-8192).
    pub window_width: Option<i32>,
    /// Height the window opens at, in logical pixels (300-8192).
    pub window_height: Option<i32>,
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
    /// Alphabetical. Omitted when empty, which includes every task while the
    /// label feature is turned off.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<Label>,
    /// Omitted while the list feature is turned off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<ListRef>,
    pub done: bool,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ListRef {
    pub id: i64,
    pub name: String,
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
    /// The whole catalogue, so a client can draw a picker without asking again.
    pub labels: Vec<Label>,
    /// Every list, in sidebar order.
    pub lists: Vec<List>,
    pub settings: Settings,
}

fn view(
    task: Task,
    priorities: &[Priority],
    labels: &[Label],
    lists: &[List],
    settings: &Settings,
) -> TaskView {
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
        labels: if settings.labels_enabled {
            // Walk the catalogue, not the ids, so the dots stay alphabetical
            // and an id whose label just went away drops out quietly.
            labels
                .iter()
                .filter(|l| task.label_ids.contains(&l.id))
                .cloned()
                .collect()
        } else {
            Vec::new()
        },
        list: settings.lists_enabled.then(|| ListRef {
            id: task.list_id,
            name: lists
                .iter()
                .find(|l| l.id == task.list_id)
                .map(|l| l.name.clone())
                .unwrap_or_else(|| "Unknown".into()),
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
        let labels = self.db.load_labels().map_err(internal)?;
        let lists = self.db.load_lists().map_err(internal)?;
        let settings = self.db.settings().map_err(internal)?;
        let include_done = p.include_done.unwrap_or(true);
        let tasks = match p.list {
            Some(spec) => {
                require_lists(&settings)?;
                self.db.load_in(resolve_list_filter(&lists, &spec)?)
            }
            None => self.db.load_all(),
        };
        Ok(tasks
            .map_err(internal)?
            .into_iter()
            .filter(|t| include_done || !t.done)
            .map(|t| view(t, &priorities, &labels, &lists, &settings))
            .collect())
    }

    /// Tasks, priorities, labels, lists and settings together — what the
    /// launcher plugin and the window both want, in one round trip. Takes the
    /// same arguments as `list_tasks`, so the socket answers like the tool.
    pub fn snapshot(&self, p: ListTasksParams) -> Result<Snapshot> {
        let priorities = self.db.load_priorities().map_err(internal)?;
        let labels = self.db.load_labels().map_err(internal)?;
        let lists = self.db.load_lists().map_err(internal)?;
        let settings = self.db.settings().map_err(internal)?;
        let include_done = p.include_done.unwrap_or(true);
        let tasks = match p.list {
            Some(spec) => {
                require_lists(&settings)?;
                self.db.load_in(resolve_list_filter(&lists, &spec)?)
            }
            None => self.db.load_all(),
        };
        let tasks = tasks
            .map_err(internal)?
            .into_iter()
            .filter(|t| include_done || !t.done)
            .map(|t| view(t, &priorities, &labels, &lists, &settings))
            .collect();
        Ok(Snapshot {
            version: self.db.data_version().map_err(internal)?,
            tasks,
            priorities,
            labels,
            lists,
            settings,
        })
    }

    pub fn add_task(&self, p: AddTaskParams) -> Result<TaskView> {
        let priorities = self.db.load_priorities().map_err(internal)?;
        let labels = self.db.load_labels().map_err(internal)?;
        let lists = self.db.load_lists().map_err(internal)?;
        let settings = self.db.settings().map_err(internal)?;
        let typed = task::split_prefixes(
            &p.title,
            settings.priorities_enabled,
            settings.labels_enabled,
            settings.lists_enabled,
        );
        if typed.title.is_empty() {
            return Err(invalid("title must not be empty"));
        }
        let priority_id = match p.priority {
            Some(spec) => {
                require_priorities(&settings)?;
                resolve_priority(&priorities, &spec)?
            }
            None => task::pick_priority(&priorities, settings.default_priority_id, typed.bangs)
                .ok_or_else(|| invalid("no priorities defined"))?,
        };
        // Labels named outright are resolved against what exists; `#tags` may
        // create new ones. Both are refused while the feature is off.
        let named = match p.labels {
            Some(specs) => {
                require_labels(&settings)?;
                specs
                    .iter()
                    .map(|spec| resolve_label(&labels, spec))
                    .collect::<Result<Vec<i64>>>()?
            }
            None => Vec::new(),
        };
        // An explicit list wins over a typed `@list`; a typed name that is no
        // list stays where it is, because typing one never creates it.
        let list_id = match (&p.list, &typed.list) {
            (Some(spec), _) => {
                require_lists(&settings)?;
                resolve_list(&lists, spec)?
            }
            (None, Some(name)) => match self.db.list_by_name(name).map_err(internal)? {
                Some(list) => list.id,
                None => self.showing_list(&settings).map_err(internal)?,
            },
            (None, None) => self.showing_list(&settings).map_err(internal)?,
        };
        let task = self
            .db
            .insert_with_labels(&typed.title, priority_id, list_id, &typed.tags)
            .map_err(internal)?;
        if named.is_empty() {
            // `insert_with_labels` may have created a label, so re-read.
            let labels = self.db.load_labels().map_err(internal)?;
            return Ok(view(task, &priorities, &labels, &lists, &settings));
        }
        let mut ids = task.label_ids.clone();
        for id in named {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        self.db.set_task_labels(task.id, &ids).map_err(internal)?;
        let labels = self.db.load_labels().map_err(internal)?;
        let task = self.reload(task.id)?;
        Ok(view(task, &priorities, &labels, &lists, &settings))
    }

    pub fn update_task(&self, p: UpdateTaskParams) -> Result<TaskView> {
        let priorities = self.db.load_priorities().map_err(internal)?;
        let labels = self.db.load_labels().map_err(internal)?;
        let lists = self.db.load_lists().map_err(internal)?;
        let settings = self.db.settings().map_err(internal)?;
        let tasks = self.db.load_in(None).map_err(internal)?;
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
        // Replaces the whole set. A title typed here is never scanned for
        // `#tags`: renaming a task cannot change its labels.
        if let Some(specs) = p.labels {
            require_labels(&settings)?;
            let ids = specs
                .iter()
                .map(|spec| resolve_label(&labels, spec))
                .collect::<Result<Vec<i64>>>()?;
            self.db.set_task_labels(p.id, &ids).map_err(internal)?;
        }
        if let Some(spec) = p.list {
            require_lists(&settings)?;
            let list_id = resolve_list(&lists, &spec)?;
            self.db.set_list(p.id, list_id).map_err(internal)?;
        }
        if let Some(done) = p.done {
            self.db.set_done(p.id, done).map_err(internal)?;
        }
        let updated = self.reload(p.id)?;
        Ok(view(updated, &priorities, &labels, &lists, &settings))
    }

    /// The list a new task goes to: the one showing, or the default when that
    /// is "all lists" or the feature is off.
    fn showing_list(&self, settings: &Settings) -> anyhow::Result<i64> {
        match settings
            .lists_enabled
            .then_some(settings.current_list_id)
            .flatten()
        {
            Some(id) => Ok(id),
            None => self.db.default_list(),
        }
    }

    /// One task, read back after a write, so a reply shows what was stored.
    /// Looks across every list: a task can be moved out of the one showing.
    fn reload(&self, id: i64) -> Result<Task> {
        self.db
            .load_in(None)
            .map_err(internal)?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| ApiError::Internal("task vanished".into()))
    }

    pub fn delete_task(&self, p: IdParams) -> Result<()> {
        if !self
            .db
            .load_in(None)
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
        let labels = self.db.load_labels().map_err(internal)?;
        let lists = self.db.load_lists().map_err(internal)?;
        let settings = self.db.settings().map_err(internal)?;
        Ok(self
            .db
            .clear_completed()
            .map_err(internal)?
            .into_iter()
            .map(|t| view(t, &priorities, &labels, &lists, &settings))
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
        if let Some(date_first) = p.date_first {
            settings.date_first = date_first;
        }
        if let Some(enabled) = p.labels_enabled {
            settings.labels_enabled = enabled;
        }
        if let Some(enabled) = p.lists_enabled {
            settings.lists_enabled = enabled;
        }
        if let Some(spec) = p.default_list {
            let lists = self.db.load_lists().map_err(internal)?;
            settings.default_list_id = if spec.trim().eq_ignore_ascii_case("first") {
                None
            } else {
                Some(resolve_list(&lists, &spec)?)
            };
        }
        if let Some(spec) = p.current_list {
            let lists = self.db.load_lists().map_err(internal)?;
            settings.current_list_id = resolve_list_filter(&lists, &spec)?;
        }
        if let Some(open_on) = p.open_on {
            settings.open_on = open_on;
        }
        if let Some(width) = p.window_width {
            settings.window_width = check_size(width, Settings::MIN_WINDOW_WIDTH, "window_width")?;
        }
        if let Some(height) = p.window_height {
            settings.window_height =
                check_size(height, Settings::MIN_WINDOW_HEIGHT, "window_height")?;
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
                task::next_color(count).to_string()
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

    // Managing labels works whether or not the feature is switched on, exactly
    // as managing priorities does. Only attaching one to a task is gated.

    pub fn list_labels(&self) -> Result<Vec<Label>> {
        self.db.load_labels().map_err(internal)
    }

    pub fn add_label(&self, p: AddLabelParams) -> Result<Label> {
        let name = p.name.trim();
        if name.is_empty() {
            return Err(invalid("name must not be empty"));
        }
        let labels = self.db.load_labels().map_err(internal)?;
        if labels.iter().any(|l| l.name.eq_ignore_ascii_case(name)) {
            return Err(invalid(format!("a label named {name:?} already exists")));
        }
        let color = match p.color {
            Some(c) => validate_color(&c)?,
            None => task::next_color(labels.len()).to_string(),
        };
        self.db.insert_label(name, &color).map_err(internal)
    }

    pub fn update_label(&self, p: UpdateLabelParams) -> Result<Vec<Label>> {
        let labels = self.db.load_labels().map_err(internal)?;
        if !labels.iter().any(|l| l.id == p.id) {
            return Err(invalid(format!("no label with id {}", p.id)));
        }
        if let Some(name) = p.name {
            let name = name.trim();
            if name.is_empty() {
                return Err(invalid("name must not be empty"));
            }
            if labels
                .iter()
                .any(|l| l.id != p.id && l.name.eq_ignore_ascii_case(name))
            {
                return Err(invalid(format!("a label named {name:?} already exists")));
            }
            self.db.rename_label(p.id, name).map_err(internal)?;
        }
        if let Some(color) = p.color {
            self.db
                .set_label_color(p.id, &validate_color(&color)?)
                .map_err(internal)?;
        }
        self.db.load_labels().map_err(internal)
    }

    // Managing lists, like priorities and labels, works whether or not the
    // feature is switched on. Only filing a task into one is gated.

    pub fn list_lists(&self) -> Result<Vec<List>> {
        self.db.load_lists().map_err(internal)
    }

    pub fn add_list(&self, p: AddListParams) -> Result<List> {
        let name = p.name.trim();
        if name.is_empty() {
            return Err(invalid("name must not be empty"));
        }
        if self.db.list_by_name(name).map_err(internal)?.is_some() {
            return Err(invalid(format!("a list named {name:?} already exists")));
        }
        self.db.insert_list(name).map_err(internal)
    }

    pub fn update_list(&self, p: UpdateListParams) -> Result<Vec<List>> {
        let lists = self.db.load_lists().map_err(internal)?;
        if !lists.iter().any(|l| l.id == p.id) {
            return Err(invalid(format!("no list with id {}", p.id)));
        }
        let name = p.name.trim();
        if name.is_empty() {
            return Err(invalid("name must not be empty"));
        }
        if lists
            .iter()
            .any(|l| l.id != p.id && l.name.eq_ignore_ascii_case(name))
        {
            return Err(invalid(format!("a list named {name:?} already exists")));
        }
        self.db.rename_list(p.id, name).map_err(internal)?;
        self.db.load_lists().map_err(internal)
    }

    pub fn reorder_lists(&self, p: ReorderListsParams) -> Result<Vec<List>> {
        let mut existing: Vec<i64> = self
            .db
            .load_lists()
            .map_err(internal)?
            .iter()
            .map(|l| l.id)
            .collect();
        let mut given = p.ids.clone();
        existing.sort_unstable();
        given.sort_unstable();
        if existing != given {
            return Err(invalid(format!(
                "ids must be a permutation of all list ids {existing:?}"
            )));
        }
        self.db.set_list_order(&p.ids).map_err(internal)?;
        self.db.load_lists().map_err(internal)
    }

    /// Deletes a list, moving its tasks to another one. Nothing is thrown
    /// away: a task always belongs somewhere.
    pub fn delete_list(&self, p: DeleteListParams) -> Result<Vec<List>> {
        let lists = self.db.load_lists().map_err(internal)?;
        if !lists.iter().any(|l| l.id == p.id) {
            return Err(invalid(format!("no list with id {}", p.id)));
        }
        let fallback = match p.move_to {
            Some(spec) => resolve_list(&lists, &spec)?,
            None => self.fallback_list(&lists, p.id)?,
        };
        if fallback == p.id {
            return Err(invalid("tasks must move to a different list"));
        }
        // The "last list" rule lives in the database layer; report it as the
        // caller's mistake rather than an internal fault.
        self.db
            .delete_list(p.id, fallback)
            .map_err(|e| invalid(e.to_string()))?;
        // A list that was the default, or the one showing, is gone now.
        let mut settings = self.db.settings().map_err(internal)?;
        let mut touched = false;
        if settings.default_list_id == Some(p.id) {
            settings.default_list_id = None;
            touched = true;
        }
        if settings.current_list_id == Some(p.id) {
            settings.current_list_id = Some(fallback);
            touched = true;
        }
        if touched {
            self.db.save_settings(&settings).map_err(internal)?;
        }
        self.db.load_lists().map_err(internal)
    }

    /// Where a deleted list's tasks go when the caller does not say: the
    /// default list, or the first other one when that is the one going.
    fn fallback_list(&self, lists: &[List], deleting: i64) -> Result<i64> {
        let default = self.db.default_list().map_err(internal)?;
        if default != deleting {
            return Ok(default);
        }
        lists
            .iter()
            .find(|l| l.id != deleting)
            .map(|l| l.id)
            .ok_or_else(|| invalid("at least one list is required"))
    }

    pub fn delete_label(&self, p: IdParams) -> Result<Vec<Label>> {
        let labels = self.db.load_labels().map_err(internal)?;
        if !labels.iter().any(|l| l.id == p.id) {
            return Err(invalid(format!("no label with id {}", p.id)));
        }
        self.db.delete_label(p.id).map_err(internal)?;
        self.db.load_labels().map_err(internal)
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

/// Accepts a list id or a case-insensitive name.
fn resolve_list(lists: &[List], spec: &str) -> Result<i64> {
    let spec = spec.trim();
    if let Ok(id) = spec.parse::<i64>() {
        if lists.iter().any(|l| l.id == id) {
            return Ok(id);
        }
    }
    lists
        .iter()
        .find(|l| l.name.eq_ignore_ascii_case(spec))
        .map(|l| l.id)
        .ok_or_else(|| {
            let names: Vec<&str> = lists.iter().map(|l| l.name.as_str()).collect();
            invalid(format!("unknown list {spec:?}; available: {names:?}"))
        })
}

/// The same, except that "all" asks for every list at once.
fn resolve_list_filter(lists: &[List], spec: &str) -> Result<Option<i64>> {
    if spec.trim().eq_ignore_ascii_case("all") {
        return Ok(None);
    }
    resolve_list(lists, spec).map(Some)
}

fn require_lists(settings: &Settings) -> Result<()> {
    if settings.lists_enabled {
        Ok(())
    } else {
        Err(invalid(
            "the list feature is turned off; enable it with update_settings {lists_enabled: true}",
        ))
    }
}

/// Accepts a label id or a case-insensitive name.
fn resolve_label(labels: &[Label], spec: &str) -> Result<i64> {
    let spec = spec.trim();
    if let Ok(id) = spec.parse::<i64>() {
        if labels.iter().any(|l| l.id == id) {
            return Ok(id);
        }
    }
    labels
        .iter()
        .find(|l| l.name.eq_ignore_ascii_case(spec))
        .map(|l| l.id)
        .ok_or_else(|| {
            let names: Vec<&str> = labels.iter().map(|l| l.name.as_str()).collect();
            invalid(format!("unknown label {spec:?}; available: {names:?}"))
        })
}

fn require_labels(settings: &Settings) -> Result<()> {
    if settings.labels_enabled {
        Ok(())
    } else {
        Err(invalid(
            "the label feature is turned off; enable it with update_settings {labels_enabled: true}",
        ))
    }
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

/// A window dimension the window can actually take.
fn check_size(size: i32, min: i32, field: &str) -> Result<i32> {
    if (min..=Settings::MAX_WINDOW_SIZE).contains(&size) {
        Ok(size)
    } else {
        Err(invalid(format!(
            "{field} must be between {min} and {}, got {size}",
            Settings::MAX_WINDOW_SIZE
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
            date_first: None,
            labels_enabled: None,
            lists_enabled: None,
            default_list: None,
            current_list: None,
            open_on: None,
            window_width: None,
            window_height: None,
            mcp_http_enabled: None,
            mcp_http_port: None,
        }
    }

    fn add(api: &Api, title: &str, priority: Option<&str>) -> Result<TaskView> {
        api.add_task(AddTaskParams {
            title: title.into(),
            priority: priority.map(Into::into),
            labels: None,
            list: None,
        })
    }

    /// An all-`None` task patch for the given id, for the same reason.
    fn edit(id: i64) -> UpdateTaskParams {
        UpdateTaskParams {
            id,
            title: None,
            priority: None,
            labels: None,
            list: None,
            done: None,
        }
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
                done: Some(true),
                ..edit(added.id)
            })
            .unwrap();
        assert!(updated.done);

        let open = api
            .list_tasks(ListTasksParams {
                include_done: Some(false),
                list: None,
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
            done: Some(true),
            ..edit(done.id)
        })
        .unwrap();

        api.add_label(AddLabelParams {
            name: "work".into(),
            color: None,
        })
        .unwrap();

        let all = api
            .snapshot(ListTasksParams {
                include_done: None,
                list: None,
            })
            .unwrap();
        assert_eq!(all.tasks.len(), 2);
        assert_eq!(all.priorities.len(), 4);
        // The label catalogue rides along, so a client can draw a picker
        // without asking again, and so does the sidebar.
        assert_eq!(all.labels.len(), 1);
        assert_eq!(all.lists.len(), 1);
        assert!(all.settings.priorities_enabled);

        assert_eq!(
            api.snapshot(ListTasksParams {
                include_done: Some(false),
                list: None,
            })
            .unwrap()
            .tasks
            .len(),
            1
        );
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

    /// Switches labels on, since everything about them is off by default.
    fn with_labels(api: &Api) {
        api.update_settings(UpdateSettingsParams {
            labels_enabled: Some(true),
            ..patch()
        })
        .unwrap();
    }

    fn label_names(view: &TaskView) -> Vec<&str> {
        view.labels.iter().map(|l| l.name.as_str()).collect()
    }

    /// Switches lists on, since they are off by default.
    fn with_lists(api: &Api) {
        api.update_settings(UpdateSettingsParams {
            lists_enabled: Some(true),
            ..patch()
        })
        .unwrap();
    }

    #[test]
    fn manages_lists() {
        let db = db();
        let api = Api::new(&db);
        // There is always one, so that tasks have somewhere to be.
        assert_eq!(api.list_lists().unwrap().len(), 1);

        let work = api
            .add_list(AddListParams {
                name: "Work".into(),
            })
            .unwrap();
        assert!(api
            .add_list(AddListParams {
                name: "  work ".into()
            })
            .is_err());

        let renamed = api
            .update_list(UpdateListParams {
                id: work.id,
                name: "Office".into(),
            })
            .unwrap();
        assert_eq!(renamed[1].name, "Office");

        let first = renamed[0].id;
        let reordered = api
            .reorder_lists(ReorderListsParams {
                ids: vec![work.id, first],
            })
            .unwrap();
        assert_eq!(reordered[0].name, "Office");
        assert!(api
            .reorder_lists(ReorderListsParams { ids: vec![work.id] })
            .is_err());

        let left = api
            .delete_list(DeleteListParams {
                id: work.id,
                move_to: None,
            })
            .unwrap();
        assert_eq!(left.len(), 1);
        // The last one stays: a task always belongs somewhere.
        assert!(api
            .delete_list(DeleteListParams {
                id: left[0].id,
                move_to: None
            })
            .is_err());
    }

    #[test]
    fn lists_are_off_by_default() {
        let db = db();
        let api = Api::new(&db);
        assert!(!api.get_settings().unwrap().lists_enabled);
        api.add_list(AddListParams {
            name: "Work".into(),
        })
        .unwrap();

        // `@` is plain text, and a task reports no list at all.
        let added = add(&api, "@work literal at", None).unwrap();
        assert_eq!(added.title, "@work literal at");
        assert!(added.list.is_none());
        assert!(api
            .update_task(UpdateTaskParams {
                list: Some("Work".into()),
                ..edit(added.id)
            })
            .is_err());

        with_lists(&api);
        let filed = add(&api, "@work pay the invoice", None).unwrap();
        assert_eq!(filed.title, "pay the invoice");
        assert_eq!(filed.list.as_ref().unwrap().name, "Work");
    }

    #[test]
    fn an_unknown_at_list_files_the_task_anyway() {
        let db = db();
        let api = Api::new(&db);
        with_lists(&api);

        // Typing a name never creates a list: the task lands where it would
        // have gone, and the caller can see which list that was.
        let added = add(&api, "@nope buy milk", None).unwrap();
        assert_eq!(added.title, "buy milk");
        assert_eq!(added.list.as_ref().unwrap().name, "Tasks");
        assert_eq!(api.list_lists().unwrap().len(), 1);

        // Naming one outright is an error instead, like an unknown priority.
        assert!(api
            .add_task(AddTaskParams {
                title: "x".into(),
                priority: None,
                labels: None,
                list: Some("nope".into()),
            })
            .is_err());
    }

    #[test]
    fn tasks_move_between_lists() {
        let db = db();
        let api = Api::new(&db);
        with_lists(&api);
        let work = api
            .add_list(AddListParams {
                name: "Work".into(),
            })
            .unwrap();
        let task = add(&api, "invoice", None).unwrap();
        assert_eq!(task.list.as_ref().unwrap().name, "Tasks");

        let moved = api
            .update_task(UpdateTaskParams {
                list: Some("work".into()),
                ..edit(task.id)
            })
            .unwrap();
        assert_eq!(moved.list.as_ref().unwrap().id, work.id);

        // Listing follows the list showing, and "all" ignores it.
        api.update_settings(UpdateSettingsParams {
            current_list: Some("Tasks".into()),
            ..patch()
        })
        .unwrap();
        assert!(api
            .list_tasks(ListTasksParams {
                include_done: None,
                list: None
            })
            .unwrap()
            .is_empty());
        assert_eq!(
            api.list_tasks(ListTasksParams {
                include_done: None,
                list: Some("all".into())
            })
            .unwrap()
            .len(),
            1
        );
        // A new task joins the list being shown.
        let next = add(&api, "dishes", None).unwrap();
        assert_eq!(next.list.as_ref().unwrap().name, "Tasks");
    }

    #[test]
    fn deleting_a_list_keeps_its_tasks_and_settles_the_settings() {
        let db = db();
        let api = Api::new(&db);
        with_lists(&api);
        let work = api
            .add_list(AddListParams {
                name: "Work".into(),
            })
            .unwrap();
        api.update_settings(UpdateSettingsParams {
            default_list: Some("Work".into()),
            current_list: Some("Work".into()),
            ..patch()
        })
        .unwrap();
        let task = add(&api, "invoice", None).unwrap();
        assert_eq!(task.list.as_ref().unwrap().id, work.id);

        api.delete_list(DeleteListParams {
            id: work.id,
            move_to: None,
        })
        .unwrap();
        // The task survives in the remaining list, and the settings that
        // pointed at the deleted one have been settled.
        let settings = api.get_settings().unwrap();
        assert_eq!(settings.default_list_id, None);
        assert!(settings.current_list_id.is_some());
        let all = api
            .list_tasks(ListTasksParams {
                include_done: None,
                list: Some("all".into()),
            })
            .unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].list.as_ref().unwrap().name, "Tasks");
    }

    #[test]
    fn manages_labels() {
        let db = db();
        let api = Api::new(&db);
        assert!(api.list_labels().unwrap().is_empty());

        let work = api
            .add_label(AddLabelParams {
                name: "work".into(),
                color: None,
            })
            .unwrap();
        assert!(task::PALETTE.contains(&work.color.as_str()));
        api.add_label(AddLabelParams {
            name: "admin".into(),
            color: Some("#ABCDEF".into()),
        })
        .unwrap();
        // Alphabetical, and the color was lower-cased on the way in.
        let all = api.list_labels().unwrap();
        assert_eq!(
            all.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            ["admin", "work"]
        );
        assert_eq!(all[0].color, "#abcdef");

        // One label per name, whatever the case.
        assert!(api
            .add_label(AddLabelParams {
                name: "Work".into(),
                color: None,
            })
            .is_err());

        let renamed = api
            .update_label(UpdateLabelParams {
                id: work.id,
                name: Some("urgent".into()),
                color: Some("#c01c28".into()),
            })
            .unwrap();
        assert_eq!(
            renamed.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            ["admin", "urgent"]
        );
        // Renaming onto another label's name is refused too.
        assert!(api
            .update_label(UpdateLabelParams {
                id: work.id,
                name: Some("ADMIN".into()),
                color: None,
            })
            .is_err());

        let left = api.delete_label(IdParams { id: work.id }).unwrap();
        assert_eq!(left.len(), 1);
        assert!(api.delete_label(IdParams { id: work.id }).is_err());
    }

    #[test]
    fn labels_are_off_by_default() {
        let db = db();
        let api = Api::new(&db);
        assert!(!api.get_settings().unwrap().labels_enabled);
        api.add_label(AddLabelParams {
            name: "work".into(),
            color: None,
        })
        .unwrap();

        // `#` is plain text, no label is attached, and none is reported.
        let added = add(&api, "#work literal hash", None).unwrap();
        assert_eq!(added.title, "#work literal hash");
        assert!(added.labels.is_empty());
        assert_eq!(api.list_labels().unwrap().len(), 1);

        // Naming labels outright is refused while the feature is off.
        assert!(api
            .update_task(UpdateTaskParams {
                labels: Some(vec!["work".into()]),
                ..edit(added.id)
            })
            .is_err());

        with_labels(&api);
        let tagged = add(&api, "#work pay rent", None).unwrap();
        assert_eq!(tagged.title, "pay rent");
        assert_eq!(label_names(&tagged), ["work"]);
        // Turning the feature off hides assignments without losing them.
        api.update_settings(UpdateSettingsParams {
            labels_enabled: Some(false),
            ..patch()
        })
        .unwrap();
        let hidden = api
            .list_tasks(ListTasksParams {
                include_done: None,
                list: None,
            })
            .unwrap();
        assert!(hidden.iter().all(|t| t.labels.is_empty()));
        with_labels(&api);
        let shown = api
            .list_tasks(ListTasksParams {
                include_done: None,
                list: None,
            })
            .unwrap();
        assert!(shown.iter().any(|t| label_names(t) == ["work"]));
    }

    #[test]
    fn typed_tags_create_labels_and_leave_the_title_alone() {
        let db = db();
        let api = Api::new(&db);
        with_labels(&api);

        let added = add(&api, "!! #work #Work #q4 Email Sam about #3421", None).unwrap();
        // Only the leading run is read; the '#' in the title stays put.
        assert_eq!(added.title, "Email Sam about #3421");
        assert_eq!(label_names(&added), ["q4", "work"]);
        assert_eq!(added.priority.as_ref().unwrap().name, "Medium");

        // A second task reuses the labels rather than making more.
        let again = add(&api, "#Q4 file taxes", None).unwrap();
        assert_eq!(label_names(&again), ["q4"]);
        assert_eq!(api.list_labels().unwrap().len(), 2);

        // Labels can also be named outright, alongside the tags.
        let both = api
            .add_task(AddTaskParams {
                title: "#work ship it".into(),
                priority: None,
                labels: Some(vec!["q4".into()]),
                list: None,
            })
            .unwrap();
        assert_eq!(label_names(&both), ["q4", "work"]);
        // An unknown name is an error, unlike an unknown tag.
        assert!(api
            .add_task(AddTaskParams {
                title: "x".into(),
                priority: None,
                labels: Some(vec!["nope".into()]),
                list: None,
            })
            .is_err());
    }

    #[test]
    fn update_task_replaces_the_whole_label_set() {
        let db = db();
        let api = Api::new(&db);
        with_labels(&api);
        let task = add(&api, "#work #home tidy up", None).unwrap();
        assert_eq!(label_names(&task), ["home", "work"]);

        let one = api
            .update_task(UpdateTaskParams {
                labels: Some(vec!["WORK".into()]),
                ..edit(task.id)
            })
            .unwrap();
        assert_eq!(label_names(&one), ["work"]);

        // Leaving the field out changes nothing.
        let untouched = api
            .update_task(UpdateTaskParams {
                done: Some(true),
                ..edit(task.id)
            })
            .unwrap();
        assert_eq!(label_names(&untouched), ["work"]);

        // An empty list clears them.
        let none = api
            .update_task(UpdateTaskParams {
                labels: Some(vec![]),
                ..edit(task.id)
            })
            .unwrap();
        assert!(none.labels.is_empty());

        assert!(api
            .update_task(UpdateTaskParams {
                labels: Some(vec!["nope".into()]),
                ..edit(task.id)
            })
            .is_err());
    }

    #[test]
    fn renaming_a_task_leaves_hashes_alone() {
        let db = db();
        let api = Api::new(&db);
        with_labels(&api);
        let task = add(&api, "pay rent", None).unwrap();

        let renamed = api
            .update_task(UpdateTaskParams {
                title: Some("#work pay rent".into()),
                ..edit(task.id)
            })
            .unwrap();
        // A rename is a rename: no tag is read and no label is created.
        assert_eq!(renamed.title, "#work pay rent");
        assert!(renamed.labels.is_empty());
        assert!(api.list_labels().unwrap().is_empty());
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
    fn date_first_is_off_by_default_and_toggles() {
        let db = db();
        let api = Api::new(&db);
        assert!(!api.get_settings().unwrap().date_first);

        let on = api
            .update_settings(UpdateSettingsParams {
                date_first: Some(true),
                ..patch()
            })
            .unwrap();
        assert!(on.date_first);
        // A patch leaves everything it does not name alone.
        assert_eq!(on.sort_order, SortOrder::PriorityOldest);

        let off = api
            .update_settings(UpdateSettingsParams {
                date_first: Some(false),
                ..patch()
            })
            .unwrap();
        assert!(!off.date_first);
    }

    #[test]
    fn window_size_is_configurable_within_limits() {
        let db = db();
        let api = Api::new(&db);
        let defaults = api.get_settings().unwrap();
        assert_eq!((defaults.window_width, defaults.window_height), (420, 600));

        let changed = api
            .update_settings(UpdateSettingsParams {
                window_width: Some(900),
                window_height: Some(700),
                ..patch()
            })
            .unwrap();
        assert_eq!((changed.window_width, changed.window_height), (900, 700));

        // Too small for the window to render, so it is refused outright
        // rather than saved and silently ignored.
        let err = api
            .update_settings(UpdateSettingsParams {
                window_width: Some(100),
                ..patch()
            })
            .unwrap_err();
        assert!(
            matches!(err, ApiError::Invalid(ref m) if m.contains("window_width")),
            "unhelpful error: {err:?}"
        );
        assert_eq!(api.get_settings().unwrap().window_width, 900);
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
            .list_tasks(ListTasksParams {
                include_done: None,
                list: None,
            })
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
