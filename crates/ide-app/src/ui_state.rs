//! UI state remembered across restarts (`<data dir>/ui.json`): selected session, split ratio,
//! detail collapsed, active profile, per-session web URL, last repo folder.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Default share of the centre (terminal) in the centre+detail area.
pub const DEFAULT_SPLIT: f32 = 0.6;
/// Limits of the centre share while dragging the divider.
pub const SPLIT_RANGE: (f32, f32) = (0.2, 0.85);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    /// Selected session id.
    pub selected: Option<String>,
    /// Profile whose sessions the sidebar shows.
    pub profile: Option<String>,
    /// Centre share of the centre+detail width (0..1).
    pub split_ratio: f32,
    /// Detail area collapsed to a thin strip.
    pub detail_collapsed: bool,
    /// Last URL of each session's web tab, by session id.
    pub web_urls: BTreeMap<String, String>,
    /// Last repository folder used in the new-session dialog.
    pub last_repo: Option<String>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            selected: None,
            profile: None,
            split_ratio: DEFAULT_SPLIT,
            detail_collapsed: false,
            web_urls: BTreeMap::new(),
            last_repo: None,
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
            ..Default::default()
        };
        s.web_urls.insert("abc".into(), "about:blank".into());
        s.save(&base);
        assert_eq!(UiState::load(&base), s);

        // Partial / older files fill in defaults; out-of-range ratios are clamped.
        std::fs::write(UiState::file(&base), r#"{"split_ratio": 5.0}"#).unwrap();
        let l = UiState::load(&base);
        assert_eq!(l.split_ratio, SPLIT_RANGE.1);
        assert!(l.web_urls.is_empty() && !l.detail_collapsed);

        std::fs::write(UiState::file(&base), "not json").unwrap();
        assert_eq!(UiState::load(&base), UiState::default());
        let _ = std::fs::remove_dir_all(&base);
    }
}
