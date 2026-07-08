use crate::ProtocolError;

/// Upper bound on a single encoded message. Guards against a malformed or hostile
/// length prefix forcing an unbounded allocation. Large screen payloads stay well
/// under this because tiles are sent per-frame, not whole 4K buffers uncompressed.
pub const MAX_MESSAGE_LEN: usize = 64 * 1024 * 1024;

/// Incrementally reassembles length-delimited frames from a byte stream.
///
/// Feed arbitrary chunks with [`FrameDecoder::extend`], then drain complete frame
/// bodies with [`FrameDecoder::next_frame`]. The 4-byte length prefix is consumed
/// internally; callers only ever see message bodies.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends received bytes to the internal buffer.
    pub fn extend(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Returns the next complete frame body, or `Ok(None)` if more bytes are needed.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, ProtocolError> {
        if self.buffer.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_le_bytes([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
        ]) as usize;
        if len > MAX_MESSAGE_LEN {
            return Err(ProtocolError::MessageTooLarge(len));
        }
        if self.buffer.len() < 4 + len {
            return Ok(None);
        }
        let body = self.buffer[4..4 + len].to_vec();
        self.buffer.drain(..4 + len);
        Ok(Some(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode, encode, ControlMessage};

    #[test]
    fn splits_multiple_frames_from_one_chunk() {
        let a = encode(&ControlMessage::Ping { nonce: 7 }).unwrap();
        let b = encode(&ControlMessage::Pong { nonce: 9 }).unwrap();
        let mut stream = a;
        stream.extend_from_slice(&b);

        let mut decoder = FrameDecoder::new();
        decoder.extend(&stream);

        let first: ControlMessage = decode(&decoder.next_frame().unwrap().unwrap()).unwrap();
        let second: ControlMessage = decode(&decoder.next_frame().unwrap().unwrap()).unwrap();
        assert_eq!(first, ControlMessage::Ping { nonce: 7 });
        assert_eq!(second, ControlMessage::Pong { nonce: 9 });
        assert!(decoder.next_frame().unwrap().is_none());
    }

    #[test]
    fn reassembles_frame_delivered_byte_by_byte() {
        let framed = encode(&ControlMessage::Ping { nonce: 42 }).unwrap();
        let mut decoder = FrameDecoder::new();
        for byte in &framed[..framed.len() - 1] {
            decoder.extend(&[*byte]);
            assert!(decoder.next_frame().unwrap().is_none());
        }
        decoder.extend(&[*framed.last().unwrap()]);
        let msg: ControlMessage = decode(&decoder.next_frame().unwrap().unwrap()).unwrap();
        assert_eq!(msg, ControlMessage::Ping { nonce: 42 });
    }

    #[test]
    fn rejects_oversized_length_prefix() {
        let mut decoder = FrameDecoder::new();
        decoder.extend(&u32::MAX.to_le_bytes());
        assert!(matches!(
            decoder.next_frame(),
            Err(ProtocolError::MessageTooLarge(_))
        ));
    }
}
