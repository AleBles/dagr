//! The socket wire format: one JSON object per line, both directions.
//!
//! Newline-delimited JSON is chosen because Quickshell's `SplitParser` reads
//! exactly this and nothing else, which is what lets the DankMaterialShell
//! plugin talk to us with no helper process in between.
//!
//! Method names deliberately match the MCP tool names, so there is one
//! vocabulary across the whole project.

use serde::{Deserialize, Serialize};

use crate::api::{
    AddPriorityParams, AddTaskParams, ApiError, IdParams, ListTasksParams, ReorderPrioritiesParams,
    UpdatePriorityParams, UpdateSettingsParams, UpdateTaskParams,
};

/// Bumped only for a change old clients could not survive. A client seeing a
/// number it does not know should warn and degrade, not guess.
pub const PROTOCOL: u32 = 1;

// --- requests --------------------------------------------------------------

#[derive(Debug)]
pub struct Request {
    pub id: u64,
    pub method: Method,
}

/// Hand-written so the socket is forgiving about `params`.
///
/// Serde's adjacently-tagged enums insist on the exact shape: a method taking
/// only optional fields still needs `"params":{}`, and a method taking none
/// must not carry it at all. That is a trap for anyone poking at the socket by
/// hand or from QML, so we try the shape as sent and then the two obvious
/// near-misses.
impl<'de> Deserialize<'de> for Request {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;

        #[derive(Deserialize)]
        struct Raw {
            id: u64,
            method: String,
            #[serde(default)]
            params: Option<serde_json::Value>,
        }

        let raw = Raw::deserialize(d)?;
        let name = serde_json::Value::String(raw.method);

        let mut shapes = Vec::with_capacity(3);
        if let Some(params) = raw.params {
            shapes.push(serde_json::json!({ "method": name, "params": params }));
        }
        shapes.push(serde_json::json!({ "method": name }));
        shapes.push(serde_json::json!({ "method": name, "params": {} }));

        let mut first_error = None;
        for shape in shapes {
            match serde_json::from_value::<Method>(shape) {
                Ok(method) => return Ok(Request { id: raw.id, method }),
                Err(e) => first_error.get_or_insert(e),
            };
        }
        Err(D::Error::custom(first_error.expect("at least one attempt")))
    }
}

/// Every method the socket understands. An enum rather than a string so the
/// compiler makes sure `serve.rs` handles all of them.
#[derive(Debug, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Method {
    ListTasks(ListTasksParams),
    AddTask(AddTaskParams),
    UpdateTask(UpdateTaskParams),
    DeleteTask(IdParams),
    ClearCompleted,
    GetSettings,
    UpdateSettings(UpdateSettingsParams),
    ListPriorities,
    AddPriority(AddPriorityParams),
    UpdatePriority(UpdatePriorityParams),
    ReorderPriorities(ReorderPrioritiesParams),
    DeletePriority(IdParams),
    /// Ask for `changed` events on this connection from now on.
    Subscribe,
    /// Cheap liveness check.
    Ping,
}

// --- replies ---------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    /// `invalid_params` or `internal`.
    pub kind: String,
    pub message: String,
}

impl Response {
    pub fn ok(id: u64, result: serde_json::Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: u64, kind: &str, message: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(ErrorBody {
                kind: kind.into(),
                message: message.into(),
            }),
        }
    }

    pub fn from_api(id: u64, err: ApiError) -> Self {
        Self::err(id, err.kind(), err.message())
    }
}

// --- events ----------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Sent unprompted as soon as a client connects, so it can check the
    /// protocol version and see *which* database it is talking to.
    Hello {
        protocol: u32,
        version: String,
        db: String,
        pid: u32,
    },
    /// Something in the database changed. Carries `data_version` so a client
    /// can ignore a repeat it has already handled.
    Changed { version: i64 },
    /// We lost track of what this subscriber has seen. Throw the cache away
    /// and list again.
    Resync,
}

/// Serialises to a single line, newline included. Serde never emits a raw
/// newline inside a JSON string, so one object per line always holds.
pub fn line<T: Serialize>(value: &T) -> String {
    let mut s = serde_json::to_string(value).unwrap_or_else(|e| {
        format!(r#"{{"event":"error","message":"could not encode reply: {e}"}}"#)
    });
    s.push('\n');
    s
}
