//! Domain types: tasks and user-defined priorities. No GTK in here.

use serde::Serialize;

/// A user-defined priority level. Lower `position` = higher priority; the
/// order in Preferences is the order of the task list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Priority {
    pub id: i64,
    pub name: String,
    /// CSS color, normally `#rrggbb`.
    pub color: String,
    pub position: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Task {
    pub id: i64,
    pub title: String,
    pub priority_id: i64,
    pub done: bool,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds, set when the task is checked off.
    pub completed_at: Option<i64>,
}

/// Chooses the priority for a new task: the configured default (or the lowest
/// when none is configured or it no longer exists), moved up one level per
/// `!` and clamped at the top. `None` only if there are no priorities at all.
pub fn pick_priority(
    priorities: &[Priority],
    default_id: Option<i64>,
    bangs: usize,
) -> Option<i64> {
    let last = priorities.len().checked_sub(1)?;
    let start = default_id
        .and_then(|id| priorities.iter().position(|p| p.id == id))
        .unwrap_or(last);
    Some(priorities[start.saturating_sub(bangs)].id)
}

/// Splits a leading run of `!` off a typed title so a task can be added with a
/// priority without touching the mouse: `"!! Pay rent"` → `(2, "Pay rent")`.
/// Zero bangs means the lowest priority; each extra `!` moves one level up.
pub fn split_priority_prefix(input: &str) -> (usize, String) {
    let trimmed = input.trim();
    let bangs = trimmed.chars().take_while(|c| *c == '!').count();
    // '!' is a single byte, so the char count is also the byte offset.
    let title = trimmed[bangs..].trim().to_string();
    (bangs, title)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prio(id: i64, position: i64) -> Priority {
        Priority {
            id,
            name: format!("p{id}"),
            color: "#000000".into(),
            position,
        }
    }

    #[test]
    fn pick_priority_starts_at_default_and_climbs() {
        let p = [prio(10, 0), prio(20, 1), prio(30, 2)];
        assert_eq!(pick_priority(&p, None, 0), Some(30)); // lowest
        assert_eq!(pick_priority(&p, None, 1), Some(20));
        assert_eq!(pick_priority(&p, None, 9), Some(10)); // clamped
        assert_eq!(pick_priority(&p, Some(20), 0), Some(20)); // configured
        assert_eq!(pick_priority(&p, Some(20), 1), Some(10));
        assert_eq!(pick_priority(&p, Some(99), 0), Some(30)); // gone → lowest
        assert_eq!(pick_priority(&[], None, 0), None);
    }

    #[test]
    fn split_prefix_counts_bangs() {
        assert_eq!(split_priority_prefix("Buy milk"), (0, "Buy milk".into()));
        assert_eq!(split_priority_prefix("! Buy milk"), (1, "Buy milk".into()));
        assert_eq!(split_priority_prefix("!!Buy milk"), (2, "Buy milk".into()));
        assert_eq!(
            split_priority_prefix("  !!! Buy milk "),
            (3, "Buy milk".into())
        );
        assert_eq!(split_priority_prefix("!!!!"), (4, String::new()));
        assert_eq!(split_priority_prefix("   "), (0, String::new()));
    }
}
