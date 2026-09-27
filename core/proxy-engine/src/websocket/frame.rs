//! RFC 6455 framing: enough to capture and to edit, and no more.
//!
//! ```text
//!  0               1               2
//!  FIN RSV opcode  MASK length     payload
//! ```
//!
//! A proxy that re-frames every byte it copies can only get the handshake
//! the two ends already agreed on wrong. So an unmodified message is written
//! back as the bytes it arrived as. Editing is the one case that builds a
//! new frame: a text payload that a breakpoint changed, sent as one message
//! with the masking the direction requires.
//!
//! Bytes that are not a frame — which is what several of the existing
//! tunnel tests send — are not an error here. The reader says so, and the
//! tunnel falls back to copying the rest unchanged.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt};

/// The most payload one assembled message is allowed to hold in memory.
///
/// Capture already stops at a megabyte; this is the ceiling on *forwarding*
/// a parsed message. A frame larger than this is treated as opaque bytes and
/// copied rather than buffered.
pub const MAX_ASSEMBLED_BYTES: usize = 16 * 1024 * 1024;

/// One WebSocket frame, plus the exact bytes it arrived as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The on-wire bytes, including the header and any mask.
    pub raw: Vec<u8>,
    /// Whether this is the last frame of its message.
    pub fin: bool,
    /// The opcode, unmasked.
    pub opcode: Opcode,
    /// The payload, already unmasked.
    pub payload: Vec<u8>,
}

/// A frame opcode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opcode {
    /// A continuation of the message in progress.
    Continuation,
    /// A text message (or its first frame).
    Text,
    /// A binary message (or its first frame).
    Binary,
    /// The close handshake.
    Close,
    /// A ping.
    Ping,
    /// A pong.
    Pong,
}

impl Opcode {
    /// The opcode as the four bits on the wire.
    fn as_nibble(self) -> u8 {
        match self {
            Self::Continuation => 0,
            Self::Text => 1,
            Self::Binary => 2,
            Self::Close => 8,
            Self::Ping => 9,
            Self::Pong => 10,
        }
    }

    /// Whether this is a control opcode.
    pub fn is_control(self) -> bool {
        matches!(self, Self::Close | Self::Ping | Self::Pong)
    }

    /// Whether this starts a data message.
    pub fn is_data(self) -> bool {
        matches!(self, Self::Text | Self::Binary)
    }
}

/// Why a buffer could not be read as a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// More bytes are needed before a decision can be made.
    Incomplete,
    /// These bytes are not a WebSocket frame.
    Invalid,
}

/// Reads one frame from `bytes`, returning it and how many bytes it used.
pub fn decode(bytes: &[u8]) -> Result<(Frame, usize), FrameError> {
    if bytes.len() < 2 {
        return Err(FrameError::Incomplete);
    }

    let first = bytes[0];
    let second = bytes[1];
    let fin = first & 0x80 != 0;
    let rsv = first & 0x70;
    let opcode = match first & 0x0f {
        0 => Opcode::Continuation,
        1 => Opcode::Text,
        2 => Opcode::Binary,
        8 => Opcode::Close,
        9 => Opcode::Ping,
        10 => Opcode::Pong,
        _ => return Err(FrameError::Invalid),
    };

    // RSV bits are for extensions the handshake negotiated. This proxy does
    // not negotiate any, so a set bit is not a frame we should interpret.
    if rsv != 0 {
        return Err(FrameError::Invalid);
    }

    let masked = second & 0x80 != 0;
    let mut length = (second & 0x7f) as usize;
    let mut offset = 2;

    if opcode.is_control() {
        // Control frames are unfragmented and at most 125 bytes.
        if !fin || length > 125 {
            return Err(FrameError::Invalid);
        }
    }

    match length {
        126 => {
            if bytes.len() < offset + 2 {
                return Err(FrameError::Incomplete);
            }
            length = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]) as usize;
            offset += 2;
            if length < 126 {
                return Err(FrameError::Invalid);
            }
        }
        127 => {
            if bytes.len() < offset + 8 {
                return Err(FrameError::Incomplete);
            }
            let n = u64::from_be_bytes(bytes[offset..offset + 8].try_into().expect("8 bytes"));
            offset += 8;
            if n < 65_536 || n > MAX_ASSEMBLED_BYTES as u64 {
                return Err(FrameError::Invalid);
            }
            length = n as usize;
        }
        _ => {}
    }

    if opcode.is_control() && length > 125 {
        return Err(FrameError::Invalid);
    }

    let mask = if masked {
        if bytes.len() < offset + 4 {
            return Err(FrameError::Incomplete);
        }
        let mask = [
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ];
        offset += 4;
        Some(mask)
    } else {
        None
    };

    if bytes.len() < offset + length {
        return Err(FrameError::Incomplete);
    }

    let mut payload = bytes[offset..offset + length].to_vec();
    if let Some(mask) = mask {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }

    let used = offset + length;
    Ok((
        Frame {
            raw: bytes[..used].to_vec(),
            fin,
            opcode,
            payload,
        },
        used,
    ))
}

