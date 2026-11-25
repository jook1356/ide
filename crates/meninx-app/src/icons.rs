//! Tabler icons (MIT, `assets/icons/`, unmodified files from github.com/tabler/tabler-icons)
//! drawn with iced's `svg` widget. Drawn like `@tabler/icons-react` with `stroke={1.5}`: the
//! files' `stroke-width="2"` is replaced when they are loaded; the colour comes from the style.

use std::sync::LazyLock;

use iced::widget::svg;
use iced::{Color, Element, Widget as _};

/// Icons used by the navigation rail, the side panel and the detail window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Messages,
    Key,
    Stack2,
    GitBranch,
    Settings,
    SidebarCollapse,
    SidebarExpand,
    FolderOpen,
    ArrowsSort,
    Search,
    GitCommit,
    ArrowUp,
    ChevronRight,
    ChevronDown,
    // Changes / History tab actions (changes_view.rs).
    Check,
    Minus,
    Archive,
    ArrowBackUp,
    ArrowRight,
    PackageExport,
    CopyCheck,
    Trash,
    Cloud,
    Plus,
    Folder,
    Refresh,
    // Detail window (detail_window.rs).
    WindowMaximize,
    Pin,
    /// Tabler's filled pin (no stroke).
    PinFilled,
    ArrowBarToLeft,
    // Changes panel's 스크립트 section (scripts_view.rs).
    ChevronsDown,
    ChevronsUp,
    /// Tabler's filled player-play (no stroke).
    PlayerPlayFilled,
    // App footer (self_update.rs).
    Hammer,
    // Session list edit mode (session_edit.rs).
    ListCheck,
    Dots,
    // Notification centre (notice_view.rs).
    Bell,
    InfoCircle,
    CircleCheck,
    AlertTriangle,
    CircleX,
    X,
    Checks,
    // Widgets (widgets_ui.rs, widgets_page.rs).
    Puzzle,
    // Running work (notice_view.rs), turned by [`spinner`].
    Loader,
}

const SOURCES: [&str; 45] = [
    include_str!("../assets/icons/messages.svg"),
    include_str!("../assets/icons/key.svg"),
    include_str!("../assets/icons/stack-2.svg"),
    include_str!("../assets/icons/git-branch.svg"),
    include_str!("../assets/icons/settings.svg"),
    include_str!("../assets/icons/layout-sidebar-left-collapse.svg"),
    include_str!("../assets/icons/layout-sidebar-left-expand.svg"),
    include_str!("../assets/icons/folder-open.svg"),
    include_str!("../assets/icons/arrows-sort.svg"),
    include_str!("../assets/icons/search.svg"),
    include_str!("../assets/icons/git-commit.svg"),
    include_str!("../assets/icons/arrow-up.svg"),
    include_str!("../assets/icons/chevron-right.svg"),
    include_str!("../assets/icons/chevron-down.svg"),
    include_str!("../assets/icons/check.svg"),
    include_str!("../assets/icons/minus.svg"),
    include_str!("../assets/icons/archive.svg"),
    include_str!("../assets/icons/arrow-back-up.svg"),
    include_str!("../assets/icons/arrow-right.svg"),
    include_str!("../assets/icons/package-export.svg"),
    include_str!("../assets/icons/copy-check.svg"),
    include_str!("../assets/icons/trash.svg"),
    include_str!("../assets/icons/cloud.svg"),
    include_str!("../assets/icons/plus.svg"),
    include_str!("../assets/icons/folder.svg"),
    include_str!("../assets/icons/refresh.svg"),
    include_str!("../assets/icons/window-maximize.svg"),
    include_str!("../assets/icons/pin.svg"),
    include_str!("../assets/icons/pin-filled.svg"),
    include_str!("../assets/icons/arrow-bar-to-left.svg"),
    include_str!("../assets/icons/chevrons-down.svg"),
    include_str!("../assets/icons/chevrons-up.svg"),
    include_str!("../assets/icons/player-play-filled.svg"),
    include_str!("../assets/icons/hammer.svg"),
    include_str!("../assets/icons/list-check.svg"),
    include_str!("../assets/icons/dots.svg"),
    include_str!("../assets/icons/bell.svg"),
    include_str!("../assets/icons/info-circle.svg"),
    include_str!("../assets/icons/circle-check.svg"),
    include_str!("../assets/icons/alert-triangle.svg"),
    include_str!("../assets/icons/circle-x.svg"),
    include_str!("../assets/icons/x.svg"),
    include_str!("../assets/icons/checks.svg"),
    include_str!("../assets/icons/puzzle.svg"),
    include_str!("../assets/icons/loader-2.svg"),
];

/// Stroke width the icons are drawn with (in the 24-unit view box).
const STROKE: &str = "1.5";

/// History's unpushed mark: Desk draws its arrow with `stroke={2.5}`.
const STROKE_BOLD: &str = "2.5";

static HANDLES: LazyLock<Vec<svg::Handle>> = LazyLock::new(|| {
    SOURCES
        .iter()
        .enumerate()
        .map(|(i, s)| {
            // Checkbox marks are bold like Mantine's.
            let bold = [Icon::ArrowUp, Icon::Check, Icon::Minus].map(|x| x as usize);
            let w = if bold.contains(&i) { STROKE_BOLD } else { STROKE };
            let s = s.replace("stroke-width=\"2\"", &format!("stroke-width=\"{w}\""));
            svg::Handle::from_memory(s.into_bytes())
        })
        .collect()
});

fn handle(icon: Icon) -> svg::Handle {
    HANDLES[icon as usize].clone()
}

/// The icon at `size` logical pixels in `color`.
pub fn icon<'a, M: 'a>(icon: Icon, size: f32, color: Color) -> Element<'a, M> {
    svg(handle(icon))
        .width(size)
        .height(size)
        .style(move |_, _| svg::Style { color: Some(color) })
        .boxed()
}

/// Turns per second of [`spinner`].
const SPIN_HZ: f32 = 1.2;

/// How often a shown spinner is redrawn (the app's subscription while work runs).
pub const SPIN_FRAME: std::time::Duration = std::time::Duration::from_millis(80);

/// A turning loader at `size` in `color`. Its angle follows the clock, so it moves while
/// something redraws the view every [`SPIN_FRAME`].
pub fn spinner<'a, M: 'a>(size: f32, color: Color) -> Element<'a, M> {
    static START: LazyLock<std::time::Instant> = LazyLock::new(std::time::Instant::now);
    let turns = START.elapsed().as_secs_f32() * SPIN_HZ;
    let angle = turns.fract() * std::f32::consts::TAU;
    svg(handle(Icon::Loader))
        .width(size)
        .height(size)
        .rotation(iced::Radians(angle))
        .style(move |_, _| svg::Style { color: Some(color) })
        .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_tabler_outlines_with_thin_strokes() {
        for (i, s) in SOURCES.iter().enumerate() {
            assert!(s.contains("viewBox=\"0 0 24 24\""));
            if [Icon::PinFilled, Icon::PlayerPlayFilled].map(|x| x as usize).contains(&i) {
                assert!(s.contains("fill=\"currentColor\"") && !s.contains("stroke=\"currentColor\""));
                continue;
            }
            assert!(s.contains("stroke=\"currentColor\""));
            assert!(s.contains("stroke-width=\"2\""));
        }
        assert_eq!(HANDLES.len(), Icon::Loader as usize + 1);
    }
}
