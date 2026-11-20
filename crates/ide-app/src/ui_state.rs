//! UI state remembered across restarts (`<data dir>/ui.json`): selected session, split ratio,
//! detail collapsed, active configuration, per-session web URL, last repo folder, UI language,
//! which server to use (local / SSH), what closing the window does and each session's open
//! detail items.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ide_i18n::LanguagePref;
use serde::{Deserialize, Serialize};

/// Default share of the centre (terminal) in the centre+detail area.
pub const DEFAULT_SPLIT: f32 = 0.5;
/// Limits of the centre share while dragging the divider.
pub const SPLIT_RANGE: (f32, f32) = (0.2, 0.85);

/// What closing the window does with a local server's running sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CloseAction {
    /// Ask when sessions are running (default).
    #[default]
    Ask,
    /// Exit the app; the daemon keeps the sessions running.
    Keep,
    /// Stop every session and the daemon, then exit.
    Stop,
}

/// Which server the app connects to.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ServerPref {
    /// `ide-server --stdio` next to `ide.exe` (attaches to or starts the local daemon).
    #[default]
    Local,
    /// `ssh <host> <command>` (command empty = `ide-server --stdio`).
    Ssh {
        host: String,
        #[serde(default)]
        command: String,
    },
}

/// Open items of one session's detail area.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DetailUi {
    /// Open items in order: `web`, `file:<path>`, `diff:s:<path>` (staged) /
    /// `diff:w:<path>` (working tree).
    pub open: Vec<String>,
    /// The item shown.
    pub active: Option<String>,
    /// Side panel tab: `changes`, `explore` or `search`.
    pub side: String,
}

/// Deserialize a field, falling back to its default when the value is invalid (so one bad
/// field does not reset the whole file).
fn lenient<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    /// Selected session id.
    pub selected: Option<String>,
    /// Configuration whose sessions the sidebar shows (`profile` in older files: profiles
    /// were migrated to configurations with the same slug).
    #[serde(alias = "profile")]
    pub configuration: Option<String>,
    /// Centre share of the centre+detail width (0..1).
    pub split_ratio: f32,
    /// Detail area collapsed to a thin strip.
    pub detail_collapsed: bool,
    /// Last URL of each session's web tab, by session id.
    pub web_urls: BTreeMap<String, String>,
    /// Last repository folder used in the new-session dialog.
    pub last_repo: Option<String>,
    /// UI language (`"system"` or a language code); missing in older files = system.
    pub language: LanguagePref,
    /// Closing the window: ask / keep sessions running / stop them and the server.
    #[serde(deserialize_with = "lenient")]
    pub on_close: CloseAction,
    /// Server to connect to (`IDE_REMOTE` overrides it).
    #[serde(deserialize_with = "lenient")]
    pub server: ServerPref,
    /// Open detail items per session id.
    #[serde(deserialize_with = "lenient")]
    pub detail: BTreeMap<String, DetailUi>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            selected: None,
            configuration: None,
            split_ratio: DEFAULT_SPLIT,
            detail_collapsed: false,
            web_urls: BTreeMap::new(),
            last_repo: None,
            language: LanguagePref::System,
            on_close: CloseAction::Ask,
            server: ServerPref::Local,
            detail: BTreeMap::new(),
        }
    }
}

impl UiState {
    pub fn file(data_dir: &Path) -> PathBuf {
        data_dir.join("ui.json")
    }

    /// Load `ui.json`; a missing or unreadable file gives the defaults (logged).
    pub fn load(data_dir: &Path) -> Self {
        let path = Self::file(data_dir);
        let mut state: UiState = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!("ignoring invalid {}: {e}", path.display());
                UiState::default()
            }),
            Err(_) => UiState::default(),
        };
        if !state.split_ratio.is_finite() {
            state.split_ratio = DEFAULT_SPLIT;
        }
        state.split_ratio = state.split_ratio.clamp(SPLIT_RANGE.0, SPLIT_RANGE.1);
        state
    }

    /// Write `ui.json` atomically (temp file + rename). Errors are logged, not fatal.
    pub fn save(&self, data_dir: &Path) {
        let path = Self::file(data_dir);
        let tmp = data_dir.join("ui.json.tmp");
        let result = serde_json::to_vec_pretty(self)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                std::fs::create_dir_all(data_dir)?;
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(&tmp, &path)
            });
        if let Err(e) = result {
            tracing::warn!("saving {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let base = std::env::var_os("IDE_TEST_TMP")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("ide-app-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(UiState::load(&base), UiState::default());

        let mut s = UiState {
            selected: Some("abc".into()),
            split_ratio: 0.42,
            detail_collapsed: true,
            language: LanguagePref::Fixed(ide_i18n::Lang::Ko),
            on_close: CloseAction::Keep,
            server: ServerPref::Ssh {
                host: "me@devbox".into(),
                command: "/opt/ide/ide-server --stdio".into(),
            },
            ..Default::default()
        };
        s.web_urls.insert("abc".into(), "about:blank".into());
        s.detail.insert(
            "abc".into(),
            DetailUi {
                open: vec!["web".into(), "file:src/한글.ts".into()],
                active: Some("file:src/한글.ts".into()),
                side: "search".into(),
            },
        );
        s.save(&base);
        assert_eq!(UiState::load(&base), s);

        // Partial / older files fill in defaults; out-of-range ratios are clamped.
        std::fs::write(UiState::file(&base), r#"{"split_ratio": 5.0}"#).unwrap();
        let l = UiState::load(&base);
        assert_eq!(l.split_ratio, SPLIT_RANGE.1);
        assert!(l.web_urls.is_empty() && !l.detail_collapsed);
        assert_eq!(l.language, LanguagePref::System);
        std::fs::write(
            UiState::file(&base),
            r#"{"language": "xx", "split_ratio": 0.5}"#,
        )
        .unwrap();
        let l = UiState::load(&base);
        assert_eq!((l.language, l.split_ratio), (LanguagePref::System, 0.5));
        std::fs::write(UiState::file(&base), r#"{"language": "en"}"#).unwrap();
        assert_eq!(
            UiState::load(&base).language,
            LanguagePref::Fixed(ide_i18n::Lang::En)
        );

        // Invalid values of the new fields fall back to their defaults only.
        std::fs::write(
            UiState::file(&base),
            r#"{"on_close": "explode", "server": {"kind": "ftp"}, "split_ratio": 0.5}"#,
        )
        .unwrap();
        let l = UiState::load(&base);
        assert_eq!(
            (l.on_close, l.server, l.split_ratio),
            (CloseAction::Ask, ServerPref::Local, 0.5)
        );
        std::fs::write(
            UiState::file(&base),
            r#"{"on_close": "stop", "server": {"kind": "ssh", "host": "h"}}"#,
        )
        .unwrap();
        let l = UiState::load(&base);
        assert_eq!(l.on_close, CloseAction::Stop);
        assert_eq!(
            l.server,
            ServerPref::Ssh {
                host: "h".into(),
                command: String::new()
            }
        );

        std::fs::write(UiState::file(&base), "not json").unwrap();
        assert_eq!(UiState::load(&base), UiState::default());
        let _ = std::fs::remove_dir_all(&base);
    }
}
