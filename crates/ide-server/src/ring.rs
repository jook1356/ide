//! Per-session terminal output ring buffer addressed by monotonically increasing byte offsets.

use std::collections::VecDeque;

/// Default capacity: 8 MiB per session.
pub const RING_CAPACITY: usize = 8 << 20;

/// Bytes `[start, end)` of a session's output; older bytes are dropped once `cap` is exceeded.
#[derive(Debug)]
pub struct Ring {
    buf: VecDeque<u8>,
    cap: usize,
    start: u64,
}

/// A read from the ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingRead {
    /// Offset of `bytes[0]`.
    pub offset: u64,
    /// The requested offset was no longer (or not yet) in the ring; reading restarted at
    /// the ring start.
    pub reset: bool,
    /// Up to `max` bytes.
    pub bytes: Vec<u8>,
}

impl Ring {
    /// An empty ring.
    pub fn new(cap: usize) -> Self {
        Ring {
            buf: VecDeque::new(),
            cap: cap.max(1),
            start: 0,
        }
    }

    /// Offset of the oldest byte kept.
    pub fn start(&self) -> u64 {
        self.start
    }

    /// Offset just past the newest byte (= total bytes ever pushed).
    pub fn end(&self) -> u64 {
        self.start + self.buf.len() as u64
    }

    /// Append output.
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend(data);
        if self.buf.len() > self.cap {
            let drop = self.buf.len() - self.cap;
            self.buf.drain(..drop);
            self.start += drop as u64;
        }
    }

    /// Where a replay of `from` starts: `(offset, reset)`.
    pub fn resolve(&self, from: Option<u64>) -> (u64, bool) {
        match from {
            Some(o) if o >= self.start && o <= self.end() => (o, false),
            _ => (self.start, true),
        }
    }

    /// Read up to `max` bytes starting at `from` (see [`Ring::resolve`]).
    pub fn read(&self, from: u64, max: usize) -> RingRead {
        let (offset, reset) = self.resolve(Some(from));
        let skip = (offset - self.start) as usize;
        let n = (self.buf.len() - skip).min(max);
        let (a, b) = self.buf.as_slices();
        let mut bytes = Vec::with_capacity(n);
        if skip < a.len() {
            let take = n.min(a.len() - skip);
            bytes.extend_from_slice(&a[skip..skip + take]);
            bytes.extend_from_slice(&b[..n - take]);
        } else {
            let s = skip - a.len();
            bytes.extend_from_slice(&b[s..s + n]);
        }
        RingRead {
            offset,
            reset,
            bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_and_wraparound() {
        let mut r = Ring::new(10);
        r.push(b"hello");
        assert_eq!((r.start(), r.end()), (0, 5));
        assert_eq!(r.read(1, 3).bytes, b"ell");
        assert_eq!(r.read(5, 3).bytes, b"");
        r.push(b" world!!");
        assert_eq!((r.start(), r.end()), (3, 13));
        let all = r.read(3, 100);
        assert_eq!((all.offset, all.reset), (3, false));
        assert_eq!(all.bytes, b"lo world!!");
        // Overwritten offset -> reset to the ring start.
        let old = r.read(0, 4);
        assert_eq!(
            (old.offset, old.reset, old.bytes.as_slice()),
            (3, true, &b"lo w"[..])
        );
        // Offset from the future (e.g. a previous server instance) -> reset.
        assert_eq!(r.resolve(Some(99)), (3, true));
        assert_eq!(r.resolve(None), (3, true));
        assert_eq!(r.resolve(Some(13)), (13, false));
        // Reads across the VecDeque seam.
        for i in 0..50u8 {
            r.push(&[i]);
            let e = r.end();
            let rr = r.read(e - 4, 10);
            assert_eq!(rr.bytes.len(), 4);
            assert_eq!(*rr.bytes.last().unwrap(), i);
        }
    }
}
