//! Frontend mirrors of the backend command payloads (serde camelCase, matching
//! the `Serialize` shapes in `src-tauri/src/commands/`).

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub port: u16,
    pub fingerprint: String,
    pub backend_label: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "event", content = "data", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum HostUiEvent {
    CodeReady { code: String },
    ApprovalRequested { peer_addr: String, fingerprint: Option<String> },
    SessionStarted { peer_addr: String },
    SessionEnded { reason: String },
    Log { line: String },
    Error { message: String },
    Stopped,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MonitorUi {
    pub id: u32,
    pub name: String,
    pub width_px: u32,
    pub height_px: u32,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerInfo {
    pub monitors: Vec<MonitorUi>,
    pub fingerprint: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "event", content = "data", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ViewerUiEvent {
    Resized { width_px: u32, height_px: u32 },
    MonitorsUpdated { monitors: Vec<MonitorUi> },
    Disconnected { reason: String },
    Error { message: String },
}
