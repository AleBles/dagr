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

    pub fn label(self) -> String {
        match self {
            SortOrder::PriorityOldest => crate::tr!("prefs.sort.priority_oldest"),
            SortOrder::PriorityNewest => crate::tr!("prefs.sort.priority_newest"),
            SortOrder::Oldest => crate::tr!("prefs.sort.oldest"),
            SortOrder::Newest => crate::tr!("prefs.sort.newest"),
            SortOrder::Title => crate::tr!("prefs.sort.alphabetical"),
        }
    }

    pub fn uses_priority(self) -> bool {
        matches!(self, SortOrder::PriorityOldest | SortOrder::PriorityNewest)
    }
}

/// The language the app speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    /// Whatever the system asks for, falling back to English.
    System,
    English,
    Dutch,
}

impl Language {
    /// All of them, in the sequence shown in Preferences.
    pub const ALL: [Language; 3] = [Language::System, Language::English, Language::Dutch];

    pub fn as_str(self) -> &'static str {
        match self {
            Language::System => "system",
            Language::English => "en",
            Language::Dutch => "nl",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.as_str() == value.trim())
    }

    /// The locale to use, or `None` when the system decides.
    pub fn locale(self) -> Option<&'static str> {
        match self {
            Language::System => None,
            other => Some(other.as_str()),
        }
    }

    /// Shown in its own language, the way language pickers everywhere do it.
    pub fn label(self) -> String {
        match self {
            Language::System => crate::tr!("prefs.settings.language_system"),
            Language::English => "English".to_string(),
            Language::Dutch => "Nederlands".to_string(),
        }
    }
}

/// Which list the window shows when it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpenOn {
    /// Whatever was showing when the window was last closed.
    LastUsed,
    /// The list new tasks go to.
    DefaultList,
    /// Everything, across all lists.
    AllTasks,
}

impl OpenOn {
    /// All of them, in the sequence shown in Preferences.
    pub const ALL: [OpenOn; 3] = [OpenOn::LastUsed, OpenOn::DefaultList, OpenOn::AllTasks];

    pub fn as_str(self) -> &'static str {
        match self {
            OpenOn::LastUsed => "last_used",
            OpenOn::DefaultList => "default_list",
            OpenOn::AllTasks => "all_tasks",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.as_str() == value.trim())
    }

    pub fn label(self) -> String {
        match self {
            OpenOn::LastUsed => crate::tr!("prefs.open_on.last_used"),
            OpenOn::DefaultList => crate::tr!("prefs.open_on.default_list"),
            OpenOn::AllTasks => crate::tr!("prefs.open_on.all_tasks"),
        }
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
    /// Group the list by the day a task was added, ahead of priority.
    pub date_first: bool,
    /// Show label dots and the picker, and read `#tag` when adding a task.
    pub labels_enabled: bool,
    /// Show the list sidebar, and read `@list` when adding a task.
    pub lists_enabled: bool,
    /// List new tasks go to. `None` means the first one.
    pub default_list_id: Option<i64>,
    /// List currently showing. `None` means all of them together.
    pub current_list_id: Option<i64>,
    /// Which list to show when the window opens.
    pub open_on: OpenOn,
    /// The language to speak, or the system's choice.
    pub language: Language,
    /// Size the window opens at, in logical pixels.
    pub window_width: i32,
    pub window_height: i32,
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
            date_first: false,
            labels_enabled: false,
            lists_enabled: false,
            default_list_id: None,
            current_list_id: None,
            open_on: OpenOn::LastUsed,
            language: Language::System,
            window_width: 420,
            window_height: 600,
            mcp_http_enabled: true,
            mcp_http_port: 7331,
        }
    }
}

impl Settings {
    pub const PRIORITIES_ENABLED: &'static str = "priorities_enabled";
    pub const DEFAULT_PRIORITY: &'static str = "default_priority";
    pub const SORT_ORDER: &'static str = "sort_order";
    pub const DATE_FIRST: &'static str = "date_first";
    pub const LABELS_ENABLED: &'static str = "labels_enabled";
    pub const LISTS_ENABLED: &'static str = "lists_enabled";
    pub const DEFAULT_LIST: &'static str = "default_list";
    pub const CURRENT_LIST: &'static str = "current_list";
    pub const OPEN_ON: &'static str = "open_on";
    pub const LANGUAGE: &'static str = "language";
    pub const WINDOW_WIDTH: &'static str = "window_width";
    pub const WINDOW_HEIGHT: &'static str = "window_height";
    pub const MCP_HTTP_ENABLED: &'static str = "mcp_http_enabled";
    pub const MCP_HTTP_PORT: &'static str = "mcp_http_port";

    /// Lowest port we allow, to stay out of the privileged range.
    pub const MIN_PORT: u16 = 1024;