/// Builds one complete data frame carrying `payload`.
///
/// `mask` is set for client-to-server, as RFC 6455 requires, and cleared
/// the other way. The opcode is text or binary; the frame is always FIN.
pub fn encode_message(opcode: Opcode, payload: &[u8], mask: bool) -> Vec<u8> {
    debug_assert!(opcode.is_data());

    let mut frame = Vec::with_capacity(14 + payload.len());
    frame.push(0x80 | opcode.as_nibble());

    let len = payload.len();
    if len < 126 {
        frame.push((u8::from(mask) << 7) | len as u8);
    } else if len <= 65_535 {
        frame.push((u8::from(mask) << 7) | 126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push((u8::from(mask) << 7) | 127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }

    if mask {
        let key = fresh_mask();
        frame.extend_from_slice(&key);
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ key[index % 4]),
        );
    } else {
        frame.extend_from_slice(payload);
    }

    frame
}

/// A masking key that is not all zeroes.
///
/// Masking is not a secret: it exists to stop a proxy from seeing a
/// crafted payload as HTTP. A timestamp is enough.
fn fresh_mask() -> [u8; 4] {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u32)
        .unwrap_or(1);
    let mut key = nanos.to_le_bytes();
    if key == [0, 0, 0, 0] {
        key[0] = 1;
    }
    key
}

/// Reads frames from an `AsyncRead` until one complete data message is
/// assembled, a control frame is seen, or the bytes stop being frames.
pub struct FrameReader<R> {
    inner: R,
    buffer: Vec<u8>,
    assembler: Assembler,
}

/// What [`FrameReader::next`] produced.
#[derive(Debug)]
pub enum Read {
    /// A complete data message, with the original frames it arrived as.
    Message(Assembled),
    /// A control frame, to be forwarded as-is.
    Control(Frame),
    /// These bytes are not a frame; the reader has been drained into them.
    Opaque(Vec<u8>),
    /// The read half reached end of file, with nothing left unparsed.
    Eof,
}

/// A data message assembled from one or more frames.
#[derive(Debug)]
pub struct Assembled {
    /// Text or binary, from the first frame.
    pub opcode: Opcode,
    /// The payload, concatenated and unmasked.
    pub payload: Vec<u8>,
    /// The original frames, in order, for an unmodified forward.
    pub frames: Vec<Frame>,
}

impl Assembled {
    /// The original bytes, concatenated, so an unmodified message is what
    /// arrived rather than a re-encoded one.
    pub fn raw(&self) -> Vec<u8> {
        self.frames
            .iter()
            .flat_map(|frame| frame.raw.iter().copied())
            .collect()
    }
}

/// Assembles fragmented data frames into one message.
#[derive(Default)]
struct Assembler {
    opcode: Option<Opcode>,
    payload: Vec<u8>,
    frames: Vec<Frame>,
}

