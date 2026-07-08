use serde::{Deserialize, Serialize};

/// Which side of a session a peer plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Host,
    Viewer,
}

/// Video codecs a peer is able to decode, advertised during the handshake so the
/// host can pick one both sides understand. JPEG tiles are the mandatory baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoCodec {
    /// Uncompressed RGBA. Only used on loopback/LAN to validate the pipeline.
    RawRgba,
    /// Dirty-tile deltas compressed as JPEG. Baseline for the MVP.
    JpegTiles,
    /// H.264 video (OpenH264). Each frame's payload is an Annex-B bitstream
    /// chunk; the decoder derives full/delta from the stream itself, so
    /// `encoding` is always `Full` and `tiles` is empty for this codec.
    H264,
}

/// Picks the codec a session will use: the host's preferred codec if the viewer
/// can decode it, otherwise the first codec (in host preference order) both
/// sides support. JPEG tiles are the mandatory baseline every peer implements,
/// so that is the final fallback.
pub fn negotiate_codec(
    preferred: VideoCodec,
    host_supported: &[VideoCodec],
    viewer_supported: &[VideoCodec],
) -> VideoCodec {
    if host_supported.contains(&preferred) && viewer_supported.contains(&preferred) {
        return preferred;
    }
    host_supported
        .iter()
        .copied()
        .find(|c| viewer_supported.contains(c))
        .unwrap_or(VideoCodec::JpegTiles)
}

/// First message on the control stream. Establishes version and capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_version: u16,
    pub app_version: String,
    pub role: Role,
    pub supported_codecs: Vec<VideoCodec>,
}

/// Result of an authentication attempt, sent host -> viewer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthOutcome {
    Accepted { session_id: u64 },
    Rejected { reason: String },
    PendingApproval,
}

/// A monitor available on the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub id: u32,
    pub name: String,
    /// Top-left position of this monitor in the host's virtual desktop, in physical
    /// pixels. Needed so normalized pointer coordinates land on the right monitor.
    pub origin_x: i32,
    pub origin_y: i32,
    pub width_px: u32,
    pub height_px: u32,
    pub is_primary: bool,
}

/// Mouse buttons carried by pointer events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Press/release shared by pointer buttons and keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointerAction {
    Press,
    Release,
}

/// A logical key. Named keys cover control/navigation; `Unicode` covers printable
/// characters so that layout differences between viewer and host are handled by the
/// host mapping the character back to its local keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyCode {
    Unicode(char),
    Enter,
    Escape,
    Backspace,
    Tab,
    Space,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Shift,
    Control,
    Alt,
    Meta,
    CapsLock,
    Function(u8),
}

/// Messages on the reliable, ordered control stream (bidirectional).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ControlMessage {
    Hello(Hello),
    AuthRequest { session_code: String },
    AuthResponse(AuthOutcome),
    MonitorList { monitors: Vec<MonitorInfo> },
    SelectMonitor { monitor_id: u32 },
    /// Pointer position normalized to `0.0..=1.0` within the active monitor. Keeping
    /// coordinates resolution-independent makes DPI and mismatched resolutions a
    /// non-issue: the host scales to its own physical pixels.
    MouseMove { x_norm: f32, y_norm: f32 },
    MouseButton { button: MouseButton, action: PointerAction },
    MouseWheel { delta_x: f32, delta_y: f32 },
    KeyEvent { key: KeyCode, action: PointerAction },
    /// A run of typed printable characters. Typing travels as text (the host
    /// injects the exact characters, independent of its keyboard layout and
    /// shift state); `KeyEvent` stays for named keys and modifier shortcuts,
    /// where the physical key matters.
    Text { text: String },
    /// Host tells the viewer the active monitor changed resolution.
    Resize { width_px: u32, height_px: u32 },
    Ping { nonce: u64 },
    Pong { nonce: u64 },
    Disconnect { reason: String },
    Error { code: u16, message: String },
}

/// How a [`MediaMessage::ScreenFrame`] payload is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreenEncoding {
    /// Payload covers the whole frame; `tiles` is empty.
    Full,
    /// Payload is the concatenation of only the changed tiles listed in `tiles`.
    TileDelta,
}

/// Rectangle of a changed tile, in physical pixels of the source monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Messages on unidirectional media streams (host -> viewer).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MediaMessage {
    ScreenFrame {
        monitor_id: u32,
        frame_id: u64,
        codec: VideoCodec,
        encoding: ScreenEncoding,
        width_px: u32,
        height_px: u32,
        tiles: Vec<TileRect>,
        #[serde(with = "serde_bytes")]
        payload: Vec<u8>,
    },
}
