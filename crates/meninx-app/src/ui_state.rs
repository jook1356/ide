//! UI state remembered across restarts (`<data dir>/ui.json`): selected session, split ratio,
//! detail collapsed, active configuration, per-session web URL, last repo folder, UI language,
//! which server to use (local / SSH), the UI / code / terminal fonts, what closing the window does, each session's open
//! detail items, the page chosen in the navigation rail and whether the session list is open.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use meninx_i18n::LanguagePref;
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

/// Font families picked in Settings (blank = the default). The UI font applies at the next
/// start; the code and terminal fonts at once.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FontPrefs {
    /// UI text (iced's default font).
    pub ui: String,
    /// Editor, diff, hashes and paths.
    pub code: String,
    /// Terminals.
    pub terminal: String,
}

/// Which font of [`FontPrefs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontSlot {
    Ui,
    Code,
    Terminal,
}

impl FontPrefs {
    pub fn get(&self, slot: FontSlot) -> &str {
        match slot {
            FontSlot::Ui => &self.ui,
            FontSlot::Code => &self.code,
            FontSlot::Terminal => &self.terminal,
        }
    }

    pub fn set(&mut self, slot: FontSlot, name: &str) {
        let v = name.trim().to_owned();
        match slot {
            FontSlot::Ui => self.ui = v,
            FontSlot::Code => self.code = v,
            FontSlot::Terminal => self.terminal = v,
        }
    }
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
    /// Registered widgets (Desk's 모드 page, `widgets_page.rs`).
    Widgets,
    /// Language, server and closing behaviour.
    Settings,
}

/// Id of Local in [`UiState::current_server`] (never an entry's id).
pub const LOCAL_SERVER: &str = "local";

/// A server of the list besides Local (`docs/multi-server.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerEntry {
    /// Stable id: also the folder of its UI state (`<data>/servers/<id>/ui.json`).
    pub id: String,
    pub name: String,
    /// How to reach it (never `Local`).
    #[serde(flatten)]
    pub pref: ServerPref,
    /// Connected at start and kept connected while another server is shown (else when picked).
    #[serde(default = "yes")]
    pub auto_connect: bool,
}

fn yes() -> bool {
    true
}

/// A new entry id from `base` (a name or host): a lowercase slug, unique among `taken`.
pub fn new_server_id(base: &str, taken: &[ServerEntry]) -> String {
    let mut slug = String::new();
    for c in base.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(32).collect();
    let slug = if slug.is_empty() { "server".to_owned() } else { slug };
    let free = |s: &str| s != LOCAL_SERVER && !taken.iter().any(|e| e.id == s);
    if free(&slug) {
        return slug;
    }
    (2..).map(|n| format!("{slug}-{n}")).find(|s| free(s)).unwrap_or(slug)
}

