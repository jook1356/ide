//! Demo of the `meninx-views` widgets on fake data (Korean text, a 20 000-line file, a 10 000-entry
//! folder): file tree + changes list on the left, code view or diff view on the right.
//!
//! `MENINX_VIEWS_SCRIPT=1` runs an in-app test script: it injects the views' own messages (tree
//! keys, editor actions, find / go-to / save, Ctrl+click, diff mode / expand, ...) — no OS input —,
//! logs every callback event, measures open time and scroll frame rates, and saves iced
//! `window::screenshot`s. Output: `MENINX_VIEWS_EVIDENCE` (default `evidence-views` next to the repo)
//! as `<lang>_NN_name.png` + `script_<lang>.log`. `MENINX_VIEWS_LANG=ko|en` picks the UI language;
//! `MENINX_VIEWS_HOLD_MS=<ms>` holds key screens (log line `HOLD <name>`) for an external capture.

use meninx_views::diff::DiffFile;
use meninx_views::{
    ChangeEntry, ChangeGroup, ChangesEvent, ChangesList, ChangesMessage, CodeEvent, CodeMessage, CodeView, DiffMode,
    DiffView, DiffViewEvent, DiffViewMessage, FileTree, GitStatus, MenuAction, TreeEntry, TreeEvent, TreeMessage,
};
use iced::Widget as _;
use iced::advanced::text::editor::{Action, Edit};
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{column, container, row, space};
use iced::{Element, Length, Point, Subscription, Task, Theme, window};
use std::collections::{HashMap, VecDeque};
use std::io::Write as _;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

static T0: OnceLock<Instant> = OnceLock::new();
static LOG: OnceLock<std::sync::Mutex<Option<std::fs::File>>> = OnceLock::new();

fn evidence_dir() -> String {
    std::env::var("MENINX_VIEWS_EVIDENCE")
        .unwrap_or_else(|_| "C:/Users/사용자/hermes-workspaces/20261120-0935-a3f0/evidence-views".into())
}

fn log(s: &str) {
    let t = T0.get_or_init(Instant::now).elapsed().as_secs_f64();
    let line = format!("[{t:8.3}] {s}");
    println!("{line}");
    if let Some(m) = LOG.get()
        && let Ok(mut g) = m.lock()
        && let Some(f) = g.as_mut()
    {
        let _ = writeln!(f, "{line}");
    }
}

// ---------------------------------------------------------------------------------------------
// Fake repository

const MAIN_OLD: &str = r#"//! 데모 프로그램: 파일 트리와 코드 보기를 시험해요.
use std::collections::HashMap;

/// 사용자 정보
#[derive(Debug, Clone)]
pub struct User {
    pub name: String,
    pub age: u32,
}

impl User {
    pub fn new(name: &str, age: u32) -> Self {
        Self { name: name.to_owned(), age }
    }

    pub fn greeting(&self) -> String {
        format!("안녕하세요, {}님!", self.name)
    }
}

/// 나이 합계
fn total_age(users: &HashMap<&str, User>) -> u32 {
    users.values().map(|u| u.age).sum()
}

fn print_all(users: &HashMap<&str, User>) {
    let mut keys: Vec<_> = users.keys().collect();
    keys.sort();
    for key in keys {
        let user = &users[key];
        println!("{key}: {} ({}살)", user.name, user.age);
    }
}

fn main() {
    let mut users = HashMap::new();
    users.insert("kim", User::new("김철수", 30));
    users.insert("lee", User::new("이영희", 27));
    users.insert("park", User::new("박민수", 41));
    print_all(&users);
    for user in users.values() {
        println!("{}", user.greeting());
    }
    let total = total_age(&users);
    println!("합계 = {total}");
    let avg = total as f64 / users.len() as f64;
    println!("평균 = {avg:.1}");
}
"#;

fn main_new() -> String {
    MAIN_OLD
        .replace("User::new(\"김철수\", 30)", "User::new(\"김철수\", 31)")
        .replace(
            "    users.insert(\"park\", User::new(\"박민수\", 41));\n",
            "    users.insert(\"park\", User::new(\"박민수\", 41));\n    users.insert(\"choi\", User::new(\"최지우\", 35));\n",
        )
        .replace("    println!(\"평균 = {avg:.1}\");\n", "    println!(\"평균 나이 = {avg:.2}\");\n    // TODO: 결과를 파일로 저장해요\n")
        .replace("/// 나이 합계\n", "/// 모든 사용자의 나이 합계\n")
}

