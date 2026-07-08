use jpeg_encoder::{ColorType, Encoder};
use protocol::{MediaMessage, ScreenEncoding, TileRect, VideoCodec};

use crate::{CodecError, RgbaFrame};

/// Side length of a square tile, in pixels. Edge tiles are clamped to the frame.
pub const TILE_SIZE: u32 = 64;
const JPEG_QUALITY: u8 = 80;

/// Encodes screen frames into [`MediaMessage::ScreenFrame`] values, sending only
/// changed tiles after the first frame.
///
/// A [`TileEncoder`] is stateful: it remembers the previous frame to diff against.
/// Constructing one, or a resolution change, forces the next frame to be `Full`.
#[derive(Debug)]
pub struct TileEncoder {
    codec: VideoCodec,
    previous: Option<RgbaFrame>,
}

impl TileEncoder {
    pub fn new(codec: VideoCodec) -> Self {
        Self { codec, previous: None }
    }

    /// Forgets the previous frame so the next encode sends a full frame.
    pub fn force_keyframe(&mut self) {
        self.previous = None;
    }

    /// Encodes `frame`. The first frame after construction or a resize is sent in
    /// full; subsequent frames send only tiles whose pixels changed.
    pub fn encode(
        &mut self,
        monitor_id: u32,
        frame_id: u64,
        frame: &RgbaFrame,
    ) -> Result<MediaMessage, CodecError> {
        let send_full = match &self.previous {
            Some(prev) => !prev.same_size(frame),
            None => true,
        };

        let message = if send_full || self.codec == VideoCodec::RawRgba {
            self.encode_full(monitor_id, frame_id, frame)?
        } else {
            self.encode_delta(monitor_id, frame_id, frame)?
        };

        self.previous = Some(frame.clone());
        Ok(message)
    }

    fn encode_full(
        &self,
        monitor_id: u32,
        frame_id: u64,
        frame: &RgbaFrame,
    ) -> Result<MediaMessage, CodecError> {
        let payload = match self.codec {
            VideoCodec::RawRgba => frame.data.clone(),
            VideoCodec::JpegTiles => {
                encode_jpeg(&frame.data, frame.width, frame.height)?
            }
            // Video codecs live behind [`crate::VideoEncoder`], never here.
            VideoCodec::H264 => return Err(CodecError::UnsupportedCodec(self.codec)),
        };
        Ok(MediaMessage::ScreenFrame {
            monitor_id,
            frame_id,
            codec: self.codec,
            encoding: ScreenEncoding::Full,
            width_px: frame.width,
            height_px: frame.height,
            tiles: Vec::new(),
            payload,
        })
    }

    fn encode_delta(
        &self,
        monitor_id: u32,
        frame_id: u64,
        frame: &RgbaFrame,
    ) -> Result<MediaMessage, CodecError> {
        let previous = self.previous.as_ref().expect("delta requires a prior frame");
        let mut tiles = Vec::new();
        let mut payload = Vec::new();

        for rect in tile_rects(frame.width, frame.height) {
            if tile_changed(previous, frame, &rect) {
                let pixels = extract_tile(frame, &rect);
                let jpeg = encode_jpeg(&pixels, rect.w, rect.h)?;
                payload.extend_from_slice(&(jpeg.len() as u32).to_le_bytes());
                payload.extend_from_slice(&jpeg);
                tiles.push(rect);
            }
        }

        Ok(MediaMessage::ScreenFrame {
            monitor_id,
            frame_id,
            codec: self.codec,
            encoding: ScreenEncoding::TileDelta,
            width_px: frame.width,
            height_px: frame.height,
            tiles,
            payload,
        })
    }
}

/// Iterates the tile grid covering a frame, clamping tiles at the right/bottom edges.
pub(crate) fn tile_rects(width: u32, height: u32) -> Vec<TileRect> {
    let mut rects = Vec::new();
    let mut y = 0;
    while y < height {
        let h = TILE_SIZE.min(height - y);
        let mut x = 0;
        while x < width {
            let w = TILE_SIZE.min(width - x);
            rects.push(TileRect { x, y, w, h });
            x += TILE_SIZE;
        }
        y += TILE_SIZE;
    }
    rects
}

fn tile_changed(a: &RgbaFrame, b: &RgbaFrame, rect: &TileRect) -> bool {
    let stride = a.width as usize * 4;
    let row_bytes = rect.w as usize * 4;
    for row in 0..rect.h {
        let start = (rect.y as usize + row as usize) * stride + rect.x as usize * 4;
        if a.data[start..start + row_bytes] != b.data[start..start + row_bytes] {
            return true;
        }
    }
    false
}

fn extract_tile(frame: &RgbaFrame, rect: &TileRect) -> Vec<u8> {
    let stride = frame.width as usize * 4;
    let row_bytes = rect.w as usize * 4;
    let mut out = Vec::with_capacity(row_bytes * rect.h as usize);
    for row in 0..rect.h {
        let start = (rect.y as usize + row as usize) * stride + rect.x as usize * 4;
        out.extend_from_slice(&frame.data[start..start + row_bytes]);
    }
    out
}

fn encode_jpeg(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, CodecError> {
    let mut out = Vec::new();
    let encoder = Encoder::new(&mut out, JPEG_QUALITY);
    encoder
        .encode(rgba, width as u16, height as u16, ColorType::Rgba)
        .map_err(|e| CodecError::JpegEncode(e.to_string()))?;
    Ok(out)
}
