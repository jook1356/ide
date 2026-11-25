//! On-disk copy of a session terminal's output (`sessions/<id>/output.log`).
//!
//! The ring buffer ([`crate::ring`]) lives in memory, so a server restart used to start every
//! session's terminal empty: a resumed agent only shows what its new process draws, and the
//! scrollback from before the restart is gone. The log mirrors the ring's tail on disk; a new
//! server seeds the session's ring with it ([`crate::pty::TermHost::attach_log`]) and writes
//! [`RESUME_TRAILER`] before the next process starts, so attached clients replay the old output
//! into their scrollback and the new process starts on a clean screen below it.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::ring::RING_CAPACITY;

/// Bytes kept on disk (and restored) per session: as much as the ring holds.
pub const LOG_CAP: usize = RING_CAPACITY;

/// Written after restored output, before a new process' first byte:
/// cancel a sequence the old output may have left open (CAN), back to the primary screen with
/// default modes (scroll region, origin, SGR, mouse reporting, focus, bracketed paste,
/// application cursor/keypad, autowrap, cursor shown, kitty keyboard stack), a dim rule under
/// the old output at the bottom row, then a cleared screen with the cursor at the top. A
/// terminal moves a cleared primary screen's lines into its scrollback (alacritty, xterm), so the
/// old output and the rule stay scrollable while the new process (ConPTY asks for the cursor and
/// draws from there) starts on an empty screen, as on a fresh terminal.
pub const RESUME_TRAILER: &[u8] = concat!(
    "\x18",
    "\x1b[?1049l\x1b[r\x1b[?6l\x1b[0m",
    "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1005l\x1b[?1006l\x1b[?1004l\x1b[?2004l",
    "\x1b[?1l\x1b>\x1b[?7h\x1b[?25h\x1b[<99u",
    "\x1b[999;1H\r\n\x1b[2m",
    "────────────────────────────────────────",
    "\x1b[0m\x1b[2J\x1b[H",
)
.as_bytes();

/// The log file of one session terminal (append-only, compacted to its last `cap` bytes once it
/// grows past 1.5 × `cap`).
#[derive(Debug)]
pub struct OutputLog {
    path: PathBuf,
    file: File,
    len: u64,
    cap: usize,
}

impl OutputLog {
    /// Opens (creating) the log at `path` and returns it with the output it kept (at most `cap`
    /// bytes, starting at a line start when older bytes were cut). The file is rewritten to
    /// exactly what is returned.
    pub fn open(path: &Path, cap: usize) -> io::Result<(OutputLog, Vec<u8>)> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let mut log = OutputLog {
            path: path.to_owned(),
            len: file.seek(SeekFrom::End(0))?,
            file,
            cap: cap.max(1),
        };
        let kept = log.compact()?;
        Ok((log, kept))
    }

    /// The log's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append output; compacts when the file passed 1.5 × the cap.
    pub fn append(&mut self, data: &[u8]) -> io::Result<()> {
        self.file.write_all(data)?;
        self.len += data.len() as u64;
        if self.len > (self.cap + self.cap / 2) as u64 {
            self.compact()?;
        }
        Ok(())
    }

    /// Rewrite the file to its kept tail ([`tail`]) and return that tail.
    fn compact(&mut self) -> io::Result<Vec<u8>> {
        let from = self.len.saturating_sub(self.cap as u64);
        self.file.seek(SeekFrom::Start(from))?;
        let mut bytes = Vec::with_capacity((self.len - from) as usize);
        self.file.read_to_end(&mut bytes)?;
        let skip = if from > 0 { line_start(&bytes) } else { 0 };
        if from > 0 {
            bytes.drain(..skip);
            self.file.set_len(0)?;
            self.file.seek(SeekFrom::Start(0))?;
            self.file.write_all(&bytes)?;
            self.len = bytes.len() as u64;
        }
        self.file.seek(SeekFrom::Start(self.len))?;
        Ok(bytes)
    }
}

/// Where a tail cut at an arbitrary byte becomes safe to replay: just after its first newline
/// (never inside a UTF-8 character, and usually outside escape sequences), or 0 when there is
/// none.
fn line_start(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .position(|&b| b == b'\n')
        .map_or(0, |i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("meninx-outlog-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn appends_and_reopens() {
        let dir = tmp("reopen");
        let path = dir.join("s").join("output.log");
        let (mut log, kept) = OutputLog::open(&path, 100).unwrap();
        assert!(kept.is_empty());
        log.append(b"hello\r\n").unwrap();
        log.append(b"world").unwrap();
        drop(log);
        let (mut log, kept) = OutputLog::open(&path, 100).unwrap();
        assert_eq!(kept, b"hello\r\nworld");
        log.append(b"!").unwrap();
        drop(log);
        assert_eq!(std::fs::read(&path).unwrap(), b"hello\r\nworld!");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compacts_to_the_cap_at_a_line_start() {
        let dir = tmp("compact");
        let path = dir.join("output.log");
        let (mut log, _) = OutputLog::open(&path, 20).unwrap();
        for i in 0..10 {
            log.append(format!("line {i}\n").as_bytes()).unwrap();
            let len = std::fs::metadata(&path).unwrap().len();
            assert!(len <= 30, "compacted at 1.5 x cap: {len}");
        }
        drop(log);
        let (_, kept) = OutputLog::open(&path, 20).unwrap();
        let text = String::from_utf8(kept).unwrap();
        assert!(text.len() <= 20, "{text:?}");
        assert!(text.starts_with("line "), "starts at a line: {text:?}");
        assert!(text.ends_with("line 9\n"), "{text:?}");
        // A bigger file than the cap (e.g. a smaller cap now) is cut on open.
        std::fs::write(&path, "é".repeat(30)).unwrap();
        let (_, kept) = OutputLog::open(&path, 20).unwrap();
        assert_eq!(kept.len(), 20, "no newline: cut as is");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
