//! Frame codec: `u32 length (BE, of the rest) | u8 kind | u32 stream_id (BE) | payload`.

use bytes::{Buf, BufMut, Bytes, BytesMut};

/// Largest allowed `length` field (kind + stream id + payload): 1 MiB.
pub const MAX_FRAME_LEN: usize = 1 << 20;
/// Bytes of `kind` + `stream_id`.
pub const FRAME_HEADER_LEN: usize = 5;
/// Largest payload of one frame. Larger payloads are split by the multiplexer.
pub const MAX_PAYLOAD: usize = MAX_FRAME_LEN - FRAME_HEADER_LEN;

/// Frame kind byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FrameKind {
    /// MessagePack `RpcMessage` on stream 0 (`stream_id` 1 = fragment, more follows; 0 = last).
    Rpc = 0,
    /// Raw stream bytes.
    Data = 1,
    /// Flow-control credit: payload = `u32` BE byte count granted to the sender.
    Window = 2,
    /// Opens a stream: payload = `u32` BE initial window the opener grants to the peer.
    Open = 3,
    /// Closes a stream (payload ignored).
    Close = 4,
}

impl FrameKind {
    /// Parse the kind byte.
    pub fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            0 => FrameKind::Rpc,
            1 => FrameKind::Data,
            2 => FrameKind::Window,
            3 => FrameKind::Open,
            4 => FrameKind::Close,
            _ => return None,
        })
    }
}

/// One frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Kind.
    pub kind: FrameKind,
    /// Stream id (0 = control/RPC).
    pub stream: u32,
    /// Payload (at most [`MAX_PAYLOAD`] bytes).
    pub payload: Bytes,
}

impl Frame {
    /// A frame.
    pub fn new(kind: FrameKind, stream: u32, payload: impl Into<Bytes>) -> Self {
        Frame {
            kind,
            stream,
            payload: payload.into(),
        }
    }

    /// A `Window` frame granting `credit` bytes.
    pub fn window(stream: u32, credit: u32) -> Self {
        Frame::new(
            FrameKind::Window,
            stream,
            Bytes::copy_from_slice(&credit.to_be_bytes()),
        )
    }

    /// The `u32` carried by `Window`/`Open` frames.
    pub fn u32_payload(&self) -> Option<u32> {
        let b: [u8; 4] = self.payload.get(..4)?.try_into().ok()?;
        Some(u32::from_be_bytes(b))
    }

    /// Size on the wire.
    pub fn encoded_len(&self) -> usize {
        4 + FRAME_HEADER_LEN + self.payload.len()
    }

    /// Append the encoded frame to `out`. Panics if the payload exceeds [`MAX_PAYLOAD`]
    /// (a programming error: the multiplexer splits payloads).
    pub fn encode(&self, out: &mut BytesMut) {
        assert!(self.payload.len() <= MAX_PAYLOAD, "frame payload too large");
        out.reserve(self.encoded_len());
        out.put_u32((FRAME_HEADER_LEN + self.payload.len()) as u32);
        out.put_u8(self.kind as u8);
        out.put_u32(self.stream);
        out.put_slice(&self.payload);
    }
}

/// Malformed input.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// `length` is below the header size or above [`MAX_FRAME_LEN`].
    #[error("invalid frame length {0}")]
    BadLength(usize),
    /// Unknown kind byte.
    #[error("unknown frame kind {0}")]
    BadKind(u8),
}

/// Decode one frame from the front of `buf` if it is complete (incremental: call again after
/// appending more bytes). Consumes the frame's bytes on success.
pub fn decode(buf: &mut BytesMut) -> Result<Option<Frame>, FrameError> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if !(FRAME_HEADER_LEN..=MAX_FRAME_LEN).contains(&len) {
        return Err(FrameError::BadLength(len));
    }
    if buf.len() < 5 {
        return Ok(None);
    }
    let kind_byte = buf[4];
    let kind = FrameKind::from_u8(kind_byte).ok_or(FrameError::BadKind(kind_byte))?;
    if buf.len() < 4 + len {
        buf.reserve(4 + len - buf.len());
        return Ok(None);
    }
    buf.advance(5);
    let stream = buf.get_u32();
    let payload = buf.split_to(len - FRAME_HEADER_LEN).freeze();
    Ok(Some(Frame {
        kind,
        stream,
        payload,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(f: &Frame) -> BytesMut {
        let mut b = BytesMut::new();
        f.encode(&mut b);
        b
    }

    #[test]
    fn round_trip_all_kinds() {
        let frames = [
            Frame::new(FrameKind::Rpc, 0, &b"\x81\xa1a\x01"[..]),
            Frame::new(FrameKind::Data, 7, vec![9u8; 1000]),
            Frame::window(7, 256 * 1024),
            Frame::new(
                FrameKind::Open,
                2,
                Bytes::copy_from_slice(&65536u32.to_be_bytes()),
            ),
            Frame::new(FrameKind::Close, 2, Bytes::new()),
            Frame::new(FrameKind::Data, u32::MAX, vec![1u8; MAX_PAYLOAD]),
        ];
        let mut all = BytesMut::new();
        for f in &frames {
            f.encode(&mut all);
        }
        for f in &frames {
            assert_eq!(decode(&mut all).unwrap().as_ref(), Some(f));
        }
        assert!(all.is_empty());
        assert_eq!(decode(&mut all).unwrap(), None);
        assert_eq!(Frame::window(1, 42).u32_payload(), Some(42));
    }

    #[test]
    fn split_frames_byte_by_byte() {
        let a = Frame::new(FrameKind::Data, 3, &b"hello world"[..]);
        let b = Frame::new(FrameKind::Rpc, 0, &b"xyz"[..]);
        let mut wire = enc(&a);
        wire.extend_from_slice(&enc(&b));
        let mut buf = BytesMut::new();
        let mut out = Vec::new();
        for byte in wire.iter() {
            buf.put_u8(*byte);
            while let Some(f) = decode(&mut buf).unwrap() {
                out.push(f);
            }
        }
        assert_eq!(out, vec![a, b]);
    }

    #[test]
    fn rejects_bad_input() {
        let mut b = BytesMut::from(&[0u8, 0, 0, 2, 0, 0][..]);
        assert_eq!(decode(&mut b), Err(FrameError::BadLength(2)));
        let mut b = BytesMut::new();
        b.put_u32((MAX_FRAME_LEN + 1) as u32);
        b.put_u8(1);
        assert_eq!(
            decode(&mut b),
            Err(FrameError::BadLength(MAX_FRAME_LEN + 1))
        );
        let mut b = BytesMut::new();
        b.put_u32(5);
        b.put_u8(99);
        b.put_u32(0);
        assert_eq!(decode(&mut b), Err(FrameError::BadKind(99)));
    }
}
