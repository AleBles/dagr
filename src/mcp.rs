//! The MCP server: twelve tools over the rmcp SDK.
//!
//! This is only an adapter. Every rule about tasks lives in `api.rs`, which
//! the socket in `serve.rs` calls too, so both front doors behave the same.
//! Each tool here resolves to: take the lock, call `Api`, render the result
//! as JSON.
//!
//! There is one transport, Streamable HTTP, hosted by the background service
//! (`mcp_http.rs`). The old stdio server was a second way in with the same
//! twelve tools and its own failure modes, so it is gone.

use std::sync::{Arc, Mutex, MutexGuard};

use rmcp::{
    handler::server::wrapper::Parameters, model::*, tool, tool_handler, tool_router,
    ErrorData as McpError, ServerHandler,
};
use serde::Serialize;

use crate::api::{
    AddPriorityParams, AddTaskParams, Api, ApiError, IdParams, ListTasksParams,
    ReorderPrioritiesParams, UpdatePriorityParams, UpdateSettingsParams, UpdateTaskParams,
};
use crate::db::Db;

#[derive(Clone)]
pub struct TasksServer {
    db: Arc<Mutex<Db>>,
}

#[tool_router]
impl TasksServer {
    pub fn new(db: Db) -> Self {
        Self {
            db: Arc::new(Mutex::new(db)),
        }
    }

    #[tool(
        description = "List tasks in display order: open tasks first, then by priority (highest first), then oldest first."
    )]
    fn list_tasks(
        &self,
        Parameters(p): Parameters<ListTasksParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).list_tasks(p).map_err(mcp_error)?)
    }

    #[tool(description = "Add a task. Returns the created task.")]
    fn add_task(
        &self,
        Parameters(p): Parameters<AddTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).add_task(p).map_err(mcp_error)?)
    }

    #[tool(
        description = "Change a task's title, priority and/or done state. Only the given fields change."
    )]
    fn update_task(
        &self,
        Parameters(p): Parameters<UpdateTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).update_task(p).map_err(mcp_error)?)
    }

    #[tool(description = "Delete a task permanently.")]
    fn delete_task(&self, Parameters(p): Parameters<IdParams>) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        let id = p.id;
        Api::new(&db).delete_task(p).map_err(mcp_error)?;
        text(format!("Deleted task {id}"))
    }

    #[tool(description = "Delete every completed task. Returns the removed tasks.")]
    fn clear_completed(&self) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).clear_completed().map_err(mcp_error)?)
    }

    #[tool(description = "Read the app settings.")]
    fn get_settings(&self) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).get_settings().map_err(mcp_error)?)
    }

    #[tool(
        description = "Change app settings. Only the given fields change. Returns the new settings."
    )]
    fn update_settings(
        &self,
        Parameters(p): Parameters<UpdateSettingsParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).update_settings(p).map_err(mcp_error)?)
    }

    #[tool(
        description = "List the priorities, highest first. The order here is the order of the task list."
    )]
    fn list_priorities(&self) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).list_priorities().map_err(mcp_error)?)
    }

    #[tool(
        description = "Add a priority at the bottom (lowest). Use reorder_priorities to move it."
    )]
    fn add_priority(
        &self,
        Parameters(p): Parameters<AddPriorityParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).add_priority(p).map_err(mcp_error)?)
    }

    #[tool(description = "Rename and/or recolor a priority.")]
    fn update_priority(
        &self,
        Parameters(p): Parameters<UpdatePriorityParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).update_priority(p).map_err(mcp_error)?)
    }

    #[tool(
        description = "Set the priority order. Pass every priority id exactly once, highest first."
    )]
    fn reorder_priorities(
        &self,
        Parameters(p): Parameters<ReorderPrioritiesParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).reorder_priorities(p).map_err(mcp_error)?)
    }

    #[tool(
        description = "Delete a priority. Its tasks move to the next lower priority (or next higher if it was the lowest). The last priority cannot be deleted."
    )]
    fn delete_priority(
        &self,
        Parameters(p): Parameters<IdParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).delete_priority(p).map_err(mcp_error)?)
    }

    fn db(&self) -> Result<MutexGuard<'_, Db>, McpError> {
        self.db
            .lock()
            .map_err(|_| McpError::internal_error("database lock poisoned", None))
    }
}

#[tool_handler]
impl ServerHandler for TasksServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("dagr", env!("CARGO_PKG_VERSION"))
                    .with_title("Dagr")
                    .with_description("Priority-ordered personal task list"),
            )
            .with_instructions(
                "Dagr: a personal to-do list. Tasks have a title, a done flag and one \
                 priority. Priorities are user-defined, ordered highest first, and that \
                 order sorts the list; the feature can be switched off in settings. Use \
                 list_tasks / list_priorities / get_settings first to see ids and state. \
                 Changes appear in the running app immediately."
                    .to_string(),
            )
    }
}

// --- helpers -------------------------------------------------------------------

fn mcp_error(err: ApiError) -> McpError {
    match err {
        ApiError::Invalid(m) => McpError::invalid_params(m, None),
        ApiError::Internal(m) => McpError::internal_error(m, None),
    }
}

fn json<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let body = serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(body)]))
}

fn text(message: String) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::text(message)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The behaviour of each tool is tested in `api.rs`; what matters here is
    /// that every action is still actually exposed as a tool.
    #[test]
    fn exposes_every_action_as_a_tool() {
        let names: Vec<String> = TasksServer::tool_router()
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        for expected in [
            "list_tasks",
            "add_task",
            "update_task",
            "delete_task",
            "clear_completed",
            "get_settings",
            "update_settings",
            "list_priorities",
            "add_priority",
            "update_priority",
            "reorder_priorities",
            "delete_priority",
        ] {
            assert!(
                names.contains(&expected.to_string()),
                "missing tool {expected}"
            );
        }
        assert_eq!(names.len(), 12, "unexpected tool count: {names:?}");
    }
}