    /// The window cannot usefully be smaller than this, so neither can the
    /// setting: these are also the window's own size requests.
    pub const MIN_WINDOW_WIDTH: i32 = 360;
    pub const MIN_WINDOW_HEIGHT: i32 = 300;
    /// Roomy enough for any display, small enough to catch a typo.
    pub const MAX_WINDOW_SIZE: i32 = 8192;

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
                Self::DATE_FIRST => {
                    s.date_first = parse_bool(value).unwrap_or(s.date_first);
                }
                Self::LABELS_ENABLED => {
                    s.labels_enabled = parse_bool(value).unwrap_or(s.labels_enabled);
                }
                Self::LISTS_ENABLED => {
                    s.lists_enabled = parse_bool(value).unwrap_or(s.lists_enabled);
                }
                Self::DEFAULT_LIST => {
                    // "first" (or anything unparsable) means automatic.
                    s.default_list_id = value.trim().parse::<i64>().ok();
                }
                Self::CURRENT_LIST => {
                    // "all" (or anything unparsable) means every list at once.
                    s.current_list_id = value.trim().parse::<i64>().ok();
                }
                Self::OPEN_ON => {
                    s.open_on = OpenOn::parse(value).unwrap_or(s.open_on);
                }
                Self::LANGUAGE => {
                    s.language = Language::parse(value).unwrap_or(s.language);
                }
                Self::WINDOW_WIDTH => {
                    s.window_width =
                        parse_size(value, Self::MIN_WINDOW_WIDTH).unwrap_or(s.window_width);
                }
                Self::WINDOW_HEIGHT => {
                    s.window_height =
                        parse_size(value, Self::MIN_WINDOW_HEIGHT).unwrap_or(s.window_height);
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
            (Self::DATE_FIRST, self.date_first.to_string()),
            (Self::LABELS_ENABLED, self.labels_enabled.to_string()),
            (Self::LISTS_ENABLED, self.lists_enabled.to_string()),
            (
                Self::DEFAULT_LIST,
                self.default_list_id
                    .map_or_else(|| "first".to_string(), |id| id.to_string()),
            ),
            (
                Self::CURRENT_LIST,
                self.current_list_id
                    .map_or_else(|| "all".to_string(), |id| id.to_string()),
            ),
            (Self::OPEN_ON, self.open_on.as_str().to_string()),
            (Self::LANGUAGE, self.language.as_str().to_string()),
            (Self::WINDOW_WIDTH, self.window_width.to_string()),
            (Self::WINDOW_HEIGHT, self.window_height.to_string()),
            (Self::MCP_HTTP_ENABLED, self.mcp_http_enabled.to_string()),
            (Self::MCP_HTTP_PORT, self.mcp_http_port.to_string()),
        ]
    }

    /// `http://127.0.0.1:<port>/mcp`
    pub fn mcp_http_url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.mcp_http_port)
    }
}

/// A window dimension, or `None` if it is not a number in range.
fn parse_size(value: &str, min: i32) -> Option<i32> {
    value
        .trim()
        .parse::<i32>()
        .ok()
        .filter(|size| (min..=Settings::MAX_WINDOW_SIZE).contains(size))
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
            date_first: true,
            labels_enabled: true,
            lists_enabled: true,
            default_list_id: Some(2),
            current_list_id: None,
            open_on: OpenOn::AllTasks,
            language: Language::Dutch,
            window_width: 900,
            window_height: 700,
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
        assert!(!Settings::default().date_first);
        assert!(Settings::from_pairs([("date_first", "on")]).date_first);
        // Labels and lists stay off until asked for.
        assert!(!Settings::default().labels_enabled);
        assert!(!Settings::default().lists_enabled);
        assert_eq!(Settings::default().open_on, OpenOn::LastUsed);
        assert_eq!(
            Settings::from_pairs([("current_list", "all")]).current_list_id,
            None
        );
        assert_eq!(
            Settings::from_pairs([("default_list", "7")]).default_list_id,
            Some(7)
        );
        assert_eq!(
            Settings::from_pairs([("open_on", "all_tasks")]).open_on,
            OpenOn::AllTasks
        );
        assert_eq!(
            Settings::from_pairs([("open_on", "sideways")]).open_on,
            OpenOn::LastUsed
        );
        assert_eq!(Settings::default().language, Language::System);
        assert_eq!(
            Settings::from_pairs([("language", "nl")]).language,
            Language::Dutch
        );
        assert!(Settings::from_pairs([("labels_enabled", "yes")]).labels_enabled);
        // Sizes below the window's own minimum, or not numbers at all, are
        // ignored rather than saved.
        assert_eq!(
            Settings::from_pairs([("window_width", "100")]).window_width,
            420
        );
        assert_eq!(
            Settings::from_pairs([("window_height", "wide")]).window_height,
            600
        );
        assert_eq!(
            Settings::from_pairs([("window_width", "1000")]).window_width,
            1000
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
