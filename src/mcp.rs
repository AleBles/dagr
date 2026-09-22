//! The MCP server: twenty-one tools over the rmcp SDK.
//!
//! This is only an adapter. Every rule about tasks lives in `api.rs`, which
//! the socket in `serve.rs` calls too, so both front doors behave the same.
//! Each tool here resolves to: take the lock, call `Api`, render the result
//! as JSON.
//!
//! There is one transport, Streamable HTTP, hosted by the background service
//! (`mcp_http.rs`). The old stdio server was a second way in with the same
//! tools and its own failure modes, so it is gone.

use std::sync::{Arc, Mutex, MutexGuard};

use rmcp::{
    handler::server::wrapper::Parameters, model::*, tool, tool_handler, tool_router,
    ErrorData as McpError, ServerHandler,
};
use serde::Serialize;

use crate::api::{
    AddLabelParams, AddListParams, AddPriorityParams, AddTaskParams, Api, ApiError,
    DeleteListParams, IdParams, ListTasksParams, ReorderListsParams, ReorderPrioritiesParams,
    UpdateLabelParams, UpdateListParams, UpdatePriorityParams, UpdateSettingsParams,
    UpdateTaskParams,
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

    #[tool(
        description = "List the labels, alphabetically. Labels are optional tags; a task carries any number of them."
    )]
    fn list_labels(&self) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).list_labels().map_err(mcp_error)?)
    }

    #[tool(description = "Add a label. Returns the label that was created.")]
    fn add_label(
        &self,
        Parameters(p): Parameters<AddLabelParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).add_label(p).map_err(mcp_error)?)
    }

    #[tool(description = "Rename and/or recolor a label. Only the given fields change.")]
    fn update_label(
        &self,
        Parameters(p): Parameters<UpdateLabelParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).update_label(p).map_err(mcp_error)?)
    }

    #[tool(
        description = "Delete a label. It comes off every task that carried it; the tasks themselves stay."
    )]
    fn delete_label(
        &self,
        Parameters(p): Parameters<IdParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).delete_label(p).map_err(mcp_error)?)
    }

    #[tool(
        description = "List the task lists, in sidebar order. Every task belongs to exactly one."
    )]
    fn list_lists(&self) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).list_lists().map_err(mcp_error)?)
    }

    #[tool(description = "Add a list at the bottom of the sidebar. Returns the list created.")]
    fn add_list(
        &self,
        Parameters(p): Parameters<AddListParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).add_list(p).map_err(mcp_error)?)
    }

    #[tool(description = "Rename a list.")]
    fn update_list(
        &self,
        Parameters(p): Parameters<UpdateListParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).update_list(p).map_err(mcp_error)?)
    }

    #[tool(description = "Set the sidebar order. Pass every list id exactly once, top first.")]
    fn reorder_lists(
        &self,
        Parameters(p): Parameters<ReorderListsParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).reorder_lists(p).map_err(mcp_error)?)
    }

    #[tool(
        description = "Delete a list. Its tasks move to another list rather than being deleted, and the last list cannot go."
    )]
    fn delete_list(
        &self,
        Parameters(p): Parameters<DeleteListParams>,
    ) -> Result<CallToolResult, McpError> {
        let db = self.db()?;
        json(&Api::new(&db).delete_list(p).map_err(mcp_error)?)
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
                "Dagr: a personal to-do list. Tasks have a title, a done flag, one \
                 priority, any number of labels and one list. Priorities are user-defined, ordered \
                 highest first, and that order sorts the list. Labels are user-defined \
                 name/color tags, sorted alphabetically; set them with update_task \
                 {labels: [...]}, which replaces the whole set, or write a leading #tag in \
                 add_task, which creates the label if it is new. Both features can be \
                 switched off in settings, and labels are off by default. Lists hold \
                 tasks: every task is in exactly one, new tasks go to the list showing in \
                 the window, a leading @list in add_task files it elsewhere, and \
                 update_task {list: \"...\"} moves it. Lists are off by default too. Use \
                 list_tasks / list_priorities / list_labels / list_lists / get_settings \
                 first to see ids and state. Changes appear in the running app \
                 immediately."
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
            "list_labels",
            "add_label",
            "update_label",
            "delete_label",
            "list_lists",
            "add_list",
            "update_list",
            "reorder_lists",
            "delete_list",
        ] {
            assert!(
                names.contains(&expected.to_string()),
                "missing tool {expected}"
            );
        }
        assert_eq!(names.len(), 21, "unexpected tool count: {names:?}");
    }
}
