//! SQLite persistence. Two tables, one connection, plain synchronous calls —
//! the data is small and every call finishes in microseconds.
//!
//! Pre-release: schema changes are made in place, no migrations. To start
//! over, stop the background service first, then delete the file:
//!
//! ```text
//! systemctl --user stop dagr      # it holds the database open
//! rm ~/.local/share/dagr/dagr.db
//! ```
//!
//! The window and `dagr serve` are separate processes sharing this file, so
//! the database runs in WAL mode and readers poll `data_version` to notice
//! each other's commits.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};

use crate::settings::{Settings, SortOrder};
use crate::task::{self, Label, List, Priority, Task};

/// Seeded into an empty database, highest first. The names are translated
/// once, when the database is created: from then on they are the user's own
/// data, and changing language leaves them alone.
fn default_priorities() -> [(String, &'static str); 4] {
    [
        (crate::tr!("seed.priority.high"), "#e01b24"),
        (crate::tr!("seed.priority.medium"), "#ff7800"),
        (crate::tr!("seed.priority.low"), "#3584e4"),
        (crate::tr!("seed.priority.none"), "#9a9996"),
    ]
}

pub struct Db {
    conn: Connection,
}

impl Db {
    /// `~/.local/share/dagr/dagr.db` (honours `XDG_DATA_HOME`).
    pub fn default_path() -> PathBuf {
        let mut path = gtk::glib::user_data_dir();
        path.push("dagr");
        path.push("dagr.db");
        path
    }

    /// Opens (and creates if needed) the default database.
    pub fn open() -> Result<Self> {
        Self::open_at(&Self::default_path())
    }

    /// Opens (and creates if needed) a database at `path`, including its
    /// parent directory.
    pub fn open_at(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("creating data directory {}", dir.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening database {}", path.display()))?;
        Self::init(conn)
    }

    /// A throwaway database for tests.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS priorities (
                id       INTEGER PRIMARY KEY,
                name     TEXT    NOT NULL,
                color    TEXT    NOT NULL,
                position INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS lists (
            id       INTEGER PRIMARY KEY,
            name     TEXT    NOT NULL COLLATE NOCASE UNIQUE,
            position INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS labels (
            id    INTEGER PRIMARY KEY,
            name  TEXT    NOT NULL COLLATE NOCASE UNIQUE,
            color TEXT    NOT NULL
         );
         CREATE TABLE IF NOT EXISTS tasks (
                id           INTEGER PRIMARY KEY,
                title        TEXT    NOT NULL,
                priority_id  INTEGER NOT NULL REFERENCES priorities(id),
            list_id      INTEGER REFERENCES lists(id),
                done         INTEGER NOT NULL DEFAULT 0,
                created_at   INTEGER NOT NULL,
                completed_at INTEGER
             );
             CREATE TABLE IF NOT EXISTS task_labels (
            task_id  INTEGER NOT NULL REFERENCES tasks(id)  ON DELETE CASCADE,
            label_id INTEGER NOT NULL REFERENCES labels(id) ON DELETE CASCADE,
            PRIMARY KEY (task_id, label_id)
         );
         CREATE TABLE IF NOT EXISTS settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );",
        )?;
        let db = Self { conn };
        if db.load_priorities()?.is_empty() {
            for (name, color) in default_priorities() {
                db.insert_priority(&name, color)?;
            }
        }
        // There is always at least one list, even while the feature is off:
        // it is where every task lives until someone makes another.
        if db.load_lists()?.is_empty() {
            db.insert_list(&crate::tr!("seed.list"))?;
        }
        db.settle_list_column()?;
        Ok(db)
    }

    /// Changes whenever *another* connection commits. Poll it to notice edits
    /// made by the MCP server (or a second window) without file watching.
    pub fn data_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA data_version", [], |row| row.get(0))?)
    }

    // --- settings --------------------------------------------------------

    pub fn settings(&self) -> Result<Settings> {
        let mut stmt = self.conn.prepare("SELECT key, value FROM settings")?;
        let pairs: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(Settings::from_pairs(
            pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        ))
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (key, value) in settings.to_pairs() {
            tx.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params![key, value],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    // --- priorities ------------------------------------------------------

    /// All priorities, highest (position 0) first.
    pub fn load_priorities(&self) -> Result<Vec<Priority>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, color, position FROM priorities ORDER BY position ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Priority {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
                position: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Appends a new priority at the bottom (lowest).
    pub fn insert_priority(&self, name: &str, color: &str) -> Result<Priority> {
        let position: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM priorities",
            [],
            |row| row.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO priorities (name, color, position) VALUES (?1, ?2, ?3)",
            params![name, color, position],
        )?;
        Ok(Priority {
            id: self.conn.last_insert_rowid(),
            name: name.to_string(),
            color: color.to_string(),
            position,
        })
    }

    pub fn rename_priority(&self, id: i64, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE priorities SET name = ?1 WHERE id = ?2",
            params![name, id],
        )?;
        Ok(())
    }

    pub fn set_priority_color(&self, id: i64, color: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE priorities SET color = ?1 WHERE id = ?2",
            params![color, id],
        )?;
        Ok(())
    }

    /// Rewrites positions so that `ids[0]` is the highest priority.
    pub fn set_priority_order(&self, ids: &[i64]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (position, id) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE priorities SET position = ?1 WHERE id = ?2",
                params![position as i64, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Removes a priority. Its tasks move to the next lower priority (or the
    /// next higher one if it was the lowest). The last priority cannot go.
    pub fn delete_priority(&self, id: i64) -> Result<()> {
        let priorities = self.load_priorities()?;
        if priorities.len() <= 1 {
            bail!("At least one priority is required");
        }
        let Some(index) = priorities.iter().position(|p| p.id == id) else {
            bail!("Priority no longer exists");
        };
        let fallback = if index + 1 < priorities.len() {
            priorities[index + 1].id
        } else {
            priorities[index - 1].id
        };
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE tasks SET priority_id = ?1 WHERE priority_id = ?2",
            params![fallback, id],
        )?;
        tx.execute("DELETE FROM priorities WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Databases written before lists existed have no `list_id` column, and
    /// rows added by one of them have it empty. Both are cheap to settle here,
    /// which is worth the few lines to avoid asking anyone to delete their
    /// tasks. Pre-release this is the only migration there is.
    fn settle_list_column(&self) -> Result<()> {
        let has_column = self
            .conn
            .prepare("SELECT 1 FROM pragma_table_info('tasks') WHERE name = 'list_id'")?
            .exists([])?;
        if !has_column {
            self.conn.execute(
                "ALTER TABLE tasks ADD COLUMN list_id INTEGER REFERENCES lists(id)",
                [],
            )?;
        }
        self.conn.execute(
            "UPDATE tasks SET list_id = (SELECT id FROM lists ORDER BY position ASC, id ASC LIMIT 1)
             WHERE list_id IS NULL",
            [],
        )?;
        Ok(())
    }

    // --- lists -----------------------------------------------------------

    /// Every list, in sidebar order.
    pub fn load_lists(&self) -> Result<Vec<List>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, position FROM lists ORDER BY position ASC, id ASC")?;
        let rows = stmt.query_map([], |row| {
            Ok(List {
                id: row.get(0)?,
                name: row.get(1)?,
                position: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Appends a list at the bottom of the sidebar.
    pub fn insert_list(&self, name: &str) -> Result<List> {
        let position: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM lists",
            [],
            |row| row.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO lists (name, position) VALUES (?1, ?2)",
            params![name, position],
        )?;
        Ok(List {
            id: self.conn.last_insert_rowid(),
            name: name.to_string(),
            position,
        })
    }

    pub fn rename_list(&self, id: i64, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE lists SET name = ?1 WHERE id = ?2",
            params![name, id],
        )?;
        Ok(())
    }

    /// Rewrites positions so that `ids[0]` is the top of the sidebar.
    pub fn set_list_order(&self, ids: &[i64]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (position, id) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE lists SET position = ?1 WHERE id = ?2",
                params![position as i64, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Removes a list; its tasks move to `fallback`. The last list cannot go,
    /// because every task needs somewhere to be.
    pub fn delete_list(&self, id: i64, fallback: i64) -> Result<()> {
        let lists = self.load_lists()?;
        if lists.len() <= 1 {
            bail!("At least one list is required");
        }
        if !lists.iter().any(|l| l.id == id) {
            bail!("List no longer exists");
        }
        if fallback == id || !lists.iter().any(|l| l.id == fallback) {
            bail!("Tasks must move to another list");
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE tasks SET list_id = ?1 WHERE list_id = ?2",
            params![fallback, id],
        )?;
        tx.execute("DELETE FROM lists WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// How many tasks a list holds, which is what the delete confirmation and
    /// the sidebar counts need.
    pub fn count_in_list(&self, id: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM tasks WHERE list_id = ?1",
            params![id],
            |row| row.get(0),
        )?)
    }

    /// The list a `@name` refers to, matched without case. `None` if there is
    /// no such list: typing one never creates it.
    pub fn list_by_name(&self, name: &str) -> Result<Option<List>> {
        let name = name.trim();
        Ok(self
            .load_lists()?
            .into_iter()
            .find(|l| l.name.eq_ignore_ascii_case(name)))
    }

    /// The list new tasks go to: the configured default, or the first one.
    pub fn default_list(&self) -> Result<i64> {
        let lists = self.load_lists()?;
        let configured = self.settings()?.default_list_id;
        let id = configured
            .filter(|id| lists.iter().any(|l| l.id == *id))
            .or_else(|| lists.first().map(|l| l.id));
        id.ok_or_else(|| anyhow::anyhow!("no lists defined"))
    }

    // --- labels ----------------------------------------------------------

    /// Every label, alphabetically. An empty list is the normal state: unlike
    /// priorities, nothing is seeded.
    pub fn load_labels(&self) -> Result<Vec<Label>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, color FROM labels ORDER BY name COLLATE NOCASE ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Label {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn insert_label(&self, name: &str, color: &str) -> Result<Label> {
        self.conn.execute(
            "INSERT INTO labels (name, color) VALUES (?1, ?2)",
            params![name, color],
        )?;
        Ok(Label {
            id: self.conn.last_insert_rowid(),
            name: name.to_string(),
            color: color.to_string(),
        })
    }

    pub fn rename_label(&self, id: i64, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE labels SET name = ?1 WHERE id = ?2",
            params![name, id],
        )?;
        Ok(())
    }

    pub fn set_label_color(&self, id: i64, color: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE labels SET color = ?1 WHERE id = ?2",
            params![color, id],
        )?;
        Ok(())
    }

    /// Removes a label, and with it every assignment (the foreign key
    /// cascades). No fallback and no "last one" rule, unlike a priority: a
    /// task with no labels is a perfectly good task.
    pub fn delete_label(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM labels WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Ids for typed `#tag` names, creating the ones that do not exist yet
    /// with the next palette color. Matching ignores case, so `#Work` finds
    /// `work`. Both front doors go through here, so a tag means the same thing
    /// typed in the window as sent by an assistant.
    pub fn ensure_labels(&self, names: &[String]) -> Result<Vec<i64>> {
        let tx = self.conn.unchecked_transaction()?;
        let mut labels = self.load_labels()?;
        let mut ids = Vec::new();
        for name in names {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            match labels.iter().find(|l| l.name.eq_ignore_ascii_case(name)) {
                Some(existing) => ids.push(existing.id),
                None => {
                    let color = task::next_color(labels.len());
                    tx.execute(
                        "INSERT INTO labels (name, color) VALUES (?1, ?2)",
                        params![name, color],
                    )?;
                    let id = self.conn.last_insert_rowid();
                    labels.push(Label {
                        id,
                        name: name.to_string(),
                        color: color.to_string(),
                    });
                    ids.push(id);
                }
            }
        }
        tx.commit()?;
        Ok(ids)
    }

    /// Replaces a task's labels with exactly these.
    pub fn set_task_labels(&self, task_id: i64, label_ids: &[i64]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM task_labels WHERE task_id = ?1",
            params![task_id],
        )?;
        for label_id in label_ids {
            // OR IGNORE so naming the same label twice attaches it once, and
            // a label deleted underneath us is skipped instead of failing.
            tx.execute(
                "INSERT OR IGNORE INTO task_labels (task_id, label_id)
                 SELECT ?1, id FROM labels WHERE id = ?2",
                params![task_id, label_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Every task's labels, keyed by task id, alphabetically per task.
    fn task_labels(&self) -> Result<HashMap<i64, Vec<i64>>> {
        let mut stmt = self.conn.prepare(
            "SELECT tl.task_id, tl.label_id
             FROM task_labels tl
             JOIN labels l ON l.id = tl.label_id
             ORDER BY l.name COLLATE NOCASE ASC, l.id ASC",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
        let mut map: HashMap<i64, Vec<i64>> = HashMap::new();
        for row in rows {
            let (task_id, label_id) = row?;
            map.entry(task_id).or_default().push(label_id);
        }
        Ok(map)
    }

    // --- tasks -----------------------------------------------------------

    /// Every task in display order: open before done, then per the configured
    /// sort order (priority only counts while the feature is on). `id` breaks
    /// ties for tasks created in the same second, so undoing a delete puts a
    /// task back in its old slot.
    ///
    /// With `date_first` on, whole calendar days move as a block ahead of
    /// priority: the oldest day leads, and the sort order decides within it.
    ///
    /// Labels come from a second flat query rather than one statement per
    /// task, and arrive alphabetically, so two equal tasks always compare
    /// equal and every consumer gets the same order.
    ///
    /// While lists are on this is the list currently showing; `load_in` asks
    /// for another one.
    pub fn load_all(&self) -> Result<Vec<Task>> {
        let settings = self.settings()?;
        let showing = settings
            .lists_enabled
            .then_some(settings.current_list_id)
            .flatten();
        self.load_in(showing)
    }

    /// Tasks in one list, or in every list when given `None`.
    pub fn load_in(&self, list_id: Option<i64>) -> Result<Vec<Task>> {
        let settings = self.settings()?;
        let mut clauses = vec!["t.done ASC"];
        if settings.date_first {
            // `localtime` so a task added at 00:30 counts as that day, not the
            // one before. If the timezone cannot be resolved the expression is
            // NULL for every row, which ties them and leaves the order below
            // in charge - degraded, never broken.
            clauses.push(match settings.sort_order {
                SortOrder::PriorityNewest | SortOrder::Newest => {
                    "date(t.created_at, 'unixepoch', 'localtime') DESC"
                }
                _ => "date(t.created_at, 'unixepoch', 'localtime') ASC",
            });
        }
        if settings.priorities_enabled && settings.sort_order.uses_priority() {
            clauses.push("p.position ASC");
        }
        clauses.push(match settings.sort_order {
            SortOrder::PriorityOldest | SortOrder::Oldest => "t.created_at ASC, t.id ASC",
            SortOrder::PriorityNewest | SortOrder::Newest => "t.created_at DESC, t.id DESC",
            SortOrder::Title => "t.title COLLATE NOCASE ASC, t.id ASC",
        });
        let order = format!("ORDER BY {}", clauses.join(", "));
        let filter = match list_id {
            Some(_) => "WHERE t.list_id = ?1",
            None => "",
        };
        let mut stmt = self.conn.prepare(&format!(
            "SELECT t.id, t.title, t.priority_id, t.list_id, t.done, t.created_at, t.completed_at
             FROM tasks t
             JOIN priorities p ON p.id = t.priority_id
             {filter}
             {order}"
        ))?;
        let read = |row: &rusqlite::Row| {
            Ok(Task {
                id: row.get(0)?,
                title: row.get(1)?,
                priority_id: row.get(2)?,
                list_id: row.get(3)?,
                label_ids: Vec::new(),
                done: row.get::<_, i64>(4)? != 0,
                created_at: row.get(5)?,
                completed_at: row.get(6)?,
            })
        };
        let rows = match list_id {
            Some(id) => stmt.query_map(params![id], read)?,
            None => stmt.query_map([], read)?,
        };
        let mut tasks = rows.collect::<Result<Vec<Task>, _>>()?;
        let mut labels = self.task_labels()?;
        for task in &mut tasks {
            if let Some(ids) = labels.remove(&task.id) {
                task.label_ids = ids;
            }
        }
        Ok(tasks)
    }

    /// Adds a task to the default list.
    pub fn insert(&self, title: &str, priority_id: i64) -> Result<Task> {
        self.insert_in(title, priority_id, self.default_list()?)
    }

    pub fn insert_in(&self, title: &str, priority_id: i64, list_id: i64) -> Result<Task> {
        let created_at = now();
        self.conn.execute(
            "INSERT INTO tasks (title, priority_id, list_id, done, created_at)
             VALUES (?1, ?2, ?3, 0, ?4)",
            params![title, priority_id, list_id, created_at],
        )?;
        Ok(Task {
            id: self.conn.last_insert_rowid(),
            title: title.to_string(),
            priority_id,
            list_id,
            label_ids: Vec::new(),
            done: false,
            created_at,
            completed_at: None,
        })
    }

    /// Adds a task to a list carrying typed `#tags`, creating any label that
    /// is new. One call so the window and the API cannot file a task
    /// differently.
    pub fn insert_with_labels(
        &self,
        title: &str,
        priority_id: i64,
        list_id: i64,
        tags: &[String],
    ) -> Result<Task> {
        let mut task = self.insert_in(title, priority_id, list_id)?;
        if tags.is_empty() {
            return Ok(task);
        }
        let ids = self.ensure_labels(tags)?;
        self.set_task_labels(task.id, &ids)?;
        task.label_ids = self.task_labels()?.remove(&task.id).unwrap_or_default();
        Ok(task)
    }

    /// Puts a previously deleted task back, keeping its id, timestamps, list
    /// and labels. If its priority or list was removed in the meantime it gets
    /// the default one, and any label removed in the meantime is not restored.
    pub fn reinsert(&self, task: &Task) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO tasks
                 (id, title, priority_id, list_id, done, created_at, completed_at)
             VALUES (?1, ?2,
                     COALESCE((SELECT id FROM priorities WHERE id = ?3),
                              (SELECT id FROM priorities ORDER BY position DESC, id DESC LIMIT 1)),
                     COALESCE((SELECT id FROM lists WHERE id = ?4),
                              (SELECT id FROM lists ORDER BY position ASC, id ASC LIMIT 1)),
                     ?5, ?6, ?7)",
            params![
                task.id,
                task.title,
                task.priority_id,
                task.list_id,
                task.done as i64,
                task.created_at,
                task.completed_at,
            ],
        )?;
        // Deleting the task cascaded its labels away, and REPLACE above
        // deletes before it inserts, so they have to be put back by hand.
        for label_id in &task.label_ids {
            tx.execute(
                "INSERT OR IGNORE INTO task_labels (task_id, label_id)
                 SELECT ?1, id FROM labels WHERE id = ?2",
                params![task.id, label_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_list(&self, id: i64, list_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE tasks SET list_id = ?1 WHERE id = ?2",
            params![list_id, id],
        )?;
        Ok(())
    }

    pub fn set_title(&self, id: i64, title: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE tasks SET title = ?1 WHERE id = ?2",
            params![title, id],
        )?;
        Ok(())
    }

    pub fn set_priority(&self, id: i64, priority_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE tasks SET priority_id = ?1 WHERE id = ?2",
            params![priority_id, id],
        )?;
        Ok(())
    }

    pub fn set_done(&self, id: i64, done: bool) -> Result<()> {
        let completed_at = if done { Some(now()) } else { None };
        self.conn.execute(
            "UPDATE tasks SET done = ?1, completed_at = ?2 WHERE id = ?3",
            params![done as i64, completed_at, id],
        )?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Removes every done task and returns them so the caller can offer Undo.
    pub fn clear_completed(&self) -> Result<Vec<Task>> {
        let removed: Vec<Task> = self.load_all()?.into_iter().filter(|t| t.done).collect();
        self.conn.execute("DELETE FROM tasks WHERE done = 1", [])?;
        Ok(removed)
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(db: &Db) -> Vec<String> {
        db.load_all()
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect()
    }

    fn label_names(db: &Db) -> Vec<String> {
        db.load_labels()
            .unwrap()
            .into_iter()
            .map(|l| l.name)
            .collect()
    }

    /// The names of one task's labels, in stored order.
    fn labels_on(db: &Db, id: i64) -> Vec<String> {
        let labels = db.load_labels().unwrap();
        db.load_all()
            .unwrap()
            .into_iter()
            .find(|t| t.id == id)
            .unwrap()
            .label_ids
            .into_iter()
            .map(|id| {
                labels
                    .iter()
                    .find(|l| l.id == id)
                    .map(|l| l.name.clone())
                    .unwrap()
            })
            .collect()
    }

    fn priority_names(db: &Db) -> Vec<String> {
        db.load_priorities()
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect()
    }

    /// Ids of the default priorities: (high, medium, low, none).
    fn defaults(db: &Db) -> (i64, i64, i64, i64) {
        let p = db.load_priorities().unwrap();
        (p[0].id, p[1].id, p[2].id, p[3].id)
    }

    #[test]
    fn fresh_db_has_default_priorities() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(priority_names(&db), ["High", "Medium", "Low", "None"]);
        assert_eq!(db.load_priorities().unwrap().last().unwrap().name, "None");
    }

    #[test]
    fn orders_open_first_then_priority_then_creation() {
        let db = Db::open_in_memory().unwrap();
        let (high, _, low, none) = defaults(&db);
        db.insert("a", none).unwrap();
        let b = db.insert("b", high).unwrap();
        db.insert("c", low).unwrap();
        db.insert("d", high).unwrap();
        assert_eq!(titles(&db), ["b", "d", "c", "a"]);

        db.set_done(b.id, true).unwrap();
        assert_eq!(titles(&db), ["d", "c", "a", "b"]);
        let done_b = db
            .load_all()
            .unwrap()
            .into_iter()
            .find(|t| t.id == b.id)
            .unwrap();
        assert!(done_b.done);
        assert!(done_b.completed_at.is_some());

        db.set_done(b.id, false).unwrap();
        assert_eq!(titles(&db), ["b", "d", "c", "a"]);
    }

    #[test]
    fn reordering_priorities_reorders_tasks() {
        let db = Db::open_in_memory().unwrap();
        let (high, medium, low, none) = defaults(&db);
        db.insert("h", high).unwrap();
        db.insert("n", none).unwrap();
        db.set_priority_order(&[none, low, medium, high]).unwrap();
        assert_eq!(priority_names(&db), ["None", "Low", "Medium", "High"]);
        assert_eq!(titles(&db), ["n", "h"]);
    }

    #[test]
    fn settings_persist_and_change_ordering() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.settings().unwrap(), Settings::default());
        let (high, _, _, none) = defaults(&db);
        db.insert("first, low", none).unwrap();
        db.insert("second, high", high).unwrap();
        assert_eq!(titles(&db), ["second, high", "first, low"]);

        db.save_settings(&Settings {
            priorities_enabled: false,
            ..Settings::default()
        })
        .unwrap();
        assert!(!db.settings().unwrap().priorities_enabled);
        assert_eq!(titles(&db), ["first, low", "second, high"]);
    }

    #[test]
    fn sort_order_setting_changes_ordering() {
        let db = Db::open_in_memory().unwrap();
        let (high, _, _, none) = defaults(&db);
        db.insert("bravo", none).unwrap();
        db.insert("alpha", high).unwrap();
        db.insert("charlie", none).unwrap();
        let set = |order: SortOrder| {
            db.save_settings(&Settings {
                sort_order: order,
                ..Settings::default()
            })
            .unwrap()
        };
        assert_eq!(titles(&db), ["alpha", "bravo", "charlie"]);
        set(SortOrder::PriorityNewest);
        assert_eq!(titles(&db), ["alpha", "charlie", "bravo"]);
        set(SortOrder::Oldest);
        assert_eq!(titles(&db), ["bravo", "alpha", "charlie"]);
        set(SortOrder::Newest);
        assert_eq!(titles(&db), ["charlie", "alpha", "bravo"]);
        set(SortOrder::Title);
        assert_eq!(titles(&db), ["alpha", "bravo", "charlie"]);
    }

    #[test]
    fn date_first_moves_whole_days() {
        let db = Db::open_in_memory().unwrap();
        let (high, _, _, none) = defaults(&db);
        // Fixed noon-UTC stamps: whatever timezone the test machine is in,
        // these two land on one calendar day and the third on the next.
        const NOON: i64 = 1_700_049_600; // 2023-11-15T12:00:00Z
        let old_low = db.insert("old, low", none).unwrap();
        let old_high = db.insert("old, high", high).unwrap();
        let new_high = db.insert("new, high", high).unwrap();
        let backdate = |id, created_at| {
            db.conn
                .execute(
                    "UPDATE tasks SET created_at = ?1 WHERE id = ?2",
                    params![created_at, id],
                )
                .unwrap();
        };
        backdate(old_low.id, NOON);
        backdate(old_high.id, NOON + 3_600);
        backdate(new_high.id, NOON + 86_400);

        let set = |date_first: bool, sort_order: SortOrder| {
            db.save_settings(&Settings {
                date_first,
                sort_order,
                ..Settings::default()
            })
            .unwrap()
        };

        // Off, the default: priority wins outright, across days.
        assert_eq!(titles(&db), ["old, high", "new, high", "old, low"]);

        // On: the older day leads as a block, priority orders within it.
        set(true, SortOrder::PriorityOldest);
        assert_eq!(titles(&db), ["old, high", "old, low", "new, high"]);

        // "Newest first" flips which day leads, but keeps the days whole.
        set(true, SortOrder::PriorityNewest);
        assert_eq!(titles(&db), ["new, high", "old, high", "old, low"]);

        // A plain date order already runs day by day, so grouping changes
        // nothing.
        set(true, SortOrder::Oldest);
        assert_eq!(titles(&db), ["old, low", "old, high", "new, high"]);
        set(false, SortOrder::Oldest);
        assert_eq!(titles(&db), ["old, low", "old, high", "new, high"]);
    }

    #[test]
    fn edits_persist() {
        let db = Db::open_in_memory().unwrap();
        let (_, medium, _, none) = defaults(&db);
        let t = db.insert("old", none).unwrap();
        db.set_title(t.id, "new").unwrap();
        db.set_priority(t.id, medium).unwrap();
        let loaded = db.load_all().unwrap().remove(0);
        assert_eq!(loaded.title, "new");
        assert_eq!(loaded.priority_id, medium);

        db.rename_priority(medium, "Soon").unwrap();
        db.set_priority_color(medium, "#123456").unwrap();
        let p = db.load_priorities().unwrap().remove(1);
        assert_eq!((p.name.as_str(), p.color.as_str()), ("Soon", "#123456"));
    }

    #[test]
    fn deleting_a_priority_moves_its_tasks() {
        let db = Db::open_in_memory().unwrap();
        let (high, medium, low, none) = defaults(&db);
        let t_med = db.insert("m", medium).unwrap();
        let t_none = db.insert("n", none).unwrap();

        db.delete_priority(medium).unwrap(); // middle → next lower
        assert_eq!(
            db.load_all()
                .unwrap()
                .iter()
                .find(|t| t.id == t_med.id)
                .unwrap()
                .priority_id,
            low
        );

        db.delete_priority(none).unwrap(); // lowest → next higher
        assert_eq!(
            db.load_all()
                .unwrap()
                .iter()
                .find(|t| t.id == t_none.id)
                .unwrap()
                .priority_id,
            low
        );

        db.delete_priority(low).unwrap();
        assert_eq!(priority_names(&db), ["High"]);
        assert!(db.delete_priority(high).is_err());
        assert_eq!(priority_names(&db), ["High"]);
    }

    fn list_names(db: &Db) -> Vec<String> {
        db.load_lists()
            .unwrap()
            .into_iter()
            .map(|l| l.name)
            .collect()
    }

    #[test]
    fn a_fresh_database_has_one_list() {
        // Every task needs somewhere to live, so unlike labels there is always
        // at least one list, whether or not the feature is switched on.
        let db = Db::open_in_memory().unwrap();
        assert_eq!(list_names(&db), ["Tasks"]);
        let (_, _, low, _) = defaults(&db);
        let task = db.insert("t", low).unwrap();
        assert_eq!(task.list_id, db.default_list().unwrap());
    }

    #[test]
    fn tasks_load_per_list_once_lists_are_on() {
        let db = Db::open_in_memory().unwrap();
        let (_, _, low, _) = defaults(&db);
        let home = db.default_list().unwrap();
        let work = db.insert_list("Work").unwrap();
        db.insert_in("dishes", low, home).unwrap();
        db.insert_in("invoice", low, work.id).unwrap();

        // Off: the sidebar does not exist, so neither does the filter.
        assert_eq!(titles(&db), ["dishes", "invoice"]);

        let show = |current: Option<i64>| {
            db.save_settings(&Settings {
                lists_enabled: true,
                current_list_id: current,
                ..Settings::default()
            })
            .unwrap()
        };
        show(Some(work.id));
        assert_eq!(titles(&db), ["invoice"]);
        show(Some(home));
        assert_eq!(titles(&db), ["dishes"]);
        // "All tasks" is no filter at all.
        show(None);
        assert_eq!(titles(&db), ["dishes", "invoice"]);
        // `load_in` ignores what is showing and asks for one list.
        let only_work: Vec<String> = db
            .load_in(Some(work.id))
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(only_work, ["invoice"]);
    }

    #[test]
    fn deleting_a_list_moves_its_tasks() {
        let db = Db::open_in_memory().unwrap();
        let (_, _, low, _) = defaults(&db);
        let home = db.default_list().unwrap();
        let work = db.insert_list("Work").unwrap();
        let task = db.insert_in("invoice", low, work.id).unwrap();
        assert_eq!(db.count_in_list(work.id).unwrap(), 1);

        db.delete_list(work.id, home).unwrap();
        assert_eq!(list_names(&db), ["Tasks"]);
        let moved = db
            .load_in(None)
            .unwrap()
            .into_iter()
            .find(|t| t.id == task.id);
        assert_eq!(moved.unwrap().list_id, home);
        // The last list cannot go, and tasks cannot be moved to nowhere.
        assert!(db.delete_list(home, home).is_err());
    }

    #[test]
    fn lists_are_matched_by_name_without_case() {
        let db = Db::open_in_memory().unwrap();
        let work = db.insert_list("Work").unwrap();
        assert_eq!(db.list_by_name("work").unwrap().unwrap().id, work.id);
        assert_eq!(db.list_by_name("  WORK ").unwrap().unwrap().id, work.id);
        assert!(db.list_by_name("nope").unwrap().is_none());
    }

    #[test]
    fn reordering_lists_rewrites_the_sidebar() {
        let db = Db::open_in_memory().unwrap();
        let first = db.load_lists().unwrap()[0].id;
        let work = db.insert_list("Work").unwrap();
        assert_eq!(list_names(&db), ["Tasks", "Work"]);
        db.set_list_order(&[work.id, first]).unwrap();
        assert_eq!(list_names(&db), ["Work", "Tasks"]);
    }

    #[test]
    fn a_fresh_database_has_no_labels() {
        // Unlike priorities, nothing is seeded: the feature starts off and
        // empty.
        let db = Db::open_in_memory().unwrap();
        assert!(db.load_labels().unwrap().is_empty());
    }

    #[test]
    fn labels_are_alphabetical_everywhere() {
        let db = Db::open_in_memory().unwrap();
        let (_, _, low, _) = defaults(&db);
        let task = db.insert("t", low).unwrap();
        let work = db.insert_label("work", "#1c71d8").unwrap();
        let admin = db.insert_label("admin", "#c01c28").unwrap();
        let home = db.insert_label("Home", "#2ec27e").unwrap();
        assert_eq!(label_names(&db), ["admin", "Home", "work"]);

        // Attached in any order, they come back sorted by name.
        db.set_task_labels(task.id, &[work.id, home.id, admin.id])
            .unwrap();
        assert_eq!(labels_on(&db, task.id), ["admin", "Home", "work"]);

        // Setting replaces rather than appends, and names the same label once.
        db.set_task_labels(task.id, &[home.id, home.id]).unwrap();
        assert_eq!(labels_on(&db, task.id), ["Home"]);
        db.set_task_labels(task.id, &[]).unwrap();
        assert!(labels_on(&db, task.id).is_empty());
    }

    #[test]
    fn deleting_a_label_takes_it_off_its_tasks() {
        let db = Db::open_in_memory().unwrap();
        let (_, _, low, _) = defaults(&db);
        let task = db.insert("t", low).unwrap();
        let work = db.insert_label("work", "#1c71d8").unwrap();
        let home = db.insert_label("home", "#2ec27e").unwrap();
        db.set_task_labels(task.id, &[work.id, home.id]).unwrap();

        db.delete_label(work.id).unwrap();
        assert_eq!(label_names(&db), ["home"]);
        assert_eq!(labels_on(&db, task.id), ["home"]);
        // No "last one" rule: a task with no labels is fine, and the task
        // itself is untouched.
        db.delete_label(home.id).unwrap();
        assert!(label_names(&db).is_empty());
        assert_eq!(titles(&db), ["t"]);
    }

    #[test]
    fn typed_tags_create_labels_once() {
        let db = Db::open_in_memory().unwrap();
        let (_, _, low, _) = defaults(&db);
        let list = db.default_list().unwrap();
        let tags =
            |names: &[&str]| -> Vec<String> { names.iter().map(|n| (*n).to_string()).collect() };

        let first = db
            .insert_with_labels("pay rent", low, list, &tags(&["work", "home"]))
            .unwrap();
        assert_eq!(label_names(&db), ["home", "work"]);
        assert_eq!(labels_on(&db, first.id), ["home", "work"]);
        // The task carries them straight away, without a reload.
        assert_eq!(
            labels_on(&db, first.id),
            first
                .label_ids
                .iter()
                .map(|id| {
                    db.load_labels()
                        .unwrap()
                        .into_iter()
                        .find(|l| l.id == *id)
                        .unwrap()
                        .name
                })
                .collect::<Vec<_>>()
        );

        // A tag that exists is reused whatever its case, and a repeat is one.
        let second = db
            .insert_with_labels("call bank", low, list, &tags(&["Work", "work"]))
            .unwrap();
        assert_eq!(label_names(&db), ["home", "work"]);
        assert_eq!(labels_on(&db, second.id), ["work"]);

        // New labels walk the palette, counting only labels.
        let third = db
            .insert_with_labels("x", low, list, &tags(&["q4"]))
            .unwrap();
        assert_eq!(labels_on(&db, third.id), ["q4"]);
        let colors: Vec<String> = db
            .load_labels()
            .unwrap()
            .into_iter()
            .map(|l| l.color)
            .collect();
        assert_eq!(colors.len(), 3);
        assert!(colors
            .iter()
            .all(|c| crate::task::PALETTE.contains(&c.as_str())));
    }

    #[test]
    fn reinsert_drops_labels_that_are_gone() {
        let db = Db::open_in_memory().unwrap();
        let (_, _, low, _) = defaults(&db);
        let task = db.insert("t", low).unwrap();
        let work = db.insert_label("work", "#1c71d8").unwrap();
        let home = db.insert_label("home", "#2ec27e").unwrap();
        db.set_task_labels(task.id, &[work.id, home.id]).unwrap();
        let snapshot = db.load_all().unwrap().into_iter().next().unwrap();

        db.delete(task.id).unwrap();
        db.delete_label(work.id).unwrap();
        db.reinsert(&snapshot).unwrap();
        // The label that survived comes back; the other is quietly skipped.
        assert_eq!(labels_on(&db, task.id), ["home"]);
    }

    #[test]
    fn clear_completed_keeps_labels_for_undo() {
        let db = Db::open_in_memory().unwrap();
        let (_, _, low, _) = defaults(&db);
        let task = db.insert("t", low).unwrap();
        let work = db.insert_label("work", "#1c71d8").unwrap();
        db.set_task_labels(task.id, &[work.id]).unwrap();
        db.set_done(task.id, true).unwrap();

        let removed = db.clear_completed().unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].label_ids, vec![work.id]);
        db.reinsert(&removed[0]).unwrap();
        assert_eq!(labels_on(&db, task.id), ["work"]);
    }

    #[test]
    fn delete_and_reinsert_restore_position() {
        let db = Db::open_in_memory().unwrap();
        let (_, _, low, _) = defaults(&db);
        db.insert("a", low).unwrap();
        let mut b = db.insert("b", low).unwrap();
        db.insert("c", low).unwrap();
        // Labels ride along: deleting the task cascades them away, so undo has
        // to put them back. `assert_eq!(restored, b)` below is the guard.
        let work = db.insert_label("work", "#1c71d8").unwrap();
        db.set_task_labels(b.id, &[work.id]).unwrap();
        b.label_ids = vec![work.id];
        db.delete(b.id).unwrap();
        assert_eq!(titles(&db), ["a", "c"]);
        db.reinsert(&b).unwrap();
        assert_eq!(titles(&db), ["a", "b", "c"]);
        let restored = db
            .load_all()
            .unwrap()
            .into_iter()
            .find(|t| t.id == b.id)
            .unwrap();
        assert_eq!(restored, b);
    }

    #[test]
    fn reinsert_falls_back_when_priority_is_gone() {
        let db = Db::open_in_memory().unwrap();
        let (_, medium, _, none) = defaults(&db);
        let t = db.insert("x", medium).unwrap();
        db.delete(t.id).unwrap();
        db.delete_priority(medium).unwrap();
        db.reinsert(&t).unwrap();
        let restored = db.load_all().unwrap().remove(0);
        assert_eq!(restored.priority_id, none);
    }

    #[test]
    fn clear_completed_returns_removed_tasks() {
        let db = Db::open_in_memory().unwrap();
        let (_, _, _, none) = defaults(&db);
        let a = db.insert("a", none).unwrap();
        db.insert("b", none).unwrap();
        let c = db.insert("c", none).unwrap();
        db.set_done(a.id, true).unwrap();
        db.set_done(c.id, true).unwrap();

        let removed = db.clear_completed().unwrap();
        assert_eq!(
            removed.iter().map(|t| t.id).collect::<Vec<_>>(),
            [a.id, c.id]
        );
        assert_eq!(titles(&db), ["b"]);

        for t in &removed {
            db.reinsert(t).unwrap();
        }
        assert_eq!(titles(&db), ["b", "a", "c"]);
        assert!(db.clear_completed().unwrap().len() == 2);
        assert!(db.clear_completed().unwrap().is_empty());
    }
}
