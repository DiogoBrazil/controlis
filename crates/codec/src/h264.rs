//! H.264 encode/decode via OpenH264 (feature `h264`).
//!
//! One encoded frame maps to one [`MediaMessage::ScreenFrame`] whose payload is
//! the Annex-B bitstream chunk produced for that frame. The H.264 stream itself
//! carries the key/delta distinction, so messages always use
//! [`ScreenEncoding::Full`] with an empty tile list.

use openh264::decoder::Decoder;
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, RateControlMode, UsageType,
};
use openh264::formats::{RgbaSliceU8, YUVBuffer, YUVSource};
use openh264::OpenH264API;
use protocol::{MediaMessage, ScreenEncoding, VideoCodec};

use crate::{CodecError, RgbaFrame};

/// Target bitrate for the encoded stream. The phase-7 acceptance criterion is
/// staying under ~4 Mbps for typical desktop content at 1080p.
const TARGET_BITRATE_BPS: u32 = 4_000_000;

/// Encodes RGBA frames into an H.264 stream.
///
/// OpenH264 re-initializes itself when the input resolution changes, so monitor
/// switches only need [`H264Encoder::force_keyframe`] (for the same-resolution
/// case, where a delta against the previous monitor's content would be wrong).
pub struct H264Encoder {
    encoder: Encoder,
}

impl std::fmt::Debug for H264Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("H264Encoder").finish_non_exhaustive()
    }
}

impl H264Encoder {
    pub fn new(max_fps: u32) -> Result<Self, CodecError> {
        let config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(TARGET_BITRATE_BPS))
            .max_frame_rate(FrameRate::from_hz(max_fps as f32))
            // OpenH264 only enforces the bitrate cap when it may skip frames;
            // a skipped frame comes out as an empty bitstream and is not sent.
            .skip_frames(true)
            // Not supported for screen content; the encoder would warn and turn
            // them off itself.
            .adaptive_quantization(false)
            .background_detection(false);
        let encoder = Encoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|e| CodecError::H264(e.to_string()))?;
        Ok(Self { encoder })
    }

    /// Forces the next encoded frame to be an IDR keyframe.
    pub fn force_keyframe(&mut self) {
        self.encoder.force_intra_frame();
    }

    /// Encodes one frame. Returns `None` when the rate controller skipped it
    /// (nothing to send; the viewer keeps showing the previous frame).
    pub fn encode(
        &mut self,
        monitor_id: u32,
        frame_id: u64,
        frame: &RgbaFrame,
    ) -> Result<Option<MediaMessage>, CodecError> {
        // YUV 4:2:0 subsampling needs even dimensions; crop a trailing
        // row/column when a monitor reports an odd size.
        let width = frame.width & !1;
        let height = frame.height & !1;
        let cropped;
        let data = if width == frame.width && height == frame.height {
            &frame.data
        } else {
            cropped = crop(frame, width, height);
            &cropped
        };

        let rgba = RgbaSliceU8::new(data, (width as usize, height as usize));
        let yuv = YUVBuffer::from_rgb_source(rgba);
        let bitstream = self
            .encoder
            .encode(&yuv)
            .map_err(|e| CodecError::H264(e.to_string()))?;
        let payload = bitstream.to_vec();
        if payload.is_empty() {
            return Ok(None);
        }

        Ok(Some(MediaMessage::ScreenFrame {
            monitor_id,
            frame_id,
            codec: VideoCodec::H264,
            encoding: ScreenEncoding::Full,
            width_px: width,
            height_px: height,
            tiles: Vec::new(),
            payload,
        }))
    }
}

/// Decodes an H.264 stream back into RGBA frames.
pub struct H264Decoder {
    decoder: Decoder,
    canvas: Option<RgbaFrame>,
}

impl std::fmt::Debug for H264Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("H264Decoder").finish_non_exhaustive()
    }
}

impl H264Decoder {
    pub fn new() -> Result<Self, CodecError> {
        let decoder = Decoder::new().map_err(|e| CodecError::H264(e.to_string()))?;
        Ok(Self { decoder, canvas: None })
    }

