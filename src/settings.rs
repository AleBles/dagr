//! App settings. Stored as key/value rows in SQLite so the window and the MCP
//! server see the same values. Add a field here, a default, and one line in
//! `from_pairs` / `to_pairs` — that is all a new setting needs.

use serde::{Deserialize, Serialize};

/// How the task list is ordered. Completed tasks always sink to the bottom.
/// The "priority" variants fall back to plain creation order while the
/// priority feature is turned off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    /// By priority, then oldest first.
    PriorityOldest,
    /// By priority, then newest first.
    PriorityNewest,
    /// Oldest first, ignoring priority.
    Oldest,
    /// Newest first, ignoring priority.
    Newest,
    /// Alphabetical by title.
    Title,
}

impl SortOrder {
    /// All orders, in the sequence shown in Preferences.
    pub const ALL: [SortOrder; 5] = [
        SortOrder::PriorityOldest,
        SortOrder::PriorityNewest,
        SortOrder::Oldest,
        SortOrder::Newest,
        SortOrder::Title,
    ];

    /// The stored / MCP-facing name (matches the serde representation).
    pub fn as_str(self) -> &'static str {
        match self {
            SortOrder::PriorityOldest => "priority_oldest",
            SortOrder::PriorityNewest => "priority_newest",
            SortOrder::Oldest => "oldest",
            SortOrder::Newest => "newest",
            SortOrder::Title => "title",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.as_str() == value.trim())
    }

    pub fn label(self) -> &'static str {
        match self {
            SortOrder::PriorityOldest => "Priority, then oldest first",
            SortOrder::PriorityNewest => "Priority, then newest first",
            SortOrder::Oldest => "Oldest first",
            SortOrder::Newest => "Newest first",
            SortOrder::Title => "Alphabetical",
        }
    }

    pub fn uses_priority(self) -> bool {
        matches!(self, SortOrder::PriorityOldest | SortOrder::PriorityNewest)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Settings {
    /// Show priority dots and the picker, and sort the list by priority.
    pub priorities_enabled: bool,
    /// Priority given to new tasks. `None` means "whatever is lowest".
    pub default_priority_id: Option<i64>,
    /// Ordering of the task list.
    pub sort_order: SortOrder,
    /// Serve MCP over HTTP on localhost while the window is open.
    pub mcp_http_enabled: bool,
    /// Port for the HTTP MCP server.
    pub mcp_http_port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            priorities_enabled: true,
            default_priority_id: None,
            sort_order: SortOrder::PriorityOldest,
            mcp_http_enabled: false,
            mcp_http_port: 7331,
        }
    }
}

impl Settings {
    pub const PRIORITIES_ENABLED: &'static str = "priorities_enabled";
    pub const DEFAULT_PRIORITY: &'static str = "default_priority";
    pub const SORT_ORDER: &'static str = "sort_order";
    pub const MCP_HTTP_ENABLED: &'static str = "mcp_http_enabled";
    pub const MCP_HTTP_PORT: &'static str = "mcp_http_port";

    /// Lowest port we allow, to stay out of the privileged range.
    pub const MIN_PORT: u16 = 1024;

    /// Builds settings from stored rows; unknown keys are ignored and missing
    /// keys keep their default.
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut s = Self::default();
        for (key, value) in pairs {
            match key {
                Self::PRIORITIES_ENABLED => {
                    s.priorities_enabled = parse_bool(value).unwrap_or(s.priorities_enabled);
                }
                Self::DEFAULT_PRIORITY => {
                    // "lowest" (or anything unparsable) means automatic.
                    s.default_priority_id = value.trim().parse::<i64>().ok();
                }
                Self::SORT_ORDER => {
                    s.sort_order = SortOrder::parse(value).unwrap_or(s.sort_order);
                }
                Self::MCP_HTTP_ENABLED => {
                    s.mcp_http_enabled = parse_bool(value).unwrap_or(s.mcp_http_enabled);
                }
                Self::MCP_HTTP_PORT => {
                    s.mcp_http_port = value
                        .trim()
                        .parse::<u16>()
                        .ok()
                        .filter(|p| *p >= Self::MIN_PORT)
                        .unwrap_or(s.mcp_http_port);
                }
                _ => {}
            }
        }
        s
    }

    pub fn to_pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            (
                Self::PRIORITIES_ENABLED,
                self.priorities_enabled.to_string(),
            ),
            (
                Self::DEFAULT_PRIORITY,
                self.default_priority_id
                    .map_or_else(|| "lowest".to_string(), |id| id.to_string()),
            ),
            (Self::SORT_ORDER, self.sort_order.as_str().to_string()),
            (Self::MCP_HTTP_ENABLED, self.mcp_http_enabled.to_string()),
            (Self::MCP_HTTP_PORT, self.mcp_http_port.to_string()),
        ]
    }

    /// `http://127.0.0.1:<port>/mcp`
    pub fn mcp_http_url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.mcp_http_port)
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_ignores_junk() {
        let s = Settings {
            priorities_enabled: false,
            default_priority_id: Some(3),
            sort_order: SortOrder::Newest,
            mcp_http_enabled: true,
            mcp_http_port: 4242,
        };
        let pairs = s.to_pairs();
        let back = Settings::from_pairs(pairs.iter().map(|(k, v)| (*k, v.as_str())));
        assert_eq!(back, s);

        let junk = Settings::from_pairs([("priorities_enabled", "maybe"), ("unknown", "1")]);
        assert_eq!(junk, Settings::default());
        assert!(!Settings::from_pairs([("priorities_enabled", "0")]).priorities_enabled);
        assert_eq!(
            Settings::from_pairs([("default_priority", "lowest")]).default_priority_id,
            None
        );
        assert_eq!(
            Settings::from_pairs([("sort_order", "title")]).sort_order,
            SortOrder::Title
        );
        assert_eq!(
            Settings::from_pairs([("sort_order", "sideways")]).sort_order,
            SortOrder::PriorityOldest
        );
        for order in SortOrder::ALL {
            assert_eq!(SortOrder::parse(order.as_str()), Some(order));
            assert_eq!(
                serde_json::to_value(order).unwrap(),
                serde_json::Value::String(order.as_str().into())
            );
        }
        // Privileged or unparsable ports fall back to the default.
        assert_eq!(
            Settings::from_pairs([("mcp_http_port", "80")]).mcp_http_port,
            7331
        );
        assert_eq!(
            Settings::from_pairs([("mcp_http_port", "x")]).mcp_http_port,
            7331
        );
    }
}
