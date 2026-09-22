//! Domain types: tasks, and the user-defined priorities, labels and lists
//! they are filed under. No GTK in here.

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

/// A user-defined tag. A task carries any number of them, and they have no
/// order of their own: labels sort by name everywhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Label {
    pub id: i64,
    pub name: String,
    /// CSS color, normally `#rrggbb`.
    pub color: String,
}

/// A user-defined list of tasks. Every task belongs to exactly one; the order
/// in the sidebar is `position`, lowest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct List {
    pub id: i64,
    pub name: String,
    pub position: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Task {
    pub id: i64,
    pub title: String,
    pub priority_id: i64,
    /// The list this task is filed under. There is always one, even while the
    /// list feature is switched off.
    pub list_id: i64,
    /// Labels on this task, in the order they are displayed (alphabetical).
    /// Empty is the normal case.
    pub label_ids: Vec<i64>,
    pub done: bool,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds, set when the task is checked off.
    pub completed_at: Option<i64>,
}

/// GNOME palette colors handed to newly created priorities and labels, in turn.
pub const PALETTE: [&str; 8] = [
    "#9141ac", // purple
    "#2ec27e", // green
    "#f5c211", // yellow
    "#986a44", // brown
    "#1c71d8", // blue
    "#c01c28", // red
    "#e66100", // orange
    "#5e5c64", // grey
];

/// The palette color a newly created priority or label gets. Both features
/// walk the palette independently, counting only their own.
pub fn next_color(count: usize) -> &'static str {
    PALETTE[count % PALETTE.len()]
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

/// What a typed line meant, once its leading markers are read off.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Prefix {
    /// Leading `!`s. Zero means the default priority; each one moves a level up.
    pub bangs: usize,
    /// Leading `#tag` names, as typed, without case-insensitive duplicates.
    pub tags: Vec<String>,
    /// The `@list` asked for, as typed. Whether such a list exists is the
    /// caller's business.
    pub list: Option<String>,
    pub title: String,
}

/// Splits the leading run of `!`, `#tag` and `@list` markers off a typed line,
/// so a task can be filed without touching the mouse: `"!! #work @home Pay
/// rent"` becomes two bangs, one tag, one list and "Pay rent".
///
/// The run ends at the first word that is no marker at all, so a `#` or `@`
/// further along stays literal: `"#work Buy #2 pencils"` keeps "Buy #2
/// pencils". A feature that is switched off does not read its own marker,
/// which is why the flags are arguments — with labels off, `#work` is simply
/// part of the title. Only the first `@list` counts; a task lives in one list.
pub fn split_prefixes(input: &str, priorities: bool, labels: bool, lists: bool) -> Prefix {
    let mut rest = input.trim();
    let mut prefix = Prefix::default();
    loop {
        if priorities && rest.starts_with('!') {
            // '!' is a single byte, so the char count is also the byte offset.
            let bangs = rest.chars().take_while(|c| *c == '!').count();
            prefix.bangs += bangs;
            rest = rest[bangs..].trim_start();
        } else if labels && rest.starts_with('#') {
            let name: String = rest[1..]
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '#' && *c != '!')
                .collect();
            if name.is_empty() {
                break; // a bare '#' is text, and skipping it would never end
            }
            rest = rest[1 + name.len()..].trim_start();
            if !prefix
                .tags
                .iter()
                .any(|tag| tag.eq_ignore_ascii_case(&name))
            {
                prefix.tags.push(name);
            }
        } else if lists && rest.starts_with('@') {
            if prefix.list.is_some() {
                // A task lives in one list, so a second @name is not a marker
                // — it is where the title starts.
                break;
            }
            let name: String = rest[1..]
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '#' && *c != '!' && *c != '@')
                .collect();
            if name.is_empty() {
                break; // a bare '@' is text, and skipping it would never end
            }
            rest = rest[1 + name.len()..].trim_start();
            prefix.list = Some(name);
        } else {
            break;
        }
    }
    prefix.title = rest.trim().to_string();
    prefix
}

/// Markers a typed line meant, whose feature is switched off. Used to explain
/// why a `!` or a `#tag` was left sitting in the title as plain text.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct Ignored {
    pub priorities: bool,
    pub labels: bool,
    pub lists: bool,
}

impl Ignored {
    pub fn any(self) -> bool {
        self.priorities || self.labels || self.lists
    }
}