/// Which server the app connects to.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ServerPref {
    /// `meninx-server --stdio` next to `meninx.exe` (attaches to or starts the local daemon).
    #[default]
    Local,
    /// `ssh <host> <command>` (command empty = `meninx-server --stdio`).
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
    /// The input box under agent terminals (`composer.rs`).
    pub composer_open: bool,
    /// Last repository folder used in the new-session dialog.
    pub last_repo: Option<String>,
    /// UI language (`"system"` or a language code); missing in older files = system.
    pub language: LanguagePref,
    /// Closing the window: ask / keep sessions running / stop them and the server.
    #[serde(deserialize_with = "lenient")]
    pub on_close: CloseAction,
    /// The one server of older files: read once, moved into `servers` (never written).
    #[serde(deserialize_with = "lenient", skip_serializing)]
    pub server: ServerPref,
    /// Servers besides Local (`docs/multi-server.md`; `MENINX_REMOTE` replaces the list).
    #[serde(deserialize_with = "lenient")]
    pub servers: Vec<ServerEntry>,
    /// The shown server: [`LOCAL_SERVER`] or an id of `servers`.
    pub current_server: String,
    /// Fonts picked in Settings.
    #[serde(deserialize_with = "lenient")]
    pub fonts: FontPrefs,
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
    /// Session list groups whose 보관함 (archived sessions) is unfolded; same keys as
    /// `folded_groups`, "" for the single block of one configuration.
    #[serde(deserialize_with = "lenient")]
    pub archive_open: BTreeSet<String>,
    /// Parent sessions whose children are folded in the session list.
    #[serde(deserialize_with = "lenient")]
    pub folded_families: BTreeSet<String>,
    /// The self-update run whose result was dismissed (`self_update.rs`).
    pub update_seen: Option<String>,
    /// The detail area lives in a window of its own (Hermes Desk's `region-detached:detail`):
    /// kept while that window is hidden (its own close button, the detail toggle, quitting)
    /// and across restarts; cleared only by "앱으로 합치기".
    pub detail_detached: bool,
    /// The detail window is owned by the app window: it stays in front of it and minimises
    /// with it (Desk's `region-pinned:detail`).
    pub detail_pinned: bool,
    /// The detail window's last position and size (logical pixels; on Windows its restored
    /// rectangle in physical pixels, see [`WindowGeom::dpi`]).
    #[serde(deserialize_with = "lenient")]
    pub detail_window: Option<WindowGeom>,
    /// How 열린 항목 shows in the detail window (Desk's `detail-rail`).
    #[serde(deserialize_with = "lenient")]
    pub detail_rail: RailPrefs,
    /// Width of the side panel (Changes / History / Explore), set by dragging its left edge
    /// (Desk's `changes-panel-width`); clamped to the window when drawn.
    pub side_width: f32,
    /// Width of the session / profile list, set by dragging its right edge; clamped to the
    /// window when drawn.
    pub list_width: f32,
    /// The Changes panel's 스크립트 section is folded, and its height (Desk's
    /// `scripts-collapsed` / `scripts-height`).
    pub scripts_collapsed: bool,
    pub scripts_height: f32,
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

/// Position and size of a window: logical pixels, or with `dpi` (Windows) the restored outer
/// rectangle in physical pixels on a monitor of that DPI (`meninx_web::Placement`), which does not
/// depend on the scale the window had when it was saved.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct WindowGeom {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// The monitor's DPI when saved: the rectangle is in physical pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dpi: Option<u32>,
    /// The window was maximised (`x` .. `height` are then its restored rectangle).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub maximized: bool,
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
            composer_open: true,
            last_repo: None,
            language: LanguagePref::System,
            on_close: CloseAction::Ask,
            server: ServerPref::Local,
            servers: Vec::new(),
            current_server: LOCAL_SERVER.to_owned(),
            fonts: FontPrefs::default(),
            detail: BTreeMap::new(),
            page: Page::Work,
            sessions_open: true,
            sessions_all: false,
            session_sort: Vec::new(),
            folded_groups: BTreeSet::new(),
            archive_open: BTreeSet::new(),
            folded_families: BTreeSet::new(),
            update_seen: None,
            detail_detached: false,
            detail_pinned: false,
            detail_window: None,
            detail_rail: RailPrefs::default(),
            side_width: crate::detail::SIDE_W,
            list_width: crate::app::SIDEBAR_W,
            scripts_collapsed: false,
            scripts_height: crate::scripts_view::DEFAULT_H,
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

    /// The UI state file of server `id`: Local's is `ui.json` (also the record of the globals),
    /// another server's `servers/<id>/ui.json`.
    pub fn server_file(data_dir: &Path, id: &str) -> PathBuf {
        if id == LOCAL_SERVER {
            Self::file(data_dir)
        } else {
            data_dir.join("servers").join(id).join("ui.json")
        }
    }

    /// Load `ui.json`; a missing or unreadable file gives the defaults (logged).
    pub fn load(data_dir: &Path) -> Self {
        Self::load_from(&Self::file(data_dir))
    }

    /// Load a UI state file (see [`UiState::load`]).
    pub fn load_from(path: &Path) -> Self {
        let mut state: UiState = match std::fs::read(path) {
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
        state.migrate_servers();
        state
    }

    /// The one server of an older file becomes the list's entry, shown; entries that are
    /// Local, without an id or repeated are dropped; an unknown current server is Local.
    fn migrate_servers(&mut self) {
        if let ServerPref::Ssh { host, command } = std::mem::take(&mut self.server)
            && self.servers.is_empty()
        {
            let id = new_server_id(&host, &[]);
            self.servers.push(ServerEntry {
                id: id.clone(),
                name: host.clone(),
                pref: ServerPref::Ssh { host, command },
                auto_connect: true,
            });
            self.current_server = id;
        }
        let mut seen = BTreeSet::new();
        self.servers.retain(|e| {
            !e.id.is_empty()
                && e.id != LOCAL_SERVER
                && e.pref != ServerPref::Local
                && seen.insert(e.id.clone())
        });
        if !self.servers.iter().any(|e| e.id == self.current_server) {
            self.current_server = LOCAL_SERVER.to_owned();
        }
    }

    /// Write a UI state file atomically (temp file + rename). Errors are logged, not fatal.
    pub fn save_to(&self, path: &Path) {
        let tmp = path.with_extension("json.tmp");
        let result = serde_json::to_vec_pretty(self)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(&tmp, path)
            });
        if let Err(e) = result {
            tracing::warn!("saving {}: {e}", path.display());
        }
    }
}

