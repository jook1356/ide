//! Terminal data stream records. A terminal stream is a byte stream of records
//! `u64 offset (BE) | u8 flags | u32 len (BE) | bytes`; flag bit 0 = reset. Records may be
//! split across `Data` frames arbitrarily; [`ChunkDecoder`] reassembles them.

use bytes::{Buf, BufMut, Bytes, BytesMut};

const HEADER: usize = 13;
const FLAG_RESET: u8 = 1;
/// Largest record body accepted by the decoder (the server sends at most 64 KiB).
pub const MAX_CHUNK: usize = 1 << 20;

/// A piece of terminal output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermChunk {
    /// Session id.
    pub session: String,
    /// Offset of `bytes[0]` in the session's output.
    pub offset: u64,
    /// The client must clear its terminal model before applying `bytes` (replay restarted at
    /// the ring start because the requested offset was overwritten).
    pub reset: bool,
    /// Raw PTY output.
    pub bytes: Bytes,
}

impl TermChunk {
    /// Offset just past this chunk.
    pub fn end(&self) -> u64 {
        self.offset + self.bytes.len() as u64
    }
}

/// Encode one record (without the session id, which the stream implies).
pub fn encode_chunk(offset: u64, reset: bool, bytes: &[u8], out: &mut BytesMut) {
    out.reserve(HEADER + bytes.len());
    out.put_u64(offset);
    out.put_u8(if reset { FLAG_RESET } else { 0 });
    out.put_u32(bytes.len() as u32);
    out.put_slice(bytes);
}

/// Incremental record decoder.
#[derive(Debug, Default)]
pub struct ChunkDecoder {
    buf: BytesMut,
}

/// A malformed record.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("terminal record too large ({0} bytes)")]
pub struct ChunkError(pub usize);

impl ChunkDecoder {
    /// Feed stream bytes.
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Next complete record: (offset, reset, bytes).
    pub fn next_record(&mut self) -> Result<Option<(u64, bool, Bytes)>, ChunkError> {
        if self.buf.len() < HEADER {
            return Ok(None);
        }
        let len = u32::from_be_bytes(self.buf[9..13].try_into().unwrap()) as usize;
        if len > MAX_CHUNK {
            return Err(ChunkError(len));
        }
        if self.buf.len() < HEADER + len {
            return Ok(None);
        }
        let offset = self.buf.get_u64();
        let flags = self.buf.get_u8();
        self.buf.advance(4);
        let bytes = self.buf.split_to(len).freeze();
        Ok(Some((offset, flags & FLAG_RESET != 0, bytes)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_split_anywhere() {
        let mut wire = BytesMut::new();
        encode_chunk(0, true, b"hello", &mut wire);
        encode_chunk(5, false, b"", &mut wire);
        encode_chunk(5, false, &[7u8; 3000], &mut wire);
        for step in [1usize, 2, 7, 13, 100, 10_000] {
            let mut d = ChunkDecoder::default();
            let mut out = Vec::new();
            for piece in wire.chunks(step) {
                d.push(piece);
                while let Some(r) = d.next_record().unwrap() {
                    out.push(r);
                }
            }
            assert_eq!(out.len(), 3, "step {step}");
            assert_eq!(out[0], (0, true, Bytes::from_static(b"hello")));
            assert_eq!(out[1].0, 5);
            assert_eq!(out[2].2.len(), 3000);
        }
    }
}