impl Assembler {
    fn push(&mut self, frame: Frame) -> Result<Option<Assembled>, FrameError> {
        if frame.opcode.is_control() {
            return Err(FrameError::Invalid);
        }

        match frame.opcode {
            Opcode::Continuation => {
                if self.opcode.is_none() {
                    return Err(FrameError::Invalid);
                }
            }
            opcode if opcode.is_data() => {
                if self.opcode.is_some() {
                    return Err(FrameError::Invalid);
                }
                self.opcode = Some(opcode);
            }
            _ => return Err(FrameError::Invalid),
        }

        if self.payload.len().saturating_add(frame.payload.len()) > MAX_ASSEMBLED_BYTES {
            return Err(FrameError::Invalid);
        }

        self.payload.extend_from_slice(&frame.payload);
        let fin = frame.fin;
        self.frames.push(frame);

        if fin {
            Ok(Some(Assembled {
                opcode: self.opcode.take().ok_or(FrameError::Invalid)?,
                payload: std::mem::take(&mut self.payload),
                frames: std::mem::take(&mut self.frames),
            }))
        } else {
            Ok(None)
        }
    }

    fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    fn take_raw(&mut self) -> Vec<u8> {
        let raw: Vec<u8> = self
            .frames
            .iter()
            .flat_map(|frame| frame.raw.iter().copied())
            .collect();
        self.opcode = None;
        self.payload.clear();
        self.frames.clear();
        raw
    }
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    /// A reader over `inner`.
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            buffer: Vec::new(),
            assembler: Assembler::default(),
        }
    }

    /// The next message, control frame, opaque remainder, or end of file.
    pub async fn next(&mut self) -> io::Result<Read> {
        loop {
            match decode(&self.buffer) {
                Ok((frame, used)) => {
                    let _ = self.buffer.drain(..used);
                    if frame.opcode.is_control() {
                        if !self.assembler.is_empty() {
                            // A control frame may be interleaved with a
                            // fragmented message. Hand it out; the assembler
                            // keeps the data frames it already has.
                            return Ok(Read::Control(frame));
                        }
                        return Ok(Read::Control(frame));
                    }

                    match self.assembler.push(frame) {
                        Ok(Some(message)) => return Ok(Read::Message(message)),
                        Ok(None) => continue,
                        Err(_) => return Ok(Read::Opaque(self.take_opaque())),
                    }
                }
                Err(FrameError::Incomplete) => {
                    let mut chunk = [0u8; 8 * 1024];
                    let read = self.inner.read(&mut chunk).await?;
                    if read == 0 {
                        if self.buffer.is_empty() && self.assembler.is_empty() {
                            return Ok(Read::Eof);
                        }
                        return Ok(Read::Opaque(self.take_opaque()));
                    }
                    self.buffer.extend_from_slice(&chunk[..read]);
                }
                Err(FrameError::Invalid) => return Ok(Read::Opaque(self.take_opaque())),
            }
        }
    }

    /// The bytes already held, plus whatever the assembler had, so a fallback
    /// copy can start from exactly where parsing stopped.
    fn take_opaque(&mut self) -> Vec<u8> {
        let mut opaque = self.assembler.take_raw();
        opaque.append(&mut self.buffer);
        opaque
    }

    /// Gives the inner reader back, after a fallback has copied `lead`.
    pub fn into_inner(self) -> R {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(payload: &[u8], key: [u8; 4]) -> Vec<u8> {
        payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ key[index % 4])
            .collect()
    }

    fn text_unmasked(payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![0x81, payload.len() as u8];
        frame.extend_from_slice(payload);
        frame
    }

    fn text_masked(payload: &[u8], key: [u8; 4]) -> Vec<u8> {
        let mut frame = vec![0x81, 0x80 | payload.len() as u8];
        frame.extend_from_slice(&key);
        frame.extend(mask(payload, key));
        frame
    }

    #[test]
    fn a_text_frame_is_decoded_with_its_payload() {
        let bytes = text_unmasked(b"hello");
        let (frame, used) = decode(&bytes).expect("a frame");

        assert_eq!(used, bytes.len());
        assert!(frame.fin);
        assert_eq!(frame.opcode, Opcode::Text);
        assert_eq!(frame.payload, b"hello");
        assert_eq!(frame.raw, bytes);
    }

    #[test]
    fn a_masked_frame_is_unmasked() {
        let key = [0x37, 0xfa, 0x21, 0x3d];
        let bytes = text_masked(b"hello", key);
        let (frame, _) = decode(&bytes).expect("a frame");

        assert_eq!(frame.payload, b"hello");
    }

    #[test]
    fn a_binary_frame_keeps_its_bytes() {
        let payload = [0x00, 0xff, 0x7f];
        let mut bytes = vec![0x82, 0x03];
        bytes.extend_from_slice(&payload);
        let (frame, _) = decode(&bytes).expect("a frame");

        assert_eq!(frame.opcode, Opcode::Binary);
        assert_eq!(frame.payload, payload);
    }

    #[test]
    fn reserved_bits_are_not_a_frame() {
        assert_eq!(decode(&[0xc1, 0x00]), Err(FrameError::Invalid));
    }

    #[test]
    fn an_unknown_opcode_is_not_a_frame() {
        assert_eq!(decode(&[0x83, 0x00]), Err(FrameError::Invalid));
    }

    #[test]
    fn opaque_bytes_are_not_a_frame() {
        assert_eq!(decode(b"hello"), Err(FrameError::Invalid));
    }

    #[test]
    fn a_short_buffer_asks_for_more() {
        assert_eq!(decode(&[0x81]), Err(FrameError::Incomplete));
        assert_eq!(decode(&[0x81, 0x05, b'h']), Err(FrameError::Incomplete));
    }

    #[test]
    fn an_edited_text_message_round_trips() {
        let encoded = encode_message(Opcode::Text, br#"{"edited":true}"#, true);
        let (frame, used) = decode(&encoded).expect("a frame");

        assert_eq!(used, encoded.len());
        assert_eq!(frame.opcode, Opcode::Text);
        assert_eq!(frame.payload, br#"{"edited":true}"#);
    }

    #[test]
    fn fragmented_frames_assemble_in_order() {
        let mut assembler = Assembler::default();
        let first = decode(&[0x01, 0x03, b'h', b'e', b'l']).expect("first").0;
        let second = decode(&[0x80, 0x02, b'l', b'o']).expect("second").0;

        assert!(assembler.push(first).expect("continues").is_none());
        let message = assembler
            .push(second)
            .expect("finishes")
            .expect("a message");

        assert_eq!(message.opcode, Opcode::Text);
        assert_eq!(message.payload, b"hello");
        assert_eq!(
            message.raw(),
            [0x01, 0x03, b'h', b'e', b'l', 0x80, 0x02, b'l', b'o']
        );
    }

    #[tokio::test]
    async fn a_reader_emits_one_text_message() {
        let mut reader = FrameReader::new(std::io::Cursor::new(text_unmasked(b"hi")));
        match reader.next().await.expect("reads") {
            Read::Message(message) => {
                assert_eq!(message.opcode, Opcode::Text);
                assert_eq!(message.payload, b"hi");
            }
            other => panic!("expected a message, got {other:?}"),
        }
        assert!(matches!(reader.next().await.expect("eof"), Read::Eof));
    }

    #[tokio::test]
    async fn a_reader_falls_back_on_opaque_bytes() {
        let mut reader = FrameReader::new(std::io::Cursor::new(b"second"));
        match reader.next().await.expect("reads") {
            Read::Opaque(bytes) => assert_eq!(bytes, b"second"),
            other => panic!("expected opaque bytes, got {other:?}"),
        }
    }
}