/// The PC-wide fields: every server's `App` has the same values (the shell copies a change
/// to the others, `shell.rs`). The rest is per server: `selected`, `configuration`,
/// `last_repo`, `detail`, `folded_groups`, `archive_open`, `folded_families`,
/// `default_shell`, `recent_term_dirs` (and the old `server`).
macro_rules! globals {
    ($($f:ident),* $(,)?) => {
        impl UiState {
            /// Copy the PC-wide fields from `o`.
            pub fn copy_globals(&mut self, o: &UiState) {
                $(self.$f = o.$f.clone();)*
            }

            /// The PC-wide fields are the same as `o`'s.
            pub fn globals_eq(&self, o: &UiState) -> bool {
                true $(&& self.$f == o.$f)*
            }
        }
    };
}

globals!(
    split_ratio,
    detail_collapsed,
    composer_open,
    language,
    on_close,
    fonts,
    page,
    sessions_open,
    sessions_all,
    session_sort,
    update_seen,
    detail_detached,
    detail_pinned,
    detail_window,
    detail_rail,
    side_width,
    list_width,
    scripts_collapsed,
    scripts_height,
    explore_hide_ignored,
    bookmarks,
    servers,
    current_server,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers() {
        let e = |id: &str| ServerEntry {
            id: id.into(),
            name: id.into(),
            pref: ServerPref::Ssh {
                host: id.into(),
                command: String::new(),
            },
            auto_connect: true,
        };
        assert_eq!(new_server_id("me@Dev Box", &[]), "me-dev-box");
        assert_eq!(new_server_id("local", &[]), "local-2");
        assert_eq!(new_server_id("한글", &[]), "server");
        assert_eq!(new_server_id("a", &[e("a"), e("a-2")]), "a-3");

        // An older single server becomes the shown entry.
        let mut s: UiState =
            serde_json::from_str(r#"{"server": {"kind": "ssh", "host": "me@h", "command": "x"}}"#)
                .unwrap();
        s.migrate_servers();
        assert_eq!(s.current_server, "me-h");
        assert_eq!(
            s.servers,
            [ServerEntry {
                id: "me-h".into(),
                name: "me@h".into(),
                pref: ServerPref::Ssh {
                    host: "me@h".into(),
                    command: "x".into()
                },
                auto_connect: true,
            }]
        );
        let json = serde_json::to_value(&s).unwrap();
        assert!(json.get("server").is_none());
        assert_eq!(json["servers"][0]["kind"], "ssh");

        // Bad entries are dropped; an unknown current server is Local.
        let mut s: UiState = serde_json::from_str(
            r#"{"servers": [{"id": "a", "name": "A", "kind": "ssh", "host": "h"},
                            {"id": "a", "name": "again", "kind": "ssh", "host": "h2"},
                            {"id": "local", "name": "L", "kind": "ssh", "host": "h3"},
                            {"id": "b", "name": "B", "kind": "local"}],
                "current_server": "zz"}"#,
        )
        .unwrap();
        s.migrate_servers();
        assert_eq!(s.servers.len(), 1);
        assert_eq!((s.servers[0].name.as_str(), s.servers[0].auto_connect), ("A", true));
        assert_eq!(s.current_server, LOCAL_SERVER);

        // Globals copy over; per-server fields stay.
        let mut a = UiState {
            selected: Some("x".into()),
            ..UiState::default()
        };
        let b = UiState {
            selected: Some("y".into()),
            list_width: 333.0,
            servers: vec![e("q")],
            ..UiState::default()
        };
        assert!(!a.globals_eq(&b));
        a.copy_globals(&b);
        assert!(a.globals_eq(&b));
        assert_eq!((a.selected.as_deref(), a.list_width), (Some("x"), 333.0));
    }

    #[test]
    fn round_trip_and_defaults() {
        let base = std::env::var_os("MENINX_TEST_TMP")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("meninx-app-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(UiState::load(&base), UiState::default());

        let mut s = UiState {
            selected: Some("abc".into()),
            split_ratio: 0.42,
            detail_collapsed: true,
            language: LanguagePref::Fixed(meninx_i18n::Lang::Ko),
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
                dpi: Some(144),
                maximized: true,
            }),
            servers: vec![ServerEntry {
                id: "devbox".into(),
                name: "Dev box".into(),
                pref: ServerPref::Ssh {
                    host: "me@devbox".into(),
                    command: "/opt/meninx/meninx-server --stdio".into(),
                },
                auto_connect: false,
            }],
            current_server: "devbox".into(),
            fonts: FontPrefs {
                ui: "Pretendard".into(),
                code: "D2Coding".into(),
                terminal: String::new(),
            },
            ..Default::default()
        };
        s.detail.insert(
            "abc".into(),
            DetailUi {
                open: vec!["file:src/한글.ts".into()],
                active: Some("file:src/한글.ts".into()),
                side: "search".into(),
            },
        );
        s.save_to(&UiState::file(&base));
        assert_eq!(UiState::load(&base), s);

        // Partial / older files fill in defaults; out-of-range ratios are clamped.
        std::fs::write(UiState::file(&base), r#"{"split_ratio": 5.0}"#).unwrap();
        let l = UiState::load(&base);
        assert_eq!(l.split_ratio, SPLIT_RANGE.1);
        assert!(!l.detail_collapsed);
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
            LanguagePref::Fixed(meninx_i18n::Lang::En)
        );

        // Invalid values of the new fields fall back to their defaults only.
        std::fs::write(
            UiState::file(&base),
            r#"{"on_close": "explode", "server": {"kind": "ftp"}, "page": "nowhere", "split_ratio": 0.5}"#,
        )
        .unwrap();
        let l = UiState::load(&base);
        assert_eq!(
            (l.on_close, l.servers.is_empty(), l.page, l.split_ratio),
            (CloseAction::Ask, true, Page::Work, 0.5)
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
        assert_eq!((l.servers.len(), l.current_server.as_str()), (1, "h"));

        std::fs::write(UiState::file(&base), "not json").unwrap();
        assert_eq!(UiState::load(&base), UiState::default());
        let _ = std::fs::remove_dir_all(&base);
    }
}
