//! Detection: is the program installed, which version, is it new enough.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{AgentProvider, HostEnv};

/// How long `--version` / status commands may take.
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Result of [`AgentProvider::detect`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Detection {
    /// Resolved program path (`None` = not installed / not on `PATH`).
    pub path: Option<PathBuf>,
    /// Version reported by the program (`None` = unknown).
    pub version: Option<String>,
    /// Oldest supported version.
    pub min_version: Option<String>,
    /// `version >= min_version` (true when there is no minimum; false when the version is
    /// unknown but a minimum exists).
    pub version_ok: bool,
    /// Why it is not usable (English, for logs/tooltips).
    pub error: Option<String>,
}

impl Detection {
    /// Installed and new enough.
    pub fn usable(&self) -> bool {
        self.path.is_some() && self.version_ok
    }
}

pub(crate) fn detect<P: AgentProvider + ?Sized>(p: &P, env: &HostEnv) -> Detection {
    let min_version = p.min_version().map(str::to_owned);
    let path = match p.program_path(env) {
        Ok(path) => path,
        Err(e) => {
            return Detection {
                min_version,
                error: Some(e.to_string()),
                ..Default::default()
            };
        }
    };
    let args = p.version_args();
    let (version, error) = if args.is_empty() {
        (None, None)
    } else {
        match run_capture(&path, &args, &[], env, PROBE_TIMEOUT) {
            Ok(out) => match parse_version(&out.stdout).or_else(|| parse_version(&out.stderr)) {
                Some(v) => (Some(v), None),
                None => (None, Some(format!("no version in {:?}", out.stdout.trim()))),
            },
            Err(e) => (None, Some(e)),
        }
    };
    let version_ok = match (&min_version, &version) {
        (None, _) => true,
        (Some(min), Some(v)) => version_at_least(v, min),
        (Some(_), None) => false,
    };
    let error = error.or_else(|| {
        (!version_ok).then(|| {
            format!(
                "version {} is older than {}",
                version.as_deref().unwrap_or("?"),
                min_version.as_deref().unwrap_or("?")
            )
        })
    });
    Detection {
        path: Some(path),
        version,
        min_version,
        version_ok,
        error: if version_ok { None } else { error },
    }
}

/// The first `N.N[.N...]` in `text` (e.g. `"2.1.329 (Claude Code)"`, `"codex-cli 0.160.1"`).
pub fn parse_version(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let boundary = |j: usize| j == 0 || !b[j - 1].is_ascii_alphanumeric();
        let v_prefix = i >= 1 && (b[i - 1] == b'v' || b[i - 1] == b'V') && boundary(i - 1);
        if b[i].is_ascii_digit() && (boundary(i) || v_prefix) {
            let start = i;
            let mut dots = 0;
            while i < b.len()
                && (b[i].is_ascii_digit()
                    || (b[i] == b'.' && i + 1 < b.len() && b[i + 1].is_ascii_digit()))
            {
                if b[i] == b'.' {
                    dots += 1;
                }
                i += 1;
            }
            if dots >= 1 {
                return Some(text[start..i].to_owned());
            }
        }
        i += 1;
    }
    None
}

/// Numeric comparison of dotted versions (missing parts count as 0).
pub fn version_at_least(version: &str, min: &str) -> bool {
    let parts = |s: &str| -> Vec<u64> {
        s.split('.')
            .map(|p| {
                p.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    };
    let (v, m) = (parts(version), parts(min));
    for i in 0..v.len().max(m.len()) {
        let (a, b) = (
            v.get(i).copied().unwrap_or(0),
            m.get(i).copied().unwrap_or(0),
        );
        if a != b {
            return a > b;
        }
    }
    true
}

pub(crate) struct Captured {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Run a program without a console window, with extra env vars, and capture its output.
/// Killed after `timeout`.
pub(crate) fn run_capture(
    program: &Path,
    args: &[String],
    vars: &[(String, String)],
    env: &HostEnv,
    timeout: Duration,
) -> Result<Captured, String> {
    let (program, args) = ide_core::wrap_batch_file(program.to_path_buf(), args.to_vec(), env)
        .map_err(|e| e.to_string())?;
    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .envs(vars.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("{}: {e}", program.display()))?;
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let read_out = std::thread::spawn(move || read_all(out.as_mut()));
    let read_err = std::thread::spawn(move || read_all(err.as_mut()));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.code(),
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{} timed out", program.display()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    };
    Ok(Captured {
        status,
        stdout: read_out.join().unwrap_or_default(),
        stderr: read_err.join().unwrap_or_default(),
    })
}

fn read_all<R: std::io::Read>(r: Option<&mut R>) -> String {
    let mut buf = Vec::new();
    if let Some(r) = r {
        let _ = r.read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(
            parse_version("2.1.329 (Claude Code)").as_deref(),
            Some("2.1.329")
        );
        assert_eq!(
            parse_version("codex-cli 0.160.1\n").as_deref(),
            Some("0.160.1")
        );
        assert_eq!(parse_version("node v20.1.0").as_deref(), Some("20.1.0"));
        assert_eq!(
            parse_version("abc1.2 x 3.4").as_deref(),
            Some("3.4"),
            "abc1.2: inside a word"
        );
        assert_eq!(parse_version("tool 1.2-beta").as_deref(), Some("1.2"));
        assert_eq!(parse_version("build 7 then 3.4.").as_deref(), Some("3.4"));
        assert_eq!(parse_version("nothing"), None);
        assert!(version_at_least("2.1.329", "2.1.318"));
        assert!(version_at_least("2.1.318", "2.1.318"));
        assert!(!version_at_least("2.1.9", "2.1.318"));
        assert!(version_at_least("0.160.1", "0.160"));
        assert!(!version_at_least("0.99.0", "0.160.0"));
        assert!(version_at_least("1.0", "0.999.999"));
    }
}
