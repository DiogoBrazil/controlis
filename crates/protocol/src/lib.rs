//! Wire protocol shared by host and viewer.
//!
//! This crate is intentionally free of any OS or networking dependency: it only
//! defines the messages exchanged between peers, how they are versioned, and how
//! they are framed on a byte stream. Both the QUIC transport and any test harness
//! build on top of it.

mod frame;
mod message;

pub use frame::{FrameDecoder, MAX_MESSAGE_LEN};
pub use message::{
    negotiate_codec, AuthOutcome, ControlMessage, Hello, KeyCode, MediaMessage, MonitorInfo,
    MouseButton, PointerAction, Role, ScreenEncoding, TileRect, VideoCodec,
};

use serde::{de::DeserializeOwned, Serialize};

/// Protocol version negotiated in [`Hello`]. Bumped on any incompatible change.
/// v2: added [`VideoCodec::H264`] (changes the wire shape of `Hello` and frames).
pub const PROTOCOL_VERSION: u16 = 2;

/// Errors produced while encoding or decoding protocol messages.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("failed to serialize message: {0}")]
    Serialize(postcard::Error),
    #[error("failed to deserialize message: {0}")]
    Deserialize(postcard::Error),
    #[error("message length {0} exceeds maximum {MAX_MESSAGE_LEN}")]
    MessageTooLarge(usize),
    #[error("incomplete frame: need more bytes")]
    Incomplete,
}

/// Serializes a message into a length-delimited frame ready to be written to a stream.
///
/// Layout: a `u32` little-endian length prefix followed by the postcard-encoded body.
pub fn encode<M: Serialize>(message: &M) -> Result<Vec<u8>, ProtocolError> {
    let body = postcard::to_stdvec(message).map_err(ProtocolError::Serialize)?;
    if body.len() > MAX_MESSAGE_LEN {
        return Err(ProtocolError::MessageTooLarge(body.len()));
    }
    let mut framed = Vec::with_capacity(4 + body.len());
    framed.extend_from_slice(&(body.len() as u32).to_le_bytes());
    framed.extend_from_slice(&body);
    Ok(framed)
}

/// Deserializes a message from a complete frame body (without the length prefix).
pub fn decode<M: DeserializeOwned>(body: &[u8]) -> Result<M, ProtocolError> {
    postcard::from_bytes(body).map_err(ProtocolError::Deserialize)
}