fn big_text(variant: bool) -> String {
    let mut s = String::with_capacity(1_100_000);
    s.push_str("//! 20 000줄짜리 큰 파일: 스크롤과 찾기 성능 시험용이에요.\n");
    s.push_str("// 아주 긴 줄: 줄 바꿈(Alt+Z)을 켜면 이 주석이 창 너비에 맞춰 여러 줄로 나뉘어 보여요. The quick brown fox jumps over the lazy dog, 다람쥐 헌 쳇바퀴에 타고파, 0123456789 — 끝까지 읽어 주셔서 고마워요.\n");
    let mut n = 2;
    let mut i = 0;
    while n < 20_000 {
        let k = if variant && (i == 1200 || i == 3500) { i * 2 } else { i };
        s.push_str(&format!("/// 함수 {i}: 한글 주석과 숫자를 섞은 테스트 줄이에요.\n"));
        s.push_str(&format!("pub fn func_{i}(x: u64) -> u64 {{\n"));
        s.push_str(&format!("    let s = \"문자열 {k} — mixed ASCII and 한글\";\n"));
        s.push_str(&format!("    x.wrapping_mul({k}) + s.len() as u64\n"));
        s.push_str("}\n");
        n += 5;
        i += 1;
    }
    s
}

const APP_TSX: &str = r#"import { useState } from 'react'

/** 카운터 컴포넌트 */
export function Counter({ label }: { label: string }): JSX.Element {
  const [count, setCount] = useState(0)
  return (
    <button className="counter" onClick={() => setCount((c) => c + 1)}>
      {label}: {count}번 눌렀어요
    </button>
  )
}
"#;

const PARSE_OLD: &str = "/// 간단한 파서\npub fn parse(input: &str) -> Vec<&str> {\n    input.split(',').collect()\n}\n";
const PARSE_NEW: &str =
    "/// 간단한 파서 (공백 제거)\npub fn parse(input: &str) -> Vec<&str> {\n    input.split(',').map(str::trim).collect()\n}\n";
const MEMO: &str = "오늘 할 일\n- 파일 트리 확인하기\n- 큰 파일 열어 보기 (2만 줄)\n- 변경 사항 비교하기\n";
const DESIGN_MD: &str = "# 설계 문서\n\n## 목표\n\n가볍고 빠른 IDE 화면을 만들어요.\n\n```rust\nfn main() {}\n```\n";
const CARGO_OLD: &str = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nserde = \"1\"\n";
const CARGO_NEW: &str =
    "[package]\nname = \"demo\"\nversion = \"0.2.0\"\nedition = \"2024\"\n\n[dependencies]\nserde = \"1\"\ntokio = { version = \"1\", features = [\"full\"] }\n";
const STYLE_CSS: &str = ".counter {\n  color: #e6edf3;\n  background: #1e1f22;\n  border-radius: 6px;\n}\n";
const UTIL_PY: &str = "# 도구 함수\ndef add(a: int, b: int) -> int:\n    \"\"\"두 수를 더해요.\"\"\"\n    return a + b\n";

struct Fake {
    dirs: HashMap<String, Vec<TreeEntry>>,
    files: HashMap<String, String>,
    old: HashMap<String, String>,
}