/// Reads the line a second time as if both features were on, and reports the
/// markers that only the switched-off ones would have taken.
pub fn ignored_markers(input: &str, priorities: bool, labels: bool, lists: bool) -> Ignored {
    let intended = split_prefixes(input, true, true, true);
    Ignored {
        priorities: !priorities && intended.bangs > 0,
        labels: !labels && !intended.tags.is_empty(),
        lists: !lists && intended.list.is_some(),
    }
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

    /// Every feature on, which is how the parser is used once they are set up.
    fn split(input: &str) -> Prefix {
        split_prefixes(input, true, true, true)
    }

    fn prefix(bangs: usize, tags: &[&str], title: &str) -> Prefix {
        Prefix {
            bangs,
            tags: tags.iter().map(|t| (*t).to_string()).collect(),
            list: None,
            title: title.to_string(),
        }
    }

    /// The same with a `@list`.
    fn in_list(bangs: usize, tags: &[&str], list: &str, title: &str) -> Prefix {
        Prefix {
            list: Some(list.to_string()),
            ..prefix(bangs, tags, title)
        }
    }

    #[test]
    fn split_prefixes_counts_bangs() {
        assert_eq!(split("Buy milk"), prefix(0, &[], "Buy milk"));
        assert_eq!(split("! Buy milk"), prefix(1, &[], "Buy milk"));
        assert_eq!(split("!!Buy milk"), prefix(2, &[], "Buy milk"));
        assert_eq!(split("  !!! Buy milk "), prefix(3, &[], "Buy milk"));
        assert_eq!(split("!!!!"), prefix(4, &[], ""));
        assert_eq!(split("   "), prefix(0, &[], ""));
    }

    #[test]
    fn split_prefixes_reads_tags_until_the_title_starts() {
        assert_eq!(split("#work Pay rent"), prefix(0, &["work"], "Pay rent"));
        assert_eq!(
            split("!! #work #home Pay rent"),
            prefix(2, &["work", "home"], "Pay rent")
        );
        // Markers may interleave, and need no space between them.
        assert_eq!(split("#work#home x"), prefix(0, &["work", "home"], "x"));
        assert_eq!(split("#work!! x"), prefix(2, &["work"], "x"));
        // The same tag twice is the same tag, whatever the case.
        assert_eq!(split("#Work #work x"), prefix(0, &["Work"], "x"));
        // Only the leading run counts: a later '#' belongs to the title.
        assert_eq!(
            split("#work Buy #2 pencils"),
            prefix(0, &["work"], "Buy #2 pencils")
        );
        assert_eq!(split("Buy #2 pencils"), prefix(0, &[], "Buy #2 pencils"));
        // A bare '#' is text, not the start of a tag.
        assert_eq!(split("# x"), prefix(0, &[], "# x"));
        assert_eq!(split("#work"), prefix(0, &["work"], ""));
        // Multi-byte names are sliced on a character boundary.
        assert_eq!(
            split("#Küche einkaufen"),
            prefix(0, &["Küche"], "einkaufen")
        );
    }

    #[test]
    fn split_prefixes_reads_one_list() {
        assert_eq!(
            split("@home water plants"),
            in_list(0, &[], "home", "water plants")
        );
        assert_eq!(
            split("!! #work @home Pay rent"),
            in_list(2, &["work"], "home", "Pay rent")
        );
        // Order among the markers does not matter, only that they lead.
        assert_eq!(
            split("@home !! #work Pay rent"),
            in_list(2, &["work"], "home", "Pay rent")
        );
        // A task lives in one list: a second @name is where the title starts.
        assert_eq!(split("@home @work x"), in_list(0, &[], "home", "@work x"));
        // Later in the line, and on its own, an '@' is ordinary text.
        assert_eq!(split("mail @ bob"), prefix(0, &[], "mail @ bob"));
        assert_eq!(split("@ x"), prefix(0, &[], "@ x"));
        assert_eq!(
            split("ask @bob about it"),
            prefix(0, &[], "ask @bob about it")
        );
        // With lists off the marker is part of the title.
        assert_eq!(
            split_prefixes("@home water plants", true, true, false),
            prefix(0, &[], "@home water plants")
        );
    }

    #[test]
    fn ignored_markers_explain_a_marker_left_as_text() {
        let nothing = Ignored::default();
        assert!(!nothing.any());
        // Nothing to explain while both features are on, or when neither
        // marker was typed.
        assert_eq!(ignored_markers("!! #work x", true, true, true), nothing);
        assert_eq!(ignored_markers("plain text", false, false, false), nothing);

        assert_eq!(
            ignored_markers("!! x", false, true, true),
            Ignored {
                priorities: true,
                labels: false,
                lists: false
            }
        );
        assert_eq!(
            ignored_markers("#work x", true, false, true),
            Ignored {
                priorities: false,
                labels: true,
                lists: false
            }
        );
        assert_eq!(
            ignored_markers("!! #work x", false, false, false),
            Ignored {
                priorities: true,
                labels: true,
                lists: false
            }
        );
        // A '#' that was never a leading tag is not a missed label.
        assert_eq!(
            ignored_markers("Buy #2 pencils", true, false, true),
            nothing
        );
        // A typed @list while lists are off is worth saying too.
        assert_eq!(
            ignored_markers("@home x", true, true, false),
            Ignored {
                priorities: false,
                labels: false,
                lists: true
            }
        );
        // With priorities off, a leading `!!` ends the run before the tag is
        // reached, so the priority switch is the one to point at.
        assert_eq!(
            ignored_markers("!! #work x", false, true, true),
            Ignored {
                priorities: true,
                labels: false,
                lists: false
            }
        );
    }

    #[test]
    fn a_switched_off_feature_leaves_its_marker_alone() {
        assert_eq!(
            split_prefixes("#work !! x", false, false, false),
            prefix(0, &[], "#work !! x")
        );
        assert_eq!(
            split_prefixes("!! #work x", true, false, false),
            prefix(2, &[], "#work x")
        );
        assert_eq!(
            split_prefixes("#work !! x", false, true, false),
            prefix(0, &["work"], "!! x")
        );
    }
}
