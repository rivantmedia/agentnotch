//! Control messages from the app's own CLI (the doctor, the uninstaller,
//! the smoke test) to the running app. Counts and states only: never paths,
//! prompts or other text from a session.

use crate::PROTOCOL;
use serde::{Deserialize, Serialize};

/// The `event` of a control request.
pub const CONTROL_EVENT: &str = "AgentNotchControl";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlOp {
    Status,
    Quit,
}

/// `{"protocol":1,"event":"AgentNotchControl","op":"status"|"quit"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlRequest {
    pub protocol: u32,
    pub event: String,
    pub op: ControlOp,
}

impl ControlRequest {
    pub fn new(op: ControlOp) -> Self {
        ControlRequest { protocol: PROTOCOL, event: CONTROL_EVENT.into(), op }
    }

    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
}

/// What `control status` reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ControlStatus {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub sealed: bool,
    /// The app's own token is elevated.
    #[serde(default)]
    pub elevated: bool,
    #[serde(default)]
    pub accounts: u32,
    #[serde(default)]
    pub rings: u32,
    /// Rings with a usage reading.
    #[serde(default)]
    pub readings: u32,
    #[serde(default)]
    pub sessions: u32,
    /// PermissionRequests held open.
    #[serde(default)]
    pub held: u32,
    /// `granted` | `declined` | `unasked`.
    #[serde(default)]
    pub hook_consent: String,
    /// `listening`, or why not (`in_use`, `off`, …).
    #[serde(default)]
    pub transport: String,
    /// `signed_out` | `signing_in` | `signed_in`.
    #[serde(default)]
    pub cloud: String,
    #[serde(default)]
    pub sync: bool,
}

/// The response frame: `{"ok":true,"status":{…}}` for status, `{"ok":true}`
/// for quit (the app then exits gracefully), `{"ok":false,"error":"…"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ControlStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ControlResponse {
    pub fn status(status: ControlStatus) -> Self {
        ControlResponse { ok: true, status: Some(status), error: None }
    }

    pub fn ok() -> Self {
        ControlResponse { ok: true, status: None, error: None }
    }

    pub fn error(message: impl Into<String>) -> Self {
        ControlResponse { ok: false, status: None, error: Some(message.into()) }
    }

    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_shape() {
        assert_eq!(
            String::from_utf8(ControlRequest::new(ControlOp::Quit).to_json()).unwrap(),
            r#"{"protocol":1,"event":"AgentNotchControl","op":"quit"}"#
        );
    }

    #[test]
    fn responses_ignore_unknown_fields() {
        let parsed: ControlResponse =
            serde_json::from_str(r#"{"ok":true,"status":{"version":"1.1.0","sessions":3,"future":1},"later":[]}"#)
                .unwrap();
        assert_eq!(parsed.status.unwrap().sessions, 3);
        assert_eq!(String::from_utf8(ControlResponse::ok().to_json()).unwrap(), r#"{"ok":true}"#);
    }
}
