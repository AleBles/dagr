//! Dagr — a small, priority-ordered task list with a built-in MCP
//! server. The library holds everything; `main.rs` only picks a mode.

pub mod db;
pub mod mcp;
pub mod mcp_http;
pub mod settings;
pub mod task;
pub mod ui;
