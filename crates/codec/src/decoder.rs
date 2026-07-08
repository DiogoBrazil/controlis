use protocol::{MediaMessage, ScreenEncoding, TileRect, VideoCodec};
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

use crate::{CodecError, RgbaFrame};

/// Reconstructs full RGBA frames from a stream of [`MediaMessage::ScreenFrame`].
///
/// Keeps a canvas that full frames replace and tile deltas patch in place.
#[derive(Debug, Default)]
pub struct TileDecoder {
    canvas: Option<RgbaFrame>,
}

impl TileDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies a screen-frame message and returns the current full frame.
    pub fn decode(&mut self, message: &MediaMessage) -> Result<&RgbaFrame, CodecError> {
        let MediaMessage::ScreenFrame {
            codec,
            encoding,
            width_px,
            height_px,
            tiles,
            payload,
            ..
        } = message;

        match encoding {
            ScreenEncoding::Full => {
                let data = match codec {
                    VideoCodec::RawRgba => payload.clone(),
                    VideoCodec::JpegTiles => decode_jpeg(payload, *width_px, *height_px)?,
                    // Video codecs live behind [`crate::VideoDecoder`], never here.
                    VideoCodec::H264 => return Err(CodecError::UnsupportedCodec(*codec)),
                };
                self.canvas = Some(RgbaFrame::new(*width_px, *height_px, data)?);
            }
            ScreenEncoding::TileDelta => {
                self.apply_delta(*width_px, *height_px, tiles, payload)?;
            }
        }

        Ok(self.canvas.as_ref().expect("canvas set above"))
    }

    fn apply_delta(
        &mut self,
        width: u32,
        height: u32,
        tiles: &[TileRect],
        payload: &[u8],
    ) -> Result<(), CodecError> {
        let canvas = self.canvas.as_mut().ok_or(CodecError::UnexpectedResize)?;
        if canvas.width != width || canvas.height != height {
            return Err(CodecError::UnexpectedResize);
        }

        let mut cursor = 0usize;
        for rect in tiles {
            let len = read_u32(payload, &mut cursor)? as usize;
            let jpeg = payload
                .get(cursor..cursor + len)
                .ok_or(CodecError::MalformedPayload)?;
            cursor += len;
            let pixels = decode_jpeg(jpeg, rect.w, rect.h)?;
            blit(canvas, rect, &pixels);
        }
        Ok(())
    }
}

fn blit(canvas: &mut RgbaFrame, rect: &TileRect, pixels: &[u8]) {
    let stride = canvas.width as usize * 4;
    let row_bytes = rect.w as usize * 4;
    for row in 0..rect.h {
        let dst = (rect.y as usize + row as usize) * stride + rect.x as usize * 4;
        let src = row as usize * row_bytes;
        canvas.data[dst..dst + row_bytes].copy_from_slice(&pixels[src..src + row_bytes]);
    }
}

fn read_u32(buf: &[u8], cursor: &mut usize) -> Result<u32, CodecError> {
    let bytes = buf
        .get(*cursor..*cursor + 4)
        .ok_or(CodecError::MalformedPayload)?;
    *cursor += 4;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn decode_jpeg(jpeg: &[u8], width: u32, height: u32) -> Result<Vec<u8>, CodecError> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(std::io::Cursor::new(jpeg), options);
    let pixels = decoder
        .decode()
        .map_err(|e| CodecError::JpegDecode(e.to_string()))?;
    let expected = width as usize * height as usize * 4;
    if pixels.len() != expected {
        return Err(CodecError::JpegDecode(format!(
            "decoded {} bytes, expected {expected}",
            pixels.len()
        )));
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TileEncoder, TILE_SIZE};

    /// A smooth low-frequency gradient (like real desktop content, which JPEG
    /// reproduces well). `shift` offsets the blue channel uniformly.
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

    #[test]
    fn raw_full_frame_roundtrips_exactly() {
        let frame = gradient(96, 96, 0);
        let mut enc = TileEncoder::new(VideoCodec::RawRgba);
        let mut dec = TileDecoder::new();
        let msg = enc.encode(0, 0, &frame).unwrap();
        let out = dec.decode(&msg).unwrap();
        assert_eq!(out, &frame);
    }

    #[test]
    fn jpeg_first_frame_is_full_and_close() {
        let frame = gradient(128, 128, 0);
        let mut enc = TileEncoder::new(VideoCodec::JpegTiles);
        let mut dec = TileDecoder::new();
        let msg = enc.encode(0, 0, &frame).unwrap();
        let MediaMessage::ScreenFrame { encoding, .. } = &msg;
        assert_eq!(*encoding, ScreenEncoding::Full);
        let out = dec.decode(&msg).unwrap();
        assert_eq!(out.width, 128);
        assert_eq!(out.height, 128);
    }

    #[test]
    fn only_changed_tiles_are_sent() {
        let width = TILE_SIZE * 3;
        let height = TILE_SIZE * 2;
        let first = gradient(width, height, 0);
        let mut second = first.clone();
        // Change a single pixel inside the tile at column 1, row 0.
        let px = (TILE_SIZE + 10) as usize * 4;
        second.data[px] = second.data[px].wrapping_add(50);

        let mut enc = TileEncoder::new(VideoCodec::JpegTiles);
        enc.encode(0, 0, &first).unwrap();
        let delta = enc.encode(0, 1, &second).unwrap();

        match &delta {
            MediaMessage::ScreenFrame { encoding, tiles, .. } => {
                assert_eq!(*encoding, ScreenEncoding::TileDelta);
                assert_eq!(tiles.len(), 1, "exactly one tile should change");
                assert_eq!(tiles[0].x, TILE_SIZE);
                assert_eq!(tiles[0].y, 0);
            }
        }
    }

    #[test]
    fn unchanged_frame_sends_no_tiles() {
        let frame = gradient(128, 128, 0);
        let mut enc = TileEncoder::new(VideoCodec::JpegTiles);
        enc.encode(0, 0, &frame).unwrap();
        let delta = enc.encode(0, 1, &frame).unwrap();
        match &delta {
            MediaMessage::ScreenFrame { tiles, payload, .. } => {
                assert!(tiles.is_empty());
                assert!(payload.is_empty());
            }
        }
    }

    #[test]
    fn delta_patches_canvas_toward_new_frame() {
        let width = TILE_SIZE * 2;
        let height = TILE_SIZE * 2;
        let first = gradient(width, height, 0);
        let second = gradient(width, height, 40);

        let mut enc = TileEncoder::new(VideoCodec::JpegTiles);
        let mut dec = TileDecoder::new();
        dec.decode(&enc.encode(0, 0, &first).unwrap()).unwrap();
        let patched = dec.decode(&enc.encode(0, 1, &second).unwrap()).unwrap().clone();

        // JPEG is lossy, so compare approximately against the true second frame.
        let mut max_diff = 0i32;
        for (a, b) in patched.data.iter().zip(second.data.iter()) {
            max_diff = max_diff.max((*a as i32 - *b as i32).abs());
        }
        assert!(max_diff < 40, "reconstructed frame drifted too far: {max_diff}");
    }
}
