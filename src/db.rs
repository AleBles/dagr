//! SQLite persistence. Two tables, one connection, plain synchronous calls —
//! the data is small and every call finishes in microseconds.
//!
//! Pre-release: schema changes are made in place, no migrations. Delete
//! `~/.local/share/dagr/dagr.db` after changing the schema.
//!
//! The GUI and the MCP server (`dagr --mcp`) are separate processes sharing
//! this file, so the database runs in WAL mode and readers poll
//! `data_version` to notice each other's commits.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};

use crate::settings::{Settings, SortOrder};
use crate::task::{Priority, Task};

/// Seeded into an empty database, highest first.
const DEFAULT_PRIORITIES: [(&str, &str); 4] = [
    ("High", "#e01b24"),
    ("Medium", "#ff7800"),
    ("Low", "#3584e4"),
    ("None", "#9a9996"),
];

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
             CREATE TABLE IF NOT EXISTS tasks (
                id           INTEGER PRIMARY KEY,
                title        TEXT    NOT NULL,
                priority_id  INTEGER NOT NULL REFERENCES priorities(id),
                done         INTEGER NOT NULL DEFAULT 0,
                created_at   INTEGER NOT NULL,
                completed_at INTEGER
             );
             CREATE TABLE IF NOT EXISTS settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );",
        )?;
        let db = Self { conn };
        if db.load_priorities()?.is_empty() {
            for (name, color) in DEFAULT_PRIORITIES {
                db.insert_priority(name, color)?;
            }
        }
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

    // --- tasks -----------------------------------------------------------

    /// Every task in display order: open before done, then per the configured
    /// sort order (priority only counts while the feature is on). `id` breaks
    /// ties for tasks created in the same second, so undoing a delete puts a
    /// task back in its old slot.
    pub fn load_all(&self) -> Result<Vec<Task>> {
        let settings = self.settings()?;
        let mut clauses = vec!["t.done ASC"];
        if settings.priorities_enabled && settings.sort_order.uses_priority() {
            clauses.push("p.position ASC");
        }
        clauses.push(match settings.sort_order {
            SortOrder::PriorityOldest | SortOrder::Oldest => "t.created_at ASC, t.id ASC",
            SortOrder::PriorityNewest | SortOrder::Newest => "t.created_at DESC, t.id DESC",
            SortOrder::Title => "t.title COLLATE NOCASE ASC, t.id ASC",
        });
        let order = format!("ORDER BY {}", clauses.join(", "));
        let mut stmt = self.conn.prepare(&format!(
            "SELECT t.id, t.title, t.priority_id, t.done, t.created_at, t.completed_at
             FROM tasks t
             JOIN priorities p ON p.id = t.priority_id
             {order}"
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok(Task {
                id: row.get(0)?,
                title: row.get(1)?,
                priority_id: row.get(2)?,
                done: row.get::<_, i64>(3)? != 0,
                created_at: row.get(4)?,
                completed_at: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn insert(&self, title: &str, priority_id: i64) -> Result<Task> {
        let created_at = now();
        self.conn.execute(
            "INSERT INTO tasks (title, priority_id, done, created_at) VALUES (?1, ?2, 0, ?3)",
            params![title, priority_id, created_at],
        )?;
        Ok(Task {
            id: self.conn.last_insert_rowid(),
            title: title.to_string(),
            priority_id,
            done: false,
            created_at,
            completed_at: None,
        })
    }

    /// Puts a previously deleted task back, keeping its id and timestamps. If
    /// its priority was removed in the meantime it gets the default one.
    pub fn reinsert(&self, task: &Task) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO tasks (id, title, priority_id, done, created_at, completed_at)
             VALUES (?1, ?2,
                     COALESCE((SELECT id FROM priorities WHERE id = ?3),
                              (SELECT id FROM priorities ORDER BY position DESC, id DESC LIMIT 1)),
                     ?4, ?5, ?6)",
            params![
                task.id,
                task.title,
                task.priority_id,
                task.done as i64,
                task.created_at,
                task.completed_at,
            ],
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

    #[test]
    fn delete_and_reinsert_restore_position() {
        let db = Db::open_in_memory().unwrap();
        let (_, _, low, _) = defaults(&db);
        db.insert("a", low).unwrap();
        let b = db.insert("b", low).unwrap();
        db.insert("c", low).unwrap();
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
