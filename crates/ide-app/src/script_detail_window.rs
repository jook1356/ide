//! App script steps for the detail window (`detail_window.rs`, Hermes Desk's region window):
//! 새 창으로 분리 (the terminal takes the room), the window follows the selected session (web
//! tab, file, diff), the detail toggle and the window's own close button hide it while it stays
//! detached, the pin makes it an owned window of the app window, detached / pinned / size come
//! back after a restart, 앱으로 합치기 puts the area back. English `pop_*` and Korean `ko_pop_*`
//! screenshots of both windows. A child module of `script.rs`: it uses the script's helpers.

use std::sync::Mutex;

use super::*;
use crate::detail_window::WinMsg;
use crate::ui_state::UiState;

/// Terminal columns of the selected session at [`PStep::MarkCols`].
static COLS: Mutex<u16> = Mutex::new(0);

#[derive(Debug, Clone)]
pub(super) enum PStep {
    /// Open a file of the phase 2 session.
    Open(&'static str),
    /// The detail window is open (and created) or closed.
    WaitWindow(bool, u64),
    /// `ui.json` stores (detached, collapsed, pinned).
    ExpectSaved(bool, bool, bool),
    /// The detail pane is drawn in the app window (`true`) or not.
    ExpectInApp(bool),
    /// Remember the selected session's terminal width (columns).
    MarkCols,
    /// The selected session's terminal is at least `n` columns wider than at `MarkCols`.
    WaitWider(u16, u64),
    /// Screenshot of the detail window (follow it with a `Wait`).
    Shot(&'static str),
    /// The last screenshot (of the detail window) shows the script's web page below the bars.
    ExpectWebDrawn,
    /// The detail window's title contains the text.
    ExpectTitle(&'static str),
    /// The detail window's own close button.
    CloseButton,
    /// The detail window's owner is the app window (`true`) or none.
    WaitOwner(bool, u64),
    /// Move and resize the detail window (iced window tasks, no OS input).
    Place(f32, f32, f32, f32),
    /// The detail window has this position and size (logical px, ±2).
    WaitPlace(f32, f32, f32, f32, u64),
    /// Like closing the app / starting it again (state from `ui.json` + the server).
    CloseApp,
    ReopenApp,
}

fn p(x: PStep) -> Step {
    Step::Pop(x)
}

/// Run the detail-window steps after the phase 2 / footer passes (before the server stop).
pub(super) fn splice(steps: &mut Vec<Step>) {
    let at = steps.iter().position(
        |s| matches!(s, Step::Log(t) if t.starts_with("--- stop the server from the settings page")),
    );
    if let Some(at) = at {
        steps.splice(at..at, self::steps());
    }
}

/// `IDE_APP_SCRIPT_ONLY=pop`: startup, a session with the script's web page, these steps and
/// the server stop.
pub(super) fn steps_only() -> Vec<Step> {
    use Step::*;
    let mut s = vec![
        Log("--- startup (detail window only)"),
        SetLang(LanguagePref::Fixed(Lang::En)),
        WaitReady(30_000),
        MarkDaemon,
        EnsureRepo,
        GoPage(Page::Repos),
        RegisterTyped(Repo::Test),
        WaitRepoRegistered(Repo::Test, 15_000),
        GoPage(Page::Work),
        FillNew {
            repo: Repo::Test,
            provider: "shell",
            title: "shell A",
        },
        Submit,
        WaitCreated(30_000),
        WaitText(0, ">", 15_000),
        Navigate(data_url(PAGE)),
        WaitWebTitle("ide detail tab", 15_000),
    ];
    s.extend(steps());
    s.extend([
        GoPage(Page::Settings),
        AskStopServer,
        ConfirmStopServer,
        WaitDisconnected(15_000),
        WaitDaemonGone(10_000),
        Exit,
    ]);
    s
}

fn steps() -> Vec<Step> {
    use PStep::*;
    use Step::{
        App as A, AskDelete, ConfirmDelete, ExpectLang, FillNew, Log, MarkP2, OpenChange, Pause,
        Select, SetLang, Submit, Wait, WaitChange, WaitCreated, WaitDeleted, WaitDiff, WaitFile,
        WaitReady, WaitText, WaitWebTitle, WriteP2,
    };
    let main = "src/main.ts";
    let toggle = || A(Message::ToggleDetail);
    let win = |m: WinMsg| A(Message::DetailWindow(m));
    vec![
        Log("--- detail window: 새 창으로 분리 / 앱으로 합치기 / pin (Desk's region window)"),
        SetLang(LanguagePref::Fixed(Lang::En)),
        FillNew {
            repo: Repo::Test,
            provider: "shell",
            title: "detail window",
        },
        Submit,
        WaitCreated(30_000),
        MarkP2,
        WaitText(P2, ">", 15_000),
        p(Open(main)),
        WaitFile(main, 10_000),
        Step::CdRepoP2,
        Wait(300),
        WriteP2("echo // detail window>> src\\main.ts\r"),
        WaitChange(main, 15_000),
        Select(0),
        WaitWebTitle("ide detail tab", 15_000),
        Wait(800),
        p(ExpectInApp(true)),
        Step::Shot("pop_01_attached"),
        Log("--- 새 창으로 분리: the detail area moves into its own window, the terminal widens"),
        p(MarkCols),
        win(WinMsg::Popout),
        p(WaitWindow(true, 10_000)),
        p(ExpectSaved(true, false, false)),
        p(ExpectInApp(false)),
        p(WaitWider(20, 5000)),
        p(ExpectTitle("Detail · shell A")),
        WaitWebTitle("ide detail tab", 15_000),
        // CEF draws the page at the window's size (a few frames).
        Wait(1500),
        Step::Shot("pop_02_main_detached"),
        p(Shot("pop_03_window_web")),
        Wait(200),
        p(ExpectWebDrawn),
        Pause("pop_window_web"),
        Log("--- the window follows the selected session: a file, a diff"),
        Select(P2),
        p(ExpectTitle("Detail · detail window")),
        Step::D(DetailMsg::Activate(ItemKey::File(main.into()))),
        WaitFile(main, 10_000),
        Wait(600),
        p(Shot("pop_04_window_file")),
        Wait(200),
        Pause("pop_window_file"),
        OpenChange(ChangeGroup::Unstaged, main),
        WaitDiff(10_000),
        Wait(600),
        p(Shot("pop_05_window_diff")),
        Wait(200),
        Step::Shot("pop_06_main_side_panel"),
        Log("--- the detail toggle hides / shows the window; the area stays detached"),
        toggle(),
        p(WaitWindow(false, 5000)),
        p(ExpectSaved(true, true, false)),
        p(ExpectInApp(false)),
        Wait(300),
        Step::Shot("pop_07_hidden"),
        toggle(),
        p(WaitWindow(true, 10_000)),
        p(ExpectSaved(true, false, false)),
        Log("--- the window's own close button hides it too"),
        p(CloseButton),
        p(WaitWindow(false, 5000)),
        p(ExpectSaved(true, true, false)),
        toggle(),
        p(WaitWindow(true, 10_000)),
        Log("--- pin: the window becomes owned by the app window (Electron's setParentWindow)"),
        win(WinMsg::TogglePin),
        p(WaitOwner(true, 5000)),
        p(ExpectSaved(true, false, true)),
        Wait(400),
        p(Shot("pop_08_window_pinned")),
        Wait(200),
        win(WinMsg::TogglePin),
        p(WaitOwner(false, 5000)),
        p(ExpectSaved(true, false, false)),
        win(WinMsg::TogglePin),
        p(WaitOwner(true, 5000)),
        Log("--- restart: detached, pinned and the window's size come back"),
        p(Place(160.0, 120.0, 820.0, 600.0)),
        p(WaitPlace(160.0, 120.0, 820.0, 600.0, 5000)),
        p(CloseApp),
        p(WaitWindow(false, 5000)),
        Wait(1500),
        p(ReopenApp),
        WaitReady(30_000),
        p(WaitWindow(true, 15_000)),
        p(WaitPlace(160.0, 120.0, 820.0, 600.0, 5000)),
        p(WaitOwner(true, 5000)),
        p(ExpectSaved(true, false, true)),
        Log("--- Korean pass"),
        SetLang(LanguagePref::Fixed(Lang::Ko)),
        ExpectLang(Lang::Ko, "새 세션"),
        Select(0),
        WaitWebTitle("ide detail tab", 15_000),
        p(ExpectTitle("상세 · shell A")),
        Wait(1500),
        Step::Shot("ko_pop_01_main_detached"),
        p(Shot("ko_pop_02_window_web")),
        Wait(200),
        p(ExpectWebDrawn),
        Pause("ko_pop_window_web"),
        Select(P2),
        WaitDiff(10_000),
        Wait(600),
        p(Shot("ko_pop_03_window_diff")),
        Wait(200),
        Pause("ko_pop_window_diff"),
        toggle(),
        p(WaitWindow(false, 5000)),
        Wait(300),
        Step::Shot("ko_pop_04_main_hidden"),
        toggle(),
        p(WaitWindow(true, 10_000)),
        Log("--- 앱으로 합치기: the area comes back into the app window"),
        win(WinMsg::Merge),
        p(WaitWindow(false, 5000)),
        p(ExpectSaved(false, false, true)),
        p(ExpectInApp(true)),
        Wait(800),
        Step::Shot("ko_pop_05_merged"),
        SetLang(LanguagePref::Fixed(Lang::En)),
        Wait(300),
        Step::Shot("pop_09_merged"),
        AskDelete(P2),
        ConfirmDelete,
        WaitDeleted(P2, 20_000),
        Select(0),
    ]
}

/// Mean (r, g, b) of the web area of a detail-window screenshot (below the window's bar and
/// the detail header).
fn web_rgb(app: &App, shot: &window::Screenshot) -> (f32, f32, f32) {
    let (w, h) = (shot.size.width as usize, shot.size.height as usize);
    let scale = shot.size.width as f32 / app.detail_window_size.width.max(1.0);
    let top = ((36.0 + 1.0 + 40.0 + 1.0 + 20.0) * scale) as usize;
    let bottom = h.saturating_sub((20.0 * scale) as usize);
    let (mut r, mut g, mut b, mut n) = (0f64, 0f64, 0f64, 0f64);
    for y in (top..bottom).step_by(4) {
        for x in (0..w).step_by(4) {
            let i = (y * w + x) * 4;
            if let Some(px) = shot.rgba.get(i..i + 3) {
                r += px[0] as f64;
                g += px[1] as f64;
                b += px[2] as f64;
                n += 1.0;
            }
        }
    }
    if n == 0.0 {
        return (0.0, 0.0, 0.0);
    }
    ((r / n) as f32, (g / n) as f32, (b / n) as f32)
}

pub(super) fn run(
    sc: &mut Script,
    app: &mut App,
    st: PStep,
    el: Duration,
) -> (bool, Task<Message>) {
    let timeout = |ms: u64| el > Duration::from_millis(ms);
    let mut task = Task::none();
    macro_rules! wait {
        ($ok:expr, $ms:expr, $what:expr) => {{
            if $ok {
                sc.check(true, &$what);
                true
            } else if timeout($ms) {
                sc.check(false, &format!("timeout: {}", $what));
                true
            } else {
                false
            }
        }};
    }
    let next = match st {
        PStep::Open(path) => {
            let sid = sc.id(P2).unwrap_or_default();
            let q = qp(sc, app, path);
            task = app.update(Message::Detail(sid, DetailMsg::OpenAt(q, 1, 1)));
            true
        }
        PStep::WaitWindow(open, ms) => {
            let now = app.detail_window.is_some() && app.detail_window_ready;
            let gone = app.detail_window.is_none();
            let ok = if open { now } else { gone };
            wait!(
                ok,
                ms,
                format!(
                    "detail window {} ({:?})",
                    if open { "open" } else { "closed" },
                    app.detail_window
                )
            )
        }
        PStep::ExpectSaved(detached, collapsed, pinned) => {
            let ui = UiState::load(&sc.data);
            let got = (ui.detail_detached, ui.detail_collapsed, ui.detail_pinned);
            sc.check(
                got == (detached, collapsed, pinned),
                &format!("ui.json (detached, collapsed, pinned) = {got:?}"),
            );
            true
        }
        PStep::ExpectInApp(v) => {
            sc.check(
                app.detail_in_app() == v,
                &format!("detail pane in the app window: {}", app.detail_in_app()),
            );
            true
        }
        PStep::MarkCols => {
            let cols = app.selected_live().and_then(|l| l.size).map_or(0, |s| s.0);
            *COLS.lock().unwrap() = cols;
            log(&format!("terminal columns before detaching: {cols}"));
            true
        }
        PStep::WaitWider(n, ms) => {
            let before = *COLS.lock().unwrap();
            let cols = app.selected_live().and_then(|l| l.size).map_or(0, |s| s.0);
            wait!(
                cols >= before + n,
                ms,
                format!("terminal widened from {before} to {cols} columns")
            )
        }
        PStep::Shot(name) => {
            match app.detail_window {
                Some(id) => {
                    sc.waiting_shot = true;
                    task = window::screenshot(id).map(move |s| Message::Shot(name.into(), s));
                }
                None => sc.check(false, &format!("no detail window for {name}")),
            }
            true
        }
        PStep::ExpectWebDrawn => {
            match sc.last_shot.as_ref() {
                Some((name, shot)) => {
                    let (r, g, b) = web_rgb(app, shot);
                    // The page is a blue-violet gradient; the empty pane is near-black grey.
                    sc.check(
                        b > 110.0 && b > g + 20.0 && b > r,
                        &format!("{name}: web page drawn in the detail window (mean rgb {r:.0},{g:.0},{b:.0})"),
                    );
                }
                None => sc.check(false, "no detail window screenshot"),
            }
            true
        }
        PStep::ExpectTitle(needle) => {
            let title = app.detail_window.map(|id| app.title(id)).unwrap_or_default();
            sc.check(
                title.contains(needle),
                &format!("detail window title {title:?} contains {needle:?}"),
            );
            true
        }
        PStep::CloseButton => {
            if let Some(id) = app.detail_window {
                task = app.update(Message::WindowCloseRequest(id));
            }
            true
        }
        PStep::WaitOwner(pinned, ms) => {
            let ok = match app.detail_owner {
                Some((Some(main), owner)) => {
                    if pinned {
                        owner == Some(main)
                    } else {
                        owner.is_none()
                    }
                }
                _ => false,
            };
            wait!(
                ok,
                ms,
                format!(
                    "detail window owner {:?} (pinned {pinned})",
                    app.detail_owner
                )
            )
        }
        PStep::Place(x, y, w, h) => {
            if let Some(id) = app.detail_window {
                log(&format!("place the detail window at {x},{y} size {w}x{h}"));
                // Then ask where it is (Windows sends the app no `Moved` messages).
                task = Task::batch([
                    window::move_to(id, iced::Point::new(x, y)),
                    window::resize(id, iced::Size::new(w, h)),
                ])
                .chain(window::position(id).map(move |p| {
                    Message::DetailWindow(crate::detail_window::WinMsg::Placed(id, p))
                }));
            }
            true
        }
        PStep::WaitPlace(x, y, w, h, ms) => {
            let s = app.detail_window_size;
            let pos = app.detail_window_pos;
            let near = |a: f32, b: f32| (a - b).abs() <= 2.0;
            let ok = app.detail_window_ready
                && pos.is_some_and(|p| near(p.x, x) && near(p.y, y))
                && near(s.width, w)
                && near(s.height, h);
            wait!(
                ok,
                ms,
                format!(
                    "detail window at {pos:?} size {s:?} (saved {:?})",
                    app.ui.detail_window
                )
            )
        }
        PStep::CloseApp => {
            log("close the app (drop client + state; the detail window closes with it)");
            task = app.close_for_reopen();
            true
        }
        PStep::ReopenApp => {
            log("reopen the app (new client, state from ui.json + server)");
            app.connect();
            true
        }
    };
    (next, task)
}