impl Fake {
    fn new() -> Self {
        use GitStatus::*;
        let mut dirs = HashMap::new();
        dirs.insert(
            String::new(),
            vec![
                TreeEntry::dir("src").with_status(Modified),
                TreeEntry::dir("docs").with_status(Untracked),
                TreeEntry::dir("generated"),
                TreeEntry::dir("assets").with_status(Modified),
                TreeEntry::dir("target").ignored(),
                TreeEntry::file("Cargo.toml").with_status(Modified),
                TreeEntry::file("README.md"),
                TreeEntry::file("한글_메모.txt").with_status(Untracked),
                TreeEntry::file(".gitignore"),
                TreeEntry::file("debug.log").ignored(),
            ],
        );
        dirs.insert(
            "src".into(),
            vec![
                TreeEntry::file("main.rs").with_status(Modified),
                TreeEntry::file("big.rs").with_status(Modified),
                TreeEntry::file("lib.rs"),
                TreeEntry::file("parser.rs").with_status(Renamed),
                TreeEntry::dir("ui").with_status(Added),
                TreeEntry::file("util.py"),
            ],
        );
        dirs.insert(
            "src/ui".into(),
            vec![
                TreeEntry::file("app.tsx").with_status(Added),
                TreeEntry::file("style.css"),
            ],
        );
        dirs.insert(
            "docs".into(),
            vec![TreeEntry::file("설계.md").with_status(Untracked), TreeEntry::file("guide.md")],
        );
        dirs.insert("assets".into(), vec![TreeEntry::file("logo.png").with_status(Modified)]);
        dirs.insert("target".into(), vec![TreeEntry::dir("release").ignored(), TreeEntry::file(".rustc_info.json").ignored()]);
        dirs.insert(
            "generated".into(),
            (0..10_000)
                .map(|i| TreeEntry::file(format!("gen_{i:05}.{}", ["rs", "ts", "json", "md"][i % 4])))
                .collect(),
        );
        let mut files = HashMap::new();
        files.insert("src/main.rs".into(), main_new());
        files.insert("src/big.rs".into(), big_text(true));
        files.insert("src/lib.rs".into(), "pub mod parser;\npub mod util;\n".into());
        files.insert("src/parser.rs".into(), PARSE_NEW.into());
        files.insert("src/ui/app.tsx".into(), APP_TSX.into());
        files.insert("src/ui/style.css".into(), STYLE_CSS.into());
        files.insert("src/util.py".into(), UTIL_PY.into());
        files.insert("docs/설계.md".into(), DESIGN_MD.into());
        files.insert("docs/guide.md".into(), "# Guide\n\nRun `cargo run`.\n".into());
        files.insert("Cargo.toml".into(), CARGO_NEW.into());
        files.insert("README.md".into(), "# demo\n\n데모 저장소예요.\n".into());
        files.insert("한글_메모.txt".into(), MEMO.into());
        files.insert(".gitignore".into(), "/target\n*.log\n".into());
        let mut old = HashMap::new();
        old.insert("src/main.rs".into(), MAIN_OLD.to_owned());
        old.insert("src/big.rs".into(), big_text(false));
        old.insert("Cargo.toml".into(), CARGO_OLD.to_owned());
        old.insert("src/parse.rs".into(), PARSE_OLD.to_owned());
        old.insert("docs/old_notes.md".into(), "# 예전 메모\n\n지워질 파일이에요.\n".into());
        Self { dirs, files, old }
    }

    fn changes(&self) -> (Vec<ChangeEntry>, Vec<ChangeEntry>, Vec<ChangeEntry>) {
        use GitStatus::*;
        (
            vec![
                ChangeEntry::new("src/ui/app.tsx", Added),
                ChangeEntry::renamed("src/parse.rs", "src/parser.rs"),
                ChangeEntry::new("docs/old_notes.md", Deleted),
            ],
            vec![
                ChangeEntry::new("src/main.rs", Modified),
                ChangeEntry::new("src/big.rs", Modified),
                ChangeEntry::new("Cargo.toml", Modified),
                ChangeEntry::new("assets/logo.png", Modified),
            ],
            vec![ChangeEntry::new("한글_메모.txt", Untracked), ChangeEntry::new("docs/설계.md", Untracked)],
        )
    }

    /// `git diff`-style text for one change, parsed like real git output.
    fn diff(&self, path: &str) -> Vec<DiffFile> {
        let unified = |a: &str, b: &str, old: &str, new: &str| -> String {
            similar::TextDiff::from_lines(old, new)
                .unified_diff()
                .context_radius(3)
                .header(a, b)
                .to_string()
        };
        let text = match path {
            "assets/logo.png" => {
                "diff --git a/assets/logo.png b/assets/logo.png\nindex 1111111..2222222 100644\nBinary files a/assets/logo.png and b/assets/logo.png differ\n"
                    .to_owned()
            }
            "src/parser.rs" => format!(
                "diff --git a/src/parse.rs b/src/parser.rs\nsimilarity index 80%\nrename from src/parse.rs\nrename to src/parser.rs\n{}",
                unified("a/src/parse.rs", "b/src/parser.rs", PARSE_OLD, PARSE_NEW)
            ),
            "docs/old_notes.md" => format!(
                "diff --git a/{path} b/{path}\ndeleted file mode 100644\n{}",
                unified(&format!("a/{path}"), "/dev/null", &self.old[path], "")
            ),
            _ => {
                let new = self.files.get(path).cloned().unwrap_or_default();
                let old = self.old.get(path).cloned().unwrap_or_default();
                let a = if self.old.contains_key(path) { format!("a/{path}") } else { "/dev/null".into() };
                format!("diff --git a/{path} b/{path}\n{}", unified(&a, &format!("b/{path}"), &old, &new))
            }
        };
        let mut files = DiffFile::parse_unified(&text);
        for f in &mut files {
            if let Some(t) = self.files.get(&f.new_path) {
                f.new_lines = Some(t.lines().map(str::to_owned).collect());
            }
        }
        files
    }
}

// ---------------------------------------------------------------------------------------------
// Script

