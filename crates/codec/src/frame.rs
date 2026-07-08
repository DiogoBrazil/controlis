use crate::CodecError;

/// An RGBA screen frame: 4 bytes per pixel, row-major, top-left origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl RgbaFrame {
    /// Builds a frame, validating that the buffer length matches the dimensions.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Result<Self, CodecError> {
        let expected = width as usize * height as usize * 4;
        if data.len() != expected {
            return Err(CodecError::DimensionMismatch {
                width,
                height,
                len: data.len(),
            });
        }
        Ok(Self { width, height, data })
    }

    /// A black opaque frame of the given size.
    pub fn black(width: u32, height: u32) -> Self {
        let mut data = vec![0u8; width as usize * height as usize * 4];
        for px in data.chunks_exact_mut(4) {
            px[3] = 255;
        }
        Self { width, height, data }
    }

    pub(crate) fn same_size(&self, other: &RgbaFrame) -> bool {
        self.width == other.width && self.height == other.height
    }
}