    /// Feeds one bitstream chunk. Returns the decoded frame, or `None` when the
    /// decoder needs more data before producing a picture (e.g. parameter sets).
    pub fn decode(&mut self, payload: &[u8]) -> Result<Option<&RgbaFrame>, CodecError> {
        match self
            .decoder
            .decode(payload)
            .map_err(|e| CodecError::H264(e.to_string()))?
        {
            Some(yuv) => {
                let (width, height) = yuv.dimensions();
                let canvas = self.canvas.get_or_insert_with(|| RgbaFrame::black(0, 0));
                if canvas.width as usize != width || canvas.height as usize != height {
                    *canvas = RgbaFrame::black(width as u32, height as u32);
                }
                yuv.write_rgba8(&mut canvas.data);
                Ok(Some(canvas))
            }
            None => Ok(None),
        }
    }
}

fn crop(frame: &RgbaFrame, width: u32, height: u32) -> Vec<u8> {
    let src_stride = frame.width as usize * 4;
    let row_bytes = width as usize * 4;
    let mut out = Vec::with_capacity(row_bytes * height as usize);
    for row in 0..height as usize {
        let start = row * src_stride;
        out.extend_from_slice(&frame.data[start..start + row_bytes]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: u32, height: u32, shift: u8) -> RgbaFrame {
        let mut data = vec![0u8; width as usize * height as usize * 4];
        for y in 0..height {
            for x in 0..width {
                let i = (y as usize * width as usize + x as usize) * 4;
                data[i] = (x * 255 / width.max(1)) as u8;
                data[i + 1] = (y * 255 / height.max(1)) as u8;
                data[i + 2] = 128u8.wrapping_add(shift);
                data[i + 3] = 255;
            }
        }
        RgbaFrame::new(width, height, data).unwrap()
    }

    fn max_channel_diff(a: &RgbaFrame, b: &RgbaFrame) -> i32 {
        a.data
            .iter()
            .zip(b.data.iter())
            .map(|(x, y)| (*x as i32 - *y as i32).abs())
            .max()
            .unwrap_or(0)
    }

    fn encode_expecting_output(
        enc: &mut H264Encoder,
        frame_id: u64,
        frame: &RgbaFrame,
    ) -> MediaMessage {
        enc.encode(0, frame_id, frame)
            .unwrap()
            .expect("frame should not be rate-skipped")
    }

    #[test]
    fn h264_roundtrips_close_to_source() {
        let mut enc = H264Encoder::new(30).unwrap();
        let mut dec = H264Decoder::new().unwrap();

        let first = gradient(128, 96, 0);
        let MediaMessage::ScreenFrame { payload, codec, .. } =
            encode_expecting_output(&mut enc, 0, &first);
        assert_eq!(codec, VideoCodec::H264);
        let decoded = dec.decode(&payload).unwrap().expect("keyframe decodes").clone();
        assert_eq!((decoded.width, decoded.height), (128, 96));
        // Lossy codec on limited-range YUV: allow generous but bounded error.
        assert!(max_channel_diff(&decoded, &first) < 48);

        // A delta frame moves the canvas toward the new content.
        let second = gradient(128, 96, 60);
        if let Some(MediaMessage::ScreenFrame { payload, .. }) =
            enc.encode(0, 1, &second).unwrap()
        {
            if let Some(decoded) = dec.decode(&payload).unwrap() {
                assert!(max_channel_diff(decoded, &first) > 0, "canvas should change");
            }
        }
    }

    #[test]
    fn odd_dimensions_are_cropped_to_even() {
        let mut enc = H264Encoder::new(30).unwrap();
        let frame = gradient(129, 97, 0);
        let MediaMessage::ScreenFrame { width_px, height_px, .. } =
            encode_expecting_output(&mut enc, 0, &frame);
        assert_eq!((width_px, height_px), (128, 96));
    }

    #[test]
    fn resolution_change_reinitializes_encoder() {
        let mut enc = H264Encoder::new(30).unwrap();
        let mut dec = H264Decoder::new().unwrap();

        let MediaMessage::ScreenFrame { payload, .. } =
            encode_expecting_output(&mut enc, 0, &gradient(128, 96, 0));
        dec.decode(&payload).unwrap();

        enc.force_keyframe();
        let MediaMessage::ScreenFrame { payload, width_px, height_px, .. } =
            encode_expecting_output(&mut enc, 1, &gradient(256, 128, 0));
        assert_eq!((width_px, height_px), (256, 128));
        let decoded = dec.decode(&payload).unwrap().expect("new keyframe decodes");
        assert_eq!((decoded.width, decoded.height), (256, 128));
    }
}