#[derive(Debug, Clone)]
enum Step {
    Wait(u64),
    Shot(&'static str),
    /// Hold for an external capture when `MENINX_VIEWS_HOLD_MS` is set.
    Hold(&'static str),
    Tree(TreeMessage),
    TreeKey(Named),
    Reveal(&'static str),
    Code(CodeMessage),
    Type(&'static str),
    GoTo(usize, usize),
    Diff(DiffViewMessage),
    Changes(ChangesMessage),
    OpenChange(ChangeGroup, &'static str),
    /// Measure: frames of continuous scrolling in the open view.
    Bench(BenchKind, usize),
    Check(&'static str),
    /// Starts a timer; the next screenshot logs the time since (includes layout + draw).
    Mark(&'static str),
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BenchKind {
    Code,
    Diff,
    Tree,
}

fn script() -> Vec<Step> {
    use Step::*;
    let k = |n: Named| TreeKey(n);
    vec![
        Wait(600),
        Shot("initial"),
        // Tree: keyboard navigation and lazy expansion.
        // Folders first, sorted: assets, docs, generated, src, target, then files.
        k(Named::ArrowDown), // assets
        k(Named::ArrowDown), // docs
        k(Named::ArrowRight), // expand docs
        k(Named::ArrowDown), // docs/guide.md
        k(Named::ArrowDown), // docs/설계.md
        k(Named::ArrowDown), // generated
        k(Named::ArrowDown), // src
        k(Named::ArrowRight), // expand src
        k(Named::ArrowRight), // src/ui
        k(Named::ArrowRight), // expand src/ui
        k(Named::ArrowDown), // src/ui/app.tsx
        Check("tree_selected"),
        Wait(200),
        Shot("tree_nav"),
        // Open main.rs via the tree.
        Reveal("src/main.rs"),
        k(Named::Enter),
        Wait(300),
        Shot("code_main"),
        Hold("code_main"),
        // Type Korean + ASCII on line 3 and save with the Save message (Ctrl+S binding).
        GoTo(3, 1),
        Type("// 저장 테스트: 한글 입력 ✓\n"),
        Check("dirty"),
        Wait(200),
        Shot("code_dirty"),
        Hold("code_dirty"),
        Code(CodeMessage::Save),
        Check("dirty"),
        Wait(200),
        Shot("code_saved"),
        // Ctrl+click: click at a point in the text area, then the Ctrl+click message.
        Code(CodeMessage::Action(Action::Click(
            Point::new(60.0, 13.5 * 20.0 + 10.0),
            iced::advanced::mouse::click::Kind::Single,
        ))),
        Code(CodeMessage::CtrlClick),
        // Find in main.rs.
        Code(CodeMessage::OpenFind),
        Code(CodeMessage::FindInput("user".into())),
        Code(CodeMessage::FindNext),
        Code(CodeMessage::FindNext),
        Check("find"),
        Wait(250),
        Shot("code_find"),
        Hold("code_find"),
        Code(CodeMessage::FindPrev),
        Check("find"),
        Code(CodeMessage::CloseFind),
        // The 20k-line file.
        Reveal("src/big.rs"),
        Mark("open 20k-line file"),
        k(Named::Enter),
        Shot("big_open"),
        Hold("big_open"),
        Code(CodeMessage::OpenFind),
        Code(CodeMessage::FindInput("한글".into())),
        Code(CodeMessage::FindNext),
        Check("find"),
        Wait(250),
        Shot("big_find"),
        Hold("big_find"),
        Code(CodeMessage::CloseFind),
        Code(CodeMessage::OpenGoto),
        Code(CodeMessage::GotoInput("15000:9".into())),
        Wait(150),
        Shot("big_goto_bar"),
        Mark("go to line 15000 (highlight restarts near the view)"),
        Code(CodeMessage::GotoSubmit),
        Shot("big_goto_15000"),
        Check("cursor"),
        Hold("big_goto_15000"),
        Bench(BenchKind::Code, 240),
        GoTo(1, 1),
        Wait(300),
        Bench(BenchKind::Code, 240),
        GoTo(1, 1),
        Code(CodeMessage::ToggleWrap),
        Wait(200),
        Shot("big_wrap"),
        Hold("big_wrap"),
        Code(CodeMessage::ToggleWrap),
        // Changes list -> diffs.
        OpenChange(ChangeGroup::Unstaged, "src/main.rs"),
        Wait(300),
        Shot("diff_unified"),
        Hold("diff_unified"),
        Diff(DiffViewMessage::SetMode(DiffMode::Split)),
        Wait(300),
        Shot("diff_split"),
        Hold("diff_split"),
        Diff(DiffViewMessage::SetMode(DiffMode::Unified)),
        Check("diff_rows"),
        Diff(DiffViewMessage::Expand { file: 0, region: 1 }),
        Check("diff_rows"),
        Wait(300),
        Shot("diff_expanded"),
        OpenChange(ChangeGroup::Unstaged, "src/big.rs"),
        Check("diff_rows"),
        Wait(300),
        Shot("diff_big"),
        Diff(DiffViewMessage::Expand { file: 0, region: 0 }),
        Diff(DiffViewMessage::Expand { file: 0, region: 1 }),
        Diff(DiffViewMessage::Expand { file: 0, region: 2 }),
        Check("diff_rows"),
        Bench(BenchKind::Diff, 240),
        Shot("diff_big_expanded_scrolled"),
        OpenChange(ChangeGroup::Unstaged, "assets/logo.png"),
        Wait(250),
        Shot("diff_binary"),
        OpenChange(ChangeGroup::Staged, "src/parser.rs"),
        Wait(250),
        Shot("diff_rename"),
        OpenChange(ChangeGroup::Staged, "src/ui/app.tsx"),
        Diff(DiffViewMessage::SetMode(DiffMode::Split)),
        Wait(250),
        Shot("diff_added_split"),
        Changes(ChangesMessage::ToggleGroup(ChangeGroup::Staged)),
        Wait(200),
        Shot("changes_collapsed"),
        Changes(ChangesMessage::ToggleGroup(ChangeGroup::Staged)),
        // 10k-entry folder.
        Reveal("generated"),
        k(Named::ArrowRight),
        Check("tree_rows"),
        Wait(300),
        Shot("tree_10k"),
        Bench(BenchKind::Tree, 180),
        Shot("tree_10k_scrolled"),
        // Inline rename / new file / delete request via the context menu and keys.
        Reveal("README.md"),
        Tree(TreeMessage::Key(Key::Named(Named::Home), Modifiers::empty())),
        Reveal("README.md"),
        Tree(TreeMessage::Key(Key::Named(Named::F2), Modifiers::empty())),
        Tree(TreeMessage::EditInput("읽어보기.md".into())),
        Wait(250),
        Shot("tree_rename"),
        Hold("tree_rename"),
        Tree(TreeMessage::EditSubmit),
        Reveal("src/main.rs"),
        Tree(TreeMessage::ContextAt(Point::new(140.0, 120.0))),
        Wait(200),
        Shot("tree_menu"),
        Tree(TreeMessage::Menu(MenuAction::NewFile)),
        Tree(TreeMessage::EditInput("새_파일.rs".into())),
        Wait(200),
        Shot("tree_new_file"),
        Tree(TreeMessage::EditSubmit),
        TreeKey(Named::Delete),
        Wait(300),
        Exit,
    ]
}

// ---------------------------------------------------------------------------------------------
// App

enum Main {
    Empty,
    Code(CodeView),
    Diff(DiffView),
}

#[derive(Debug, Clone)]
enum Message {
    Tree(TreeMessage),
    Code(CodeMessage),
    Diff(DiffViewMessage),
    Changes(ChangesMessage),
    WindowId(Option<window::Id>),
    Tick,
    Frame(Instant),
    Shot(String, window::Screenshot),
}

struct BenchRun {
    kind: BenchKind,
    left: usize,
    frames: Vec<Instant>,
    work_ms: Vec<f32>,
    dir: i32,
}

struct App {
    fake: Fake,
    tree: FileTree,
    changes: ChangesList,
    main: Main,
    window: Option<window::Id>,
    lang: &'static str,
    script: VecDeque<Step>,
    scripted: bool,
    wait_until: Option<Instant>,
    pending_shots: usize,
    shot_no: usize,
    bench: Option<BenchRun>,
    /// Time an open started; logged at the next frame.
    open_started: Option<(String, Instant)>,
    hold_ms: u64,
    mark: Option<(&'static str, Instant)>,
    view_ms: std::cell::Cell<f32>,
}

impl App {
    fn boot() -> (App, Task<Message>) {
        let fake = Fake::new();
        let mut tree = FileTree::new();
        tree.set_children("", fake.dirs[""].clone());
        let mut changes = ChangesList::new();
        let (s, u, t) = fake.changes();
        changes.set_all(s, u, t);
        let scripted = std::env::var("MENINX_VIEWS_SCRIPT").is_ok_and(|v| v == "1");
        let lang = if meninx_i18n::current() == meninx_i18n::Lang::Ko { "ko" } else { "en" };
        let app = App {
            fake,
            tree,
            changes,
            main: Main::Empty,
            window: None,
            lang,
            script: if scripted { script().into() } else { VecDeque::new() },
            scripted,
            wait_until: None,
            pending_shots: 0,
            shot_no: 0,
            bench: None,
            open_started: None,
            mark: None,
            hold_ms: std::env::var("MENINX_VIEWS_HOLD_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0),
            view_ms: std::cell::Cell::new(0.0),
        };
        log(&format!("boot lang={lang} scripted={scripted} pid={}", std::process::id()));
        (app, window::latest().map(Message::WindowId))
    }

    fn open_file(&mut self, path: &str) {
        let Some(text) = self.fake.files.get(path) else {
            log(&format!("open {path}: no such fake file"));
            return;
        };
        let t = Instant::now();
        let v = CodeView::new(path, text);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        log(&format!(
            "open {path}: {} lines, {} bytes, highlighted={} construct={ms:.1} ms",
            v.line_count(),
            text.len(),
            v.is_highlighted()
        ));
        self.main = Main::Code(v);
        self.open_started = Some((path.to_owned(), t));
    }

    fn on_tree_event(&mut self, ev: TreeEvent) {
        log(&format!("TreeEvent {ev:?}"));
        match ev {
            TreeEvent::Expand(dir) => {
                if let Some(children) = self.fake.dirs.get(&dir).cloned() {
                    let t = Instant::now();
                    self.tree.set_children(&dir, children);
                    log(&format!(
                        "  set_children({dir}) -> {} rows in {:.2} ms",
                        self.tree.visible_rows(),
                        t.elapsed().as_secs_f64() * 1000.0
                    ));
                }
            }
            TreeEvent::Open(path) => self.open_file(&path),
            _ => {}
        }
    }

    fn on_code_event(&mut self, ev: CodeEvent) {
        match &ev {
            CodeEvent::Save { path, text } => {
                log(&format!("CodeEvent Save {{ path: {path}, {} bytes }}", text.len()));
                self.fake.files.insert(path.clone(), text.clone());
                if let Main::Code(v) = &mut self.main
                    && let Some(e) = v.mark_saved(text)
                {
                    log(&format!("  mark_saved -> {e:?}"));
                }
            }
            other => log(&format!("CodeEvent {other:?}")),
        }
    }

    fn open_change(&mut self, group: ChangeGroup, path: &str) {
        let t = Instant::now();
        let files = self.fake.diff(path);
        let mut d = DiffView::new(files);
        if let Main::Diff(old) = &self.main {
            d.set_mode(old.mode());
        }
        log(&format!(
            "open diff {path} ({group:?}): {} rows, collapsed {:?}, {:.1} ms",
            d.row_count(),
            d.collapsed(),
            t.elapsed().as_secs_f64() * 1000.0
        ));
        self.main = Main::Diff(d);
        self.open_started = Some((format!("diff {path}"), t));
    }

    fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::Tree(m) => {
                let (task, ev) = self.tree.update(m);
                if let Some(ev) = ev {
                    self.on_tree_event(ev);
                }
                task.map(Message::Tree)
            }
            Message::Code(m) => {
                let Main::Code(v) = &mut self.main else {
                    return Task::none();
                };
                let (task, ev) = v.update(m);
                if let Some(ev) = ev {
                    self.on_code_event(ev);
                }
                task.map(Message::Code)
            }
            Message::Diff(m) => {
                let Main::Diff(v) = &mut self.main else {
                    return Task::none();
                };
                let (task, ev) = v.update(m);
                if let Some(DiffViewEvent::ModeChanged(mode)) = ev {
                    log(&format!("DiffViewEvent ModeChanged({mode:?}) rows={}", v.row_count()));
                }
                task.map(Message::Diff)
            }
            Message::Changes(m) => {
                let (task, ev) = self.changes.update(m);
                if let Some(ChangesEvent::OpenDiff { group, path }) = ev {
                    log(&format!("ChangesEvent OpenDiff {{ group: {group:?}, path: {path} }}"));
                    self.open_change(group, &path);
                }
                task.map(Message::Changes)
            }
            Message::WindowId(id) => {
                self.window = id;
                Task::none()
            }
            Message::Shot(name, shot) => {
                let path = format!("{}/{}.png", evidence_dir(), name);
                match std::fs::File::create(&path) {
                    Ok(f) => {
                        let mut enc = png::Encoder::new(std::io::BufWriter::new(f), shot.size.width, shot.size.height);
                        enc.set_color(png::ColorType::Rgba);
                        enc.set_depth(png::BitDepth::Eight);
                        let r = enc.write_header().and_then(|mut w| w.write_image_data(&shot.rgba));
                        log(&format!("screenshot {path} {}x{} {r:?}", shot.size.width, shot.size.height));
                    }
                    Err(e) => log(&format!("screenshot {path}: {e}")),
                }
                self.pending_shots -= 1;
                if let Some((what, t)) = self.mark.take() {
                    log(&format!("TIME {what}: {:.1} ms until the screenshot was taken", t.elapsed().as_secs_f64() * 1000.0));
                }
                Task::none()
            }
            Message::Frame(now) => self.on_frame(now),
            Message::Tick => self.step(),
        }
    }

    fn on_frame(&mut self, now: Instant) -> Task<Message> {
        if let Some((what, t)) = self.open_started.take() {
            // The frame event comes after the layout of the new view (before its draw).
            log(&format!(
                "  open {what} -> first frame event: {:.1} ms (view {:.2} ms)",
                t.elapsed().as_secs_f64() * 1000.0,
                self.view_ms.get()
            ));
        }
        let Some(b) = &mut self.bench else {
            return Task::none();
        };
        b.frames.push(now);
        if b.left == 0 {
            let b = self.bench.take().unwrap();
            let mut iv: Vec<f32> = b.frames.windows(2).map(|w| (w[1] - w[0]).as_secs_f32() * 1000.0).collect();
            iv.sort_by(f32::total_cmp);
            let wall = (*b.frames.last().unwrap() - b.frames[0]).as_secs_f64();
            let mut work = b.work_ms.clone();
            work.sort_by(f32::total_cmp);
            let pct = |v: &[f32], p: f64| v.get(((v.len() as f64 - 1.0) * p) as usize).copied().unwrap_or(0.0);
            log(&format!(
                "BENCH {:?} scroll: {} frames in {wall:.2}s = {:.1} fps | frame interval p50 {:.1} p95 {:.1} max {:.1} ms | update+view p50 {:.2} p95 {:.2} max {:.2} ms",
                b.kind,
                b.frames.len(),
                (b.frames.len() - 1) as f64 / wall,
                pct(&iv, 0.5),
                pct(&iv, 0.95),
                iv.last().copied().unwrap_or(0.0),
                pct(&work, 0.5),
                pct(&work, 0.95),
                work.last().copied().unwrap_or(0.0),
            ));
            return Task::none();
        }
        b.left -= 1;
        let t = Instant::now();
        let (kind, dir) = (b.kind, b.dir);
        let task = match kind {
            BenchKind::Code => {
                if let Main::Code(v) = &mut self.main {
                    let _ = v.update(CodeMessage::Action(Action::Scroll { lines: 3 * dir }));
                    if v.top_line() > v.line_count().saturating_sub(80) || (dir < 0 && v.top_line() <= 1) {
                        self.bench.as_mut().unwrap().dir = -dir;
                    }
                }
                Task::none()
            }
            BenchKind::Diff => match &mut self.main {
                Main::Diff(v) => v.update(DiffViewMessage::Key(Key::Named(Named::ArrowDown))).0.map(Message::Diff),
                _ => Task::none(),
            },
            BenchKind::Tree => self
                .tree
                .update(TreeMessage::Key(Key::Named(Named::PageDown), Modifiers::empty()))
                .0
                .map(Message::Tree),
        };
        let _ = self.view(); // include view building in the work time, as the runtime does
        self.bench.as_mut().unwrap().work_ms.push(t.elapsed().as_secs_f32() * 1000.0);
        task
    }

    fn check(&self, what: &str) {
        let s = match what {
            "tree_selected" => format!("tree selected = {:?}", self.tree.selected()),
            "tree_rows" => format!("tree visible rows = {}", self.tree.visible_rows()),
            "dirty" => match &self.main {
                Main::Code(v) => format!("code dirty = {}", v.is_dirty()),
                _ => "no code view".into(),
            },
            "find" => match &self.main {
                Main::Code(v) => format!("find state = {:?}, cursor = {:?}", v.find_state(), v.cursor()),
                _ => "no code view".into(),
            },
            "cursor" => match &self.main {
                Main::Code(v) => format!("cursor = {:?}, top line = {}", v.cursor(), v.top_line()),
                _ => "no code view".into(),
            },
            "diff_rows" => match &self.main {
                Main::Diff(v) => format!("diff mode {:?} rows {} collapsed {:?}", v.mode(), v.row_count(), v.collapsed()),
                _ => "no diff view".into(),
            },
            _ => String::new(),
        };
        log(&format!("CHECK {s}"));
    }

    fn step(&mut self) -> Task<Message> {
        if self.pending_shots > 0 || self.bench.is_some() {
            return Task::none();
        }
        if let Some(t) = self.wait_until {
            if Instant::now() < t {
                return Task::none();
            }
            self.wait_until = None;
        }
        let Some(step) = self.script.pop_front() else {
            return Task::none();
        };
        log(&format!("step {step:?}"));
        match step {
            Step::Wait(ms) => {
                self.wait_until = Some(Instant::now() + Duration::from_millis(ms));
                Task::none()
            }
            Step::Hold(name) => {
                if self.hold_ms > 0 {
                    log(&format!("HOLD {name} pid={}", std::process::id()));
                    self.wait_until = Some(Instant::now() + Duration::from_millis(self.hold_ms));
                }
                Task::none()
            }
            Step::Shot(name) => {
                let Some(id) = self.window else {
                    return Task::none();
                };
                self.shot_no += 1;
                self.pending_shots += 1;
                let name = format!("{}_{:02}_{name}", self.lang, self.shot_no);
                window::screenshot(id).map(move |s| Message::Shot(name.clone(), s))
            }
            Step::Tree(m) => self.update(Message::Tree(m)),
            Step::TreeKey(n) => self.update(Message::Tree(TreeMessage::Key(Key::Named(n), Modifiers::empty()))),
            Step::Reveal(p) => {
                let (task, events) = self.tree.reveal(p);
                for e in events {
                    self.on_tree_event(e);
                }
                task.map(Message::Tree)
            }
            Step::Code(m) => self.update(Message::Code(m)),
            Step::Type(s) => {
                let mut tasks = Vec::new();
                for c in s.chars() {
                    let edit = if c == '\n' { Edit::Enter } else { Edit::Insert(c) };
                    tasks.push(self.update(Message::Code(CodeMessage::Action(Action::Edit(edit)))));
                }
                Task::batch(tasks)
            }
            Step::GoTo(l, c) => match &mut self.main {
                Main::Code(v) => v.go_to(l, c),
                _ => Task::none(),
            },
            Step::Diff(m) => self.update(Message::Diff(m)),
            Step::Changes(m) => self.update(Message::Changes(m)),
            Step::OpenChange(g, p) => {
                let i = self.changes.entries(g).iter().position(|e| e.path == p).unwrap_or(0);
                self.update(Message::Changes(ChangesMessage::Open(g, i)))
            }
            Step::Bench(kind, frames) => {
                self.bench = Some(BenchRun {
                    kind,
                    left: frames,
                    frames: Vec::with_capacity(frames + 1),
                    work_ms: Vec::with_capacity(frames),
                    dir: 1,
                });
                Task::none()
            }
            Step::Mark(what) => {
                self.mark = Some((what, Instant::now()));
                Task::none()
            }
            Step::Check(what) => {
                self.check(what);
                Task::none()
            }
            Step::Exit => {
                log("script done");
                iced::exit()
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let t = Instant::now();
        let divider_h = || container(space()).width(Length::Fill).height(1).style(meninx_views::theme::bg(meninx_views::theme::BORDER));
        let side = column![
            container(self.tree.view().map(Message::Tree)).height(Length::FillPortion(3)),
            divider_h(),
            container(self.changes.view().map(Message::Changes)).height(Length::FillPortion(2)),
        ]
        .width(300);
        let main: Element<'_, Message> = match &self.main {
            Main::Empty => container(space())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(meninx_views::theme::bg(meninx_views::theme::BG))
                .boxed(),
            Main::Code(v) => v.view().map(Message::Code).boxed(),
            Main::Diff(v) => v.view().map(Message::Diff).boxed(),
        };
        let e = row![
            side,
            container(space()).width(1).height(Length::Fill).style(meninx_views::theme::bg(meninx_views::theme::BORDER)),
            main
        ]
        .boxed();
        self.view_ms.set(t.elapsed().as_secs_f32() * 1000.0);
        e
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![window::frames().map(Message::Frame)];
        if self.scripted {
            subs.push(iced::time::every(Duration::from_millis(30)).map(|_| Message::Tick));
        }
        Subscription::batch(subs)
    }
}

fn main() -> iced::Result {
    T0.get_or_init(Instant::now);
    if let Ok(code) = std::env::var("MENINX_VIEWS_LANG")
        && let Some(lang) = meninx_i18n::Lang::from_code(&code)
    {
        meninx_i18n::set_language(lang);
    }
    let dir = evidence_dir();
    let _ = std::fs::create_dir_all(&dir);
    let lang = if meninx_i18n::current() == meninx_i18n::Lang::Ko { "ko" } else { "en" };
    let _ = LOG.set(std::sync::Mutex::new(std::fs::File::create(format!("{dir}/script_{lang}.log")).ok()));
    iced::application(App::boot, App::update, App::view)
        .title("meninx-views")
        .theme(|_: &App| Theme::Dark)
        .subscription(App::subscription)
        .window_size((1280.0, 800.0))
        .run()
}
