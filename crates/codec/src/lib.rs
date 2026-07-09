//! Screen frame compression.
//!
//! The baseline splits a frame into fixed tiles, sends only the tiles that
//! changed since the previous frame, and compresses each with JPEG. A raw-RGBA
//! path exists purely to validate the pipeline on loopback. With the `h264`
//! feature, a real video codec (OpenH264) is available; sessions negotiate a
//! codec in the handshake and [`VideoEncoder`]/[`VideoDecoder`] dispatch to the
//! right implementation.

mod decoder;
mod encoder;
mod frame;
#[cfg(feature = "h264")]
mod h264;

pub use decoder::TileDecoder;
pub use encoder::{TileEncoder, TILE_SIZE};
pub use frame::RgbaFrame;
#[cfg(feature = "h264")]
pub use h264::{H264Decoder, H264Encoder};

use protocol::{MediaMessage, VideoCodec};

/// Errors from encoding or decoding screen frames.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("frame dimensions {width}x{height} do not match buffer length {len}")]
    DimensionMismatch { width: u32, height: u32, len: usize },
    #[error("frame dimensions changed mid-stream without a full frame")]
    UnexpectedResize,
    #[error("jpeg encode failed: {0}")]
    JpegEncode(String),
    #[error("jpeg decode failed: {0}")]
    JpegDecode(String),
    #[error("h264 codec failed: {0}")]
    H264(String),
    #[error("codec {0:?} is not supported by this build")]
    UnsupportedCodec(VideoCodec),
    #[error("malformed tile payload")]
    MalformedPayload,
    #[error(transparent)]
    Protocol(#[from] protocol::ProtocolError),
}

/// Codecs this build can encode and decode, best first. Advertised in the
/// handshake and used as the host side of codec negotiation.
pub const fn supported_codecs() -> &'static [VideoCodec] {
    #[cfg(feature = "h264")]
    {
        &[VideoCodec::H264, VideoCodec::JpegTiles, VideoCodec::RawRgba]
    }
    #[cfg(not(feature = "h264"))]
    {
        &[VideoCodec::JpegTiles, VideoCodec::RawRgba]
    }
}

/// The codec a host should prefer when the viewer supports it.
pub const fn preferred_codec() -> VideoCodec {
    supported_codecs()[0]
}

/// Compresses a decoded frame to a standalone JPEG image.
///
/// Used by UI layers that display frames outside the session pipeline (e.g.
/// pushing quadros to a webview canvas, which decodes JPEG natively).
pub fn encode_rgba_to_jpeg(frame: &RgbaFrame) -> Result<Vec<u8>, CodecError> {
    encoder::encode_jpeg(&frame.data, frame.width, frame.height)
}

/// Encoder for a session's negotiated codec.
#[derive(Debug)]
pub enum VideoEncoder {
    Tiles(TileEncoder),
    #[cfg(feature = "h264")]
    H264(Box<H264Encoder>),
}

impl VideoEncoder {
    /// Creates the encoder for `codec`. `max_fps` bounds the rate control of
    /// video codecs; tile codecs ignore it.
    pub fn new(codec: VideoCodec, max_fps: u32) -> Result<Self, CodecError> {
        let _ = max_fps;
        match codec {
            #[cfg(feature = "h264")]
            VideoCodec::H264 => Ok(Self::H264(Box::new(H264Encoder::new(max_fps)?))),
            #[cfg(not(feature = "h264"))]
            VideoCodec::H264 => Err(CodecError::UnsupportedCodec(codec)),
            other => Ok(Self::Tiles(TileEncoder::new(other))),
        }
    }

    /// Encodes one frame. `None` means the encoder chose to send nothing for
    /// this frame (e.g. the H.264 rate controller skipped it).
    pub fn encode(
        &mut self,
        monitor_id: u32,
        frame_id: u64,
        frame: &RgbaFrame,
    ) -> Result<Option<MediaMessage>, CodecError> {
        match self {
            Self::Tiles(enc) => enc.encode(monitor_id, frame_id, frame).map(Some),
            #[cfg(feature = "h264")]
            Self::H264(enc) => enc.encode(monitor_id, frame_id, frame),
        }
    }

    /// Forces the next frame to stand alone: a full frame for tile codecs, an
    /// IDR keyframe for video codecs. Used when the captured monitor changes,
    /// where a delta against the previous content would be wrong.
    pub fn force_keyframe(&mut self) {
        match self {
            Self::Tiles(enc) => enc.force_keyframe(),
            #[cfg(feature = "h264")]
            Self::H264(enc) => enc.force_keyframe(),
        }
    }
}

/// Decoder that dispatches on the codec tagged in each incoming frame message,
/// so the viewer needs no up-front knowledge of what the host negotiated.
#[derive(Debug, Default)]
pub struct VideoDecoder {
    tiles: Option<TileDecoder>,
    #[cfg(feature = "h264")]
    h264: Option<H264Decoder>,
}

impl VideoDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one frame message. Returns `None` when the message carried no
    /// picture yet (e.g. H.264 parameter sets ahead of the first keyframe).
    pub fn decode(&mut self, message: &MediaMessage) -> Result<Option<&RgbaFrame>, CodecError> {
        let MediaMessage::ScreenFrame { codec, payload, .. } = message;
        match codec {
            #[cfg(feature = "h264")]
            VideoCodec::H264 => {
                if self.h264.is_none() {
                    self.h264 = Some(H264Decoder::new()?);
                }
                self.h264.as_mut().expect("set above").decode(payload)
            }
            #[cfg(not(feature = "h264"))]
            VideoCodec::H264 => {
                let _ = payload;
                Err(CodecError::UnsupportedCodec(*codec))
            }
            _ => self
                .tiles
                .get_or_insert_with(TileDecoder::new)
                .decode(message)
                .map(Some),
        }
    }
}
