//! Dagr — a small, priority-ordered task list with a built-in MCP
//! server. The library holds everything; `main.rs` only picks a mode.

rust_i18n::i18n!("locales", fallback = "en");

/// The translation for a key, as an owned `String`: GTK builders take text by
/// value or as `&str`, not the `Cow` that `t!` hands back.
#[macro_export]
macro_rules! tr {
    ($($args:tt)*) => {
        rust_i18n::t!($($args)*).to_string()
    };
}

pub mod api;
pub mod db;
pub mod i18n;
pub mod mcp;
pub mod mcp_http;
pub mod paths;
pub mod proto;
pub mod serve;
pub mod settings;
pub mod task;
pub mod ui;
