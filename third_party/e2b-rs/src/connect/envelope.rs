//! Connect protocol message framing (5-byte length-prefixed envelopes).

/// Envelope flag: payload is compressed.
// used by Task 5
#[allow(dead_code)]
pub(crate) const FLAG_COMPRESSED: u8 = 0b0000_0001;
/// Envelope flag: end-of-stream frame (payload is trailers/error JSON).
// used by Task 5
#[allow(dead_code)]
pub(crate) const FLAG_END_STREAM: u8 = 0b0000_0010;

const HEADER_LEN: usize = 5;

/// Encode one envelope: 5-byte header (`flags: u8` + `len: u32` big-endian) + `data`.
// used by Task 5
#[allow(dead_code)]
pub(crate) fn encode_envelope(flags: u8, data: &[u8]) -> Vec<u8> {
    let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(HEADER_LEN + data.len());
    out.push(flags);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(data);
    out
}

/// A decoded Connect frame.
// used by Task 5
#[allow(dead_code)]
pub(crate) struct Frame {
    /// Envelope flags byte.
    pub flags: u8,
    /// Frame payload.
    pub data: Vec<u8>,
}

// used by Task 5
#[allow(dead_code)]
impl Frame {
    /// Whether this is the end-of-stream frame.
    pub(crate) fn is_end_stream(&self) -> bool {
        self.flags & FLAG_END_STREAM != 0
    }
}

/// Incrementally decodes envelopes from a byte stream. Push response chunks,
/// then pull complete [`Frame`]s; partial frames stay buffered.
// used by Task 5
#[allow(dead_code)]
pub(crate) struct EnvelopeDecoder {
    buf: Vec<u8>,
}

// used by Task 5
#[allow(dead_code)]
impl EnvelopeDecoder {
    /// Create an empty decoder.
    pub(crate) fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Append received bytes.
    pub(crate) fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// Return the next complete frame, or `None` if more bytes are needed.
    pub(crate) fn next_frame(&mut self) -> Option<Frame> {
        if self.buf.len() < HEADER_LEN {
            return None;
        }
        let flags = self.buf[0];
        let len = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        if self.buf.len() < HEADER_LEN + len {
            return None;
        }
        let data = self.buf[HEADER_LEN..HEADER_LEN + len].to_vec();
        self.buf.drain(..HEADER_LEN + len);
        Some(Frame { flags, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_has_5_byte_be_header() {
        let env = encode_envelope(0, b"hi");
        // flags=0, len=2 (big-endian u32), then payload.
        assert_eq!(env, vec![0x00, 0x00, 0x00, 0x00, 0x02, b'h', b'i']);
        let end = encode_envelope(FLAG_END_STREAM, b"{}");
        assert_eq!(end[0], FLAG_END_STREAM);
    }

    #[test]
    fn decoder_yields_complete_frames_and_buffers_partials() {
        let mut dec = EnvelopeDecoder::new();
        let f1 = encode_envelope(0, b"one");
        let f2 = encode_envelope(FLAG_END_STREAM, b"{}");
        // Feed f1 split across two chunks + the start of f2.
        dec.push(&f1[..3]);
        assert!(dec.next_frame().is_none()); // header incomplete
        dec.push(&f1[3..]);
        let frame = dec.next_frame().expect("frame 1");
        assert_eq!(frame.data, b"one");
        assert!(!frame.is_end_stream());
        assert!(dec.next_frame().is_none()); // nothing buffered yet
        dec.push(&f2);
        let frame = dec.next_frame().expect("frame 2");
        assert_eq!(frame.data, b"{}");
        assert!(frame.is_end_stream());
        assert!(dec.next_frame().is_none());
    }

    #[test]
    fn decoder_handles_two_frames_in_one_chunk() {
        let mut dec = EnvelopeDecoder::new();
        let mut buf = encode_envelope(0, b"a");
        buf.extend(encode_envelope(0, b"bb"));
        dec.push(&buf);
        assert_eq!(dec.next_frame().expect("f1").data, b"a");
        assert_eq!(dec.next_frame().expect("f2").data, b"bb");
        assert!(dec.next_frame().is_none());
    }
}
