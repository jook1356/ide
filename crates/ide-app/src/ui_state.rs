//! UI state remembered across restarts (`<data dir>/ui.json`): selected session, split ratio,
//! detail collapsed, active configuration, per-session web URL, last repo folder, UI language,
//! which server to use (local / SSH), what closing the window does, each session's open
//! detail items, the page chosen in the navigation rail and whether the session list is open.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ide_i18n::LanguagePref;
use serde::{Deserialize, Serialize};

use crate::session_sort::SortCriterion;

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

/// A page of the navigation rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Page {
    /// Session list, terminal and detail area.
    #[default]
    Work,
    Accounts,
    Configurations,
    /// Repository folders the new-session dialog can browse.
    Repos,
    /// Language, server and closing behaviour.
    Settings,
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
    /// Page shown next to the navigation rail.
    #[serde(deserialize_with = "lenient")]
    pub page: Page,
    /// The session list column of the work page is open.
    pub sessions_open: bool,
    /// The session list shows every configuration (Hermes Desk's 전체), grouped by configuration.
    pub sessions_all: bool,
    /// Session list sort criteria (empty = server order).
    #[serde(deserialize_with = "lenient")]
    pub session_sort: Vec<SortCriterion>,
    /// Folded groups of the session list: configuration slugs, or [`LOGIN_GROUP`].
    #[serde(deserialize_with = "lenient")]
    pub folded_groups: BTreeSet<String>,
    /// The detail area lives in a window of its own (Hermes Desk's `region-detached:detail`):
    /// kept while that window is hidden (its own close button, the detail toggle, quitting)
    /// and across restarts; cleared only by "앱으로 합치기".
    pub detail_detached: bool,
    /// The detail window is owned by the app window: it stays in front of it and minimises
    /// with it (Desk's `region-pinned:detail`).
    pub detail_pinned: bool,
    /// The detail window's last position and size (logical pixels).
    #[serde(deserialize_with = "lenient")]
    pub detail_window: Option<WindowGeom>,
    /// How 열린 항목 shows in the detail window (Desk's `detail-rail`).
    #[serde(deserialize_with = "lenient")]
    pub detail_rail: RailPrefs,
    /// Width of the side panel (Changes / History / Explore), set by dragging its left edge
    /// (Desk's `changes-panel-width`); clamped to the window when drawn.
    pub side_width: f32,
    /// Explore's 파일 찾기 leaves gitignored files and folders out (Desk's
    /// `explore-search-hide-ignored`, on by default).
    pub explore_hide_ignored: bool,
    /// Default shell of new terminals (☆ in "+ 새로 열기"; `None` = the server's first), and the
    /// last 공용 start folders.
    pub default_shell: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub recent_term_dirs: Vec<String>,
    /// Browser 즐겨찾기, shared by every session (Desk's `browser-bookmarks.json`).
    #[serde(deserialize_with = "lenient")]
    pub bookmarks: Vec<Bookmark>,
}

/// A browser bookmark.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
}

/// How 열린 항목 shows in the detail window (Hermes Desk's `lib/detailRail.ts`): a vertical tab
/// list beside the content or the header's popover. The app window's pane always uses the
/// popover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RailPrefs {
    pub mode: RailMode,
    pub side: RailSide,
    /// The list stays open beside the content (else an icon rail that opens on hover).
    pub pinned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RailMode {
    #[default]
    Tabs,
    Popover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RailSide {
    #[default]
    Left,
    Right,
}

/// Position and size of a window, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct WindowGeom {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Key of the login-terminal group in [`UiState::folded_groups`] (not a valid slug).
pub const LOGIN_GROUP: &str = ":login";

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
            page: Page::Work,
            sessions_open: true,
            sessions_all: false,
            session_sort: Vec::new(),
            folded_groups: BTreeSet::new(),
            detail_detached: false,
            detail_pinned: false,
            detail_window: None,
            detail_rail: RailPrefs::default(),
            side_width: crate::detail::SIDE_W,
            explore_hide_ignored: true,
            bookmarks: Vec::new(),
            default_shell: None,
            recent_term_dirs: Vec::new(),
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
        state.session_sort = crate::session_sort::normalized(&state.session_sort);
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
            page: Page::Repos,
            sessions_open: false,
            sessions_all: true,
            session_sort: vec![crate::session_sort::SortCriterion {
                key: crate::session_sort::SortKey::Updated,
                dir: crate::session_sort::SortDir::Desc,
            }],
            folded_groups: [LOGIN_GROUP.to_owned()].into(),
            detail_detached: true,
            detail_pinned: true,
            detail_window: Some(WindowGeom {
                x: 120.0,
                y: -40.0,
                width: 900.0,
                height: 640.0,
            }),
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
        assert_eq!((l.page, l.sessions_open), (Page::Work, true));
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
            r#"{"on_close": "explode", "server": {"kind": "ftp"}, "page": "nowhere", "split_ratio": 0.5}"#,
        )
        .unwrap();
        let l = UiState::load(&base);
        assert_eq!(
            (l.on_close, l.server, l.page, l.split_ratio),
            (CloseAction::Ask, ServerPref::Local, Page::Work, 0.5)
        );
        std::fs::write(
            UiState::file(&base),
            r#"{"page": "settings", "sessions_open": false}"#,
        )
        .unwrap();
        let l = UiState::load(&base);
        assert_eq!((l.page, l.sessions_open), (Page::Settings, false));
        // A broken window geometry is dropped; the detached / pin flags stay.
        std::fs::write(
            UiState::file(&base),
            r#"{"detail_detached": true, "detail_pinned": true, "detail_window": {"x": "a"}}"#,
        )
        .unwrap();
        let l = UiState::load(&base);
        assert_eq!(
            (l.detail_detached, l.detail_pinned, l.detail_window),
            (true, true, None)
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
