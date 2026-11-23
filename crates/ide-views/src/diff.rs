//! Diff of one or more files (Hermes Desk `DiffView`, GitHub-Desktop style): unified or
//! side-by-side, old/new line numbers, +/- tinted rows, syntax highlighting, word-level
//! highlights for changed line pairs, collapsed unchanged regions with an expand control,
//! no-newline markers, binary / rename-only notes, and a file header with path and +N −M.
//! Virtualised: only the rows in view are built.

use crate::interact::Interact;
use crate::syntax;
use crate::theme;
use crate::vlist::VList;
use crate::icons;
use ide_i18n::tr;
use iced::Widget as _;
use iced::widget::scrollable::Viewport;
use iced::widget::text::Span;
use iced::widget::{button, container, rich_text, row, space, text};
use iced::{Alignment, Code, Color, Element, Length, Task};
use std::collections::HashSet;
use std::ops::Range;

/// Unchanged lines kept around changes before a run is collapsed.
pub const CONTEXT: usize = 3;
const LINE_H: f32 = 20.0;
const HEAD_H: f32 = 34.0;
const NOTE_H: f32 = 36.0;
/// Files with more diff lines than this are not syntax highlighted.
const MAX_HIGHLIGHT_LINES: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineKind {
    Context,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// 1-based line number on the old side (`None` for added lines).
    pub old_no: Option<u32>,
    /// 1-based line number on the new side (`None` for deleted lines).
    pub new_no: Option<u32>,
    pub text: String,
    /// `\ No newline at end of file` follows this line.
    pub no_newline: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    /// Text after the second `@@` (function context), may be empty.
    pub section: String,
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    pub fn header(&self) -> String {
        let mut s = format!(
            "@@ -{},{} +{},{} @@",
            self.old_start, self.old_count, self.new_start, self.new_count
        );
        if !self.section.is_empty() {
            s.push(' ');
            s.push_str(&self.section);
        }
        s
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DiffFile {
    /// Old path when it differs (renames) or `None`.
    pub old_path: Option<String>,
    pub new_path: String,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
    /// The whole new-side file (lines without endings), when known: the unchanged lines
    /// between hunks can then be expanded too.
    pub new_lines: Option<Vec<String>>,
}

impl DiffFile {
    pub fn added(&self) -> usize {
        self.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.kind == LineKind::Add)
            .count()
    }

    pub fn removed(&self) -> usize {
        self.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.kind == LineKind::Del)
            .count()
    }

    /// Parses `git diff` / `diff -u` output into files.
    pub fn parse_unified(text: &str) -> Vec<DiffFile> {
        let mut files: Vec<DiffFile> = Vec::new();
        let mut cur: Option<DiffFile> = None;
        let mut old_no = 0u32;
        let mut new_no = 0u32;
        let mut in_hunk = false;
        let strip = |p: &str| -> Option<String> {
            let p = p.trim_end_matches(['\r']).split('\t').next().unwrap_or("");
            if p == "/dev/null" {
                None
            } else {
                Some(
                    p.strip_prefix("a/")
                        .or_else(|| p.strip_prefix("b/"))
                        .unwrap_or(p)
                        .to_owned(),
                )
            }
        };
        for raw in text.split('\n') {
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            if let Some(rest) = line.strip_prefix("diff --git ") {
                files.extend(cur.take());
                let mut f = DiffFile::default();
                if let Some((a, b)) = rest.split_once(" b/") {
                    f.old_path = strip(a);
                    f.new_path = b.to_owned();
                }
                cur = Some(f);
                in_hunk = false;
                continue;
            }
            if !in_hunk || !matches!(line.as_bytes().first(), Some(b' ' | b'+' | b'-' | b'\\')) {
                in_hunk = false;
                if let Some(p) = line.strip_prefix("--- ") {
                    if cur.as_ref().is_none_or(|f| !f.hunks.is_empty()) {
                        files.extend(cur.take());
                        cur = Some(DiffFile::default());
                    }
                    if let Some(f) = &mut cur {
                        f.old_path = strip(p);
                    }
                    continue;
                }
                if let Some(p) = line.strip_prefix("+++ ") {
                    if let Some(f) = &mut cur
                        && let Some(p) = strip(p)
                    {
                        f.new_path = p;
                    }
                    continue;
                }
                if let Some(f) = &mut cur {
                    if line.starts_with("Binary files ") || line == "GIT binary patch" {
                        f.binary = true;
                    } else if let Some(p) = line.strip_prefix("rename from ") {
                        f.old_path = Some(p.to_owned());
                    } else if let Some(p) = line.strip_prefix("rename to ") {
                        f.new_path = p.to_owned();
                    } else if let Some(h) = parse_hunk_header(line) {
                        old_no = h.old_start;
                        new_no = h.new_start;
                        f.hunks.push(h);
                        in_hunk = true;
                    }
                }
                continue;
            }
            let Some(h) = cur.as_mut().and_then(|f| f.hunks.last_mut()) else {
                continue;
            };
            let (kind, body) = (line.as_bytes()[0], &line[1..]);
            match kind {
                b'\\' => {
                    if let Some(l) = h.lines.last_mut() {
                        l.no_newline = true;
                    }
                }
                b' ' => {
                    h.lines.push(DiffLine {
                        kind: LineKind::Context,
                        old_no: Some(old_no),
                        new_no: Some(new_no),
                        text: body.to_owned(),
                        no_newline: false,
                    });
                    old_no += 1;
                    new_no += 1;
                }
                b'-' => {
                    h.lines.push(DiffLine {
                        kind: LineKind::Del,
                        old_no: Some(old_no),
                        new_no: None,
                        text: body.to_owned(),
                        no_newline: false,
                    });
                    old_no += 1;
                }
                _ => {
                    h.lines.push(DiffLine {
                        kind: LineKind::Add,
                        old_no: None,
                        new_no: Some(new_no),
                        text: body.to_owned(),
                        no_newline: false,
                    });
                    new_no += 1;
                }
            }
        }
        files.extend(cur);
        for f in &mut files {
            if f.old_path.as_deref() == Some(f.new_path.as_str()) {
                f.old_path = None;
            }
            if f.new_path.is_empty()
                && let Some(o) = f.old_path.take()
            {
                f.new_path = o;
            }
        }
        files
    }
}

fn parse_hunk_header(line: &str) -> Option<Hunk> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, section) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |s: &str| -> Option<(u32, u32)> {
        match s.split_once(',') {
            Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
            None => Some((s.parse().ok()?, 1)),
        }
    };
    let (old_start, old_count) = range(old)?;
    let (new_start, new_count) = range(new)?;
    Some(Hunk {
        old_start,
        old_count,
        new_start,
        new_count,
        section: section.trim().to_owned(),
        lines: Vec::new(),
    })
}

/// Byte ranges of `old` and `new` that differ, by Unicode words; empty when the lines are too
/// different for word highlights to help.
pub fn word_changes(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    use similar::{Algorithm, DiffTag, capture_diff_slices_deadline};
    let (ot, nt) = (tokens(old), tokens(new));
    let os: Vec<&str> = ot.iter().map(|r| &old[r.clone()]).collect();
    let ns: Vec<&str> = nt.iter().map(|r| &new[r.clone()]).collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(5);
    let ops = capture_diff_slices_deadline(Algorithm::Myers, &os, &ns, Some(deadline));
    let span = |t: &[Range<usize>], r: Range<usize>| -> Range<usize> {
        if r.is_empty() { 0..0 } else { t[r.start].start..t[r.end - 1].end }
    };
    let (mut a, mut b) = (Vec::new(), Vec::new());
    let mut same = 0usize;
    for op in ops {
        let (tag, or, nr) = op.as_tag_tuple();
        match tag {
            DiffTag::Equal => same += span(&ot, or).len(),
            DiffTag::Delete => push_range(&mut a, span(&ot, or)),
            DiffTag::Insert => push_range(&mut b, span(&nt, nr)),
            DiffTag::Replace => {
                push_range(&mut a, span(&ot, or));
                push_range(&mut b, span(&nt, nr));
            }
        }
    }
    // Mostly rewritten lines: whole-line tint only.
    let longest = old.trim().len().max(new.trim().len()).max(1);
    if same * 10 < longest * 4 {
        return (Vec::new(), Vec::new());
    }
    (a, b)
}

/// Word tokens: runs of alphanumerics / `_`, runs of whitespace, single other characters.
fn tokens(s: &str) -> Vec<Range<usize>> {
    #[derive(PartialEq, Clone, Copy)]
    enum K { Word, Space, Other }
    let kind = |c: char| if c.is_alphanumeric() || c == '_' { K::Word } else if c.is_whitespace() { K::Space } else { K::Other };
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut prev: Option<K> = None;
    for (i, c) in s.char_indices() {
        let k = kind(c);
        match (out.last_mut(), prev) {
            (Some(last), Some(p)) if p == k && k != K::Other => last.end = i + c.len_utf8(),
            _ => out.push(i..i + c.len_utf8()),
        }
        prev = Some(k);
    }
    out
}

fn push_range(v: &mut Vec<Range<usize>>, r: Range<usize>) {
    if r.is_empty() {
        return;
    }
    match v.last_mut() {
        Some(last) if last.end == r.start => last.end = r.end,
        _ => v.push(r),
    }
}

/// A prepared display line.
#[derive(Debug, Clone)]
struct DLine {
    kind: LineKind,
    old_no: Option<u32>,
    new_no: Option<u32>,
    text: String,
    hl: Vec<(Range<usize>, Code)>,
    words: Vec<Range<usize>>,
    no_newline: bool,
}

#[derive(Debug, Clone)]
enum Segment {
    Lines(Range<usize>),
    /// Unchanged lines hidden until expanded (`id` is unique within the file).
    Collapsible { id: usize, lines: Range<usize> },
    Hunk(String),
}

#[derive(Debug)]
struct Prepared {
    file: DiffFile,
    lines: Vec<DLine>,
    segments: Vec<Segment>,
    note: Option<String>,
}

fn prepare(file: DiffFile) -> Prepared {
    let note = if file.binary {
        Some(tr::diff_binary())
    } else if file.hunks.iter().all(|h| h.lines.is_empty()) {
        Some(match &file.old_path {
            Some(o) => tr::diff_renamed_only(o, &file.new_path),
            None => tr::diff_no_changes(),
        })
    } else {
        None
    };
    let mut p = Prepared {
        lines: Vec::new(),
        segments: Vec::new(),
        note,
        file,
    };
    if p.note.is_some() {
        return p;
    }
    let token = syntax::language_token(&p.file.new_path);
    let total: usize = p.file.hunks.iter().map(|h| h.lines.len()).sum();
    let token = token.filter(|_| total <= MAX_HIGHLIGHT_LINES);

    // Syntax: old side (context + deleted) and new side (context + added) as continuous
    // snippets per file, so multi-line strings / comments carry over between lines.
    let (mut old_hl, mut new_hl) = (Vec::new(), Vec::new());
    if let Some(t) = &token {
        let old_src = p.file.hunks.iter().flat_map(|h| &h.lines).filter(|l| l.kind != LineKind::Add);
        let new_src = p.file.hunks.iter().flat_map(|h| &h.lines).filter(|l| l.kind != LineKind::Del);
        old_hl = syntax::highlight_lines(t, old_src.map(|l| l.text.as_str()));
        new_hl = syntax::highlight_lines(t, new_src.map(|l| l.text.as_str()));
    }
    let full_hl = match (&token, &p.file.new_lines) {
        (Some(t), Some(lines)) if lines.len() <= MAX_HIGHLIGHT_LINES => {
            syntax::highlight_lines(t, lines.iter().map(String::as_str))
        }
        _ => Vec::new(),
    };
    let (mut oi, mut ni) = (0usize, 0usize);
    let mut region = 0usize;
    let mut prev_new_end: u32 = 0; // last new-side line number shown so far
    let mut prev_delta: i64 = 0; // old_no - new_no for context lines
    let hunks = std::mem::take(&mut p.file.hunks);
    for h in &hunks {
        // Unchanged lines between the previous hunk and this one (from the full file).
        gap_lines(
            &mut p,
            &full_hl,
            prev_new_end + 1,
            h.new_start.max(1),
            h.old_start as i64 - h.new_start as i64,
            &mut region,
        );
        p.segments.push(Segment::Hunk(h.header()));
        let base = p.lines.len();
        for l in &h.lines {
            let hl = match l.kind {
                LineKind::Del => {
                    oi += 1;
                    old_hl.get(oi - 1).cloned()
                }
                LineKind::Add => {
                    ni += 1;
                    new_hl.get(ni - 1).cloned()
                }
                LineKind::Context => {
                    oi += 1;
                    ni += 1;
                    new_hl.get(ni - 1).cloned()
                }
            };
            p.lines.push(DLine {
                kind: l.kind,
                old_no: l.old_no,
                new_no: l.new_no,
                text: l.text.clone(),
                hl: hl.unwrap_or_default(),
                words: Vec::new(),
                no_newline: l.no_newline,
            });
        }
        let end = p.lines.len();
        word_pairs(&mut p.lines[base..end]);
        // Long unchanged runs inside the hunk collapse (keeping CONTEXT lines around changes).
        let mut i = base;
        let mut shown_from = base;
        while i < end {
            if p.lines[i].kind != LineKind::Context {
                i += 1;
                continue;
            }
            let run_start = i;
            while i < end && p.lines[i].kind == LineKind::Context {
                i += 1;
            }
            let keep_before = if run_start == base { 0 } else { CONTEXT };
            let keep_after = if i == end { 0 } else { CONTEXT };
            let hide = run_start + keep_before..i.saturating_sub(keep_after);
            if hide.end > hide.start && hide.len() > 2 {
                if hide.start > shown_from {
                    p.segments.push(Segment::Lines(shown_from..hide.start));
                }
                p.segments.push(Segment::Collapsible {
                    id: region,
                    lines: hide.clone(),
                });
                region += 1;
                shown_from = hide.end;
            }
        }
        if shown_from < end {
            p.segments.push(Segment::Lines(shown_from..end));
        }
        prev_new_end = h.new_start + h.new_count.saturating_sub(1);
        if h.new_count == 0 {
            prev_new_end = h.new_start;
        }
        prev_delta = (h.old_start + h.old_count) as i64 - (h.new_start + h.new_count) as i64;
    }
    // After the last hunk.
    if let Some(n) = p.file.new_lines.as_ref().map(|l| l.len() as u32) {
        gap_lines(&mut p, &full_hl, prev_new_end + 1, n + 1, prev_delta, &mut region);
    }
    p.file.hunks = hunks;
    p
}

/// Adds the unchanged new-side lines `from..to` (1-based, from `new_lines`) as a collapsed
/// region.
fn gap_lines(
    p: &mut Prepared,
    full_hl: &[Vec<(Range<usize>, Code)>],
    from: u32,
    to: u32,
    delta: i64,
    region: &mut usize,
) {
    let Some(lines) = &p.file.new_lines else {
        return;
    };
    if to <= from {
        return;
    }
    let start = p.lines.len();
    for n in from..to {
        let Some(t) = lines.get(n as usize - 1) else {
            break;
        };
        p.lines.push(DLine {
            kind: LineKind::Context,
            old_no: Some((n as i64 + delta).max(1) as u32),
            new_no: Some(n),
            text: t.clone(),
            hl: full_hl.get(n as usize - 1).cloned().unwrap_or_default(),
            words: Vec::new(),
            no_newline: false,
        });
    }
    let end = p.lines.len();
    if end > start {
        p.segments.push(Segment::Collapsible {
            id: *region,
            lines: start..end,
        });
        *region += 1;
    }
}

/// Word highlights for each block of deleted lines followed by added lines (paired in order).
fn word_pairs(lines: &mut [DLine]) {
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind != LineKind::Del {
            i += 1;
            continue;
        }
        let d0 = i;
        while i < lines.len() && lines[i].kind == LineKind::Del {
            i += 1;
        }
        let a0 = i;
        while i < lines.len() && lines[i].kind == LineKind::Add {
            i += 1;
        }
        let pairs = (a0 - d0).min(i - a0);
        for k in 0..pairs {
            let (o, n) = word_changes(&lines[d0 + k].text, &lines[a0 + k].text);
            lines[d0 + k].words = o;
            lines[a0 + k].words = n;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffMode {
    #[default]
    Unified,
    Split,
}

#[derive(Debug, Clone)]
pub enum DiffViewMessage {
    SetMode(DiffMode),
    /// Expand the collapsed region `region` of file `file`.
    Expand { file: usize, region: usize },
    Scrolled(Viewport),
    Key(iced::keyboard::Key),
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiffViewEvent {
    /// The user switched modes (persist it if you like).
    ModeChanged(DiffMode),
}

#[derive(Debug, Clone, Copy)]
enum Row {
    File(usize),
    Note(usize),
    Hunk { file: usize, seg: usize },
    Gap { file: usize, region: usize, count: usize },
    /// A collapsed region right before a hunk: one row with the expand control in the gutter
    /// and the hunk header (GitHub style).
    GapHunk {
        file: usize,
        region: usize,
        count: usize,
        seg: usize,
    },
    /// Unified: one line.
    Line { file: usize, line: usize },
    /// Side by side: old / new line (either may be empty).
    Pair {
        file: usize,
        old: Option<usize>,
        new: Option<usize>,
    },
    NoNewline,
}

/// The diff view's state.
pub struct DiffView {
    files: Vec<Prepared>,
    mode: DiffMode,
    expanded: HashSet<(usize, usize)>,
    rows: Vec<Row>,
    offsets: Vec<f32>,
    list: VList,
    no_width: f32,
    content_width: f32,
}

impl DiffView {
    pub fn new(files: Vec<DiffFile>) -> Self {
        let mut v = Self {
            files: Vec::new(),
            mode: DiffMode::Unified,
            expanded: HashSet::new(),
            rows: Vec::new(),
            offsets: vec![0.0],
            list: VList::default(),
            no_width: 0.0,
            content_width: 0.0,
        };
        v.set_files(files);
        v
    }

    /// Replaces the files (keeps the mode; expanded regions reset).
    pub fn set_files(&mut self, files: Vec<DiffFile>) {
        self.files = files.into_iter().map(prepare).collect();
        self.expanded.clear();
        let mut max_no = 0;
        let mut longest = 0usize;
        for p in &self.files {
            for l in &p.lines {
                max_no = max_no.max(l.old_no.unwrap_or(0)).max(l.new_no.unwrap_or(0));
                longest = longest.max(display_width(&l.text));
            }
        }
        let digits = max_no.max(1).ilog10() as f32 + 1.0;
        self.no_width = (digits * theme::CODE_SIZE * 0.6 + 16.0).ceil().max(36.0);
        self.content_width = 2.0 * self.no_width + 16.0 + longest as f32 * theme::CODE_SIZE * 0.6 + 24.0;
        self.rebuild();
    }

    pub fn mode(&self) -> DiffMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: DiffMode) {
        if self.mode != mode {
            self.mode = mode;
            self.rebuild();
        }
    }

    /// Number of rows (for tests / logs).
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Collapsed regions not yet expanded, as (file, region, hidden line count).
    pub fn collapsed(&self) -> Vec<(usize, usize, usize)> {
        self.rows
            .iter()
            .filter_map(|r| match r {
                Row::Gap {
                    file,
                    region,
                    count,
                }
                | Row::GapHunk {
                    file,
                    region,
                    count,
                    ..
                } => Some((*file, *region, *count)),
                _ => None,
            })
            .collect()
    }

    pub fn update(&mut self, msg: DiffViewMessage) -> (Task<DiffViewMessage>, Option<DiffViewEvent>) {
        match msg {
            DiffViewMessage::SetMode(m) => {
                if m == self.mode {
                    return (Task::none(), None);
                }
                self.set_mode(m);
                (Task::none(), Some(DiffViewEvent::ModeChanged(m)))
            }
            DiffViewMessage::Expand { file, region } => {
                self.expanded.insert((file, region));
                self.rebuild();
                (Task::none(), None)
            }
            DiffViewMessage::Scrolled(v) => {
                self.list.on_scroll(&v);
                (Task::none(), None)
            }
            DiffViewMessage::Key(k) => {
                use iced::keyboard::{Key, key::Named};
                let step = match k {
                    Key::Named(Named::ArrowDown) => LINE_H * 3.0,
                    Key::Named(Named::ArrowUp) => -LINE_H * 3.0,
                    Key::Named(Named::PageDown) => self.list.height() - LINE_H,
                    Key::Named(Named::PageUp) => -(self.list.height() - LINE_H),
                    Key::Named(Named::Home) => -f32::INFINITY,
                    Key::Named(Named::End) => f32::INFINITY,
                    _ => return (Task::none(), None),
                };
                let max = (self.offsets.last().copied().unwrap_or(0.0) - self.list.height()).max(0.0);
                let y = (self.list.offset() + step).clamp(0.0, max);
                (self.list.scroll_to(y), None)
            }
        }
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        let multi = self.files.len() > 1;
        for (fi, p) in self.files.iter().enumerate() {
            if multi {
                rows.push(Row::File(fi));
            }
            if p.note.is_some() {
                rows.push(Row::Note(fi));
                continue;
            }
            // Visible line runs, merged so that side-by-side pairing works across segments.
            let mut run: Vec<usize> = Vec::new();
            let flush = |run: &mut Vec<usize>, rows: &mut Vec<Row>, mode: DiffMode, p: &Prepared| {
                push_lines(rows, fi, run, mode, p);
                run.clear();
            };
            for (si, seg) in p.segments.iter().enumerate() {
                match seg {
                    Segment::Lines(r) => run.extend(r.clone()),
                    Segment::Collapsible { id, lines } => {
                        if self.expanded.contains(&(fi, *id)) {
                            run.extend(lines.clone());
                        } else {
                            flush(&mut run, &mut rows, self.mode, p);
                            rows.push(Row::Gap {
                                file: fi,
                                region: *id,
                                count: lines.len(),
                            });
                        }
                    }
                    Segment::Hunk(_) => {
                        flush(&mut run, &mut rows, self.mode, p);
                        match rows.last().copied() {
                            Some(Row::Gap { file, region, count }) if file == fi => {
                                rows.pop();
                                rows.push(Row::GapHunk {
                                    file,
                                    region,
                                    count,
                                    seg: si,
                                });
                            }
                            _ => rows.push(Row::Hunk { file: fi, seg: si }),
                        }
                    }
                }
            }
            flush(&mut run, &mut rows, self.mode, p);
        }
        let mut offsets = Vec::with_capacity(rows.len() + 1);
        let mut y = 0.0;
        for r in &rows {
            offsets.push(y);
            y += match r {
                Row::File(_) => HEAD_H,
                Row::Note(_) => NOTE_H,
                Row::Gap { .. } | Row::Hunk { .. } | Row::GapHunk { .. } => LINE_H + 4.0,
                _ => LINE_H,
            };
        }
        offsets.push(y);
        self.rows = rows;
        self.offsets = offsets;
    }

    pub fn view(&self) -> Element<'_, DiffViewMessage> {
        let width = (self.mode == DiffMode::Unified).then_some(self.content_width);
        let list = self.list.view_var(
            &self.offsets,
            width,
            move |i| self.row_view(i),
            DiffViewMessage::Scrolled,
        );
        let list = Interact::new(list).on_key(|k, _| Some(DiffViewMessage::Key(k.clone())));
        let body = container(list)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::bg(theme::BG));
        if self.files.len() == 1 {
            iced::widget::column![self.file_head(0), body].boxed()
        } else if self.files.is_empty() {
            container(text(tr::diff_no_changes()).size(13).color(theme::DIM))
                .padding(16)
                .boxed()
        } else {
            body.boxed()
        }
    }

    fn file_head(&self, fi: usize) -> Element<'_, DiffViewMessage> {
        let p = &self.files[fi];
        let f = &p.file;
        let path = match &f.old_path {
            Some(o) if *o != f.new_path => format!("{o} → {}", f.new_path),
            _ => f.new_path.clone(),
        };
        let stat = |s: String, c: Color| {
            container(text(s).size(11).font(theme::mono_bold()))
                .padding([1, 6])
                .style(theme::badge(c))
                .boxed()
        };
        let mode_btn = |glyph: &'static str, m: DiffMode, label: String| {
            theme::tip(
                button(text(glyph).size(13).width(14).align_x(Alignment::Center))
                    .padding([3, 5])
                    .style(theme::toggle(self.mode == m))
                    .on_press(DiffViewMessage::SetMode(m))
                    .boxed(),
                label,
            )
        };
        container(
            row![
                icons::file_icon(&f.new_path),
                text(path)
                    .size(13)
                    .font(theme::mono_bold())
                    .wrapping(text::Wrapping::None)
                    .color(theme::TEXT),
                stat(format!("+{}", f.added()), theme::STAT_ADD),
                stat(format!("−{}", f.removed()), theme::STAT_DEL),
                space().width(Length::Fill),
                mode_btn("☰", DiffMode::Unified, tr::diff_unified()),
                mode_btn("◫", DiffMode::Split, tr::diff_split()),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([0, 10])
        .height(HEAD_H)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .style(|t| {
            let mut s = theme::header_bar(t);
            s.border = iced::Border {
                color: theme::BORDER,
                width: 1.0,
                radius: 0.0.into(),
            };
            s
        })
        .boxed()
    }

    fn row_view(&self, i: usize) -> Element<'_, DiffViewMessage> {
        match self.rows[i] {
            Row::File(fi) => self.file_head(fi),
            Row::Note(fi) => container(
                text(self.files[fi].note.clone().unwrap_or_default())
                    .size(13)
                    .color(theme::DIM),
            )
            .padding([10, 16])
            .boxed(),
            Row::Hunk { file, seg } => {
                let Segment::Hunk(h) = &self.files[file].segments[seg] else {
                    unreachable!()
                };
                container(
                    text(h.as_str())
                        .size(theme::CODE_SIZE)
                        .font(theme::mono())
                        .color(theme::HUNK_TEXT)
                        .wrapping(text::Wrapping::None),
                )
                .padding([2, 12])
                .width(Length::Fill)
                .height(Length::Fill)
                .align_y(Alignment::Center)
                .style(theme::bg(theme::HUNK_BG))
                .boxed()
            }
            Row::Gap {
                file,
                region,
                count,
            } => self.expand_row(file, region, count, None),
            Row::GapHunk {
                file,
                region,
                count,
                seg,
            } => {
                let Segment::Hunk(h) = &self.files[file].segments[seg] else {
                    unreachable!()
                };
                self.expand_row(file, region, count, Some(h.as_str()))
            }
            Row::Line { file, line } => self.unified_line(&self.files[file].lines[line]),
            Row::Pair { file, old, new } => {
                let p = &self.files[file];
                row![
                    self.half(old.map(|i| &p.lines[i]), true),
                    container(space()).width(1).height(Length::Fill).style(theme::bg(theme::BORDER)),
                    self.half(new.map(|i| &p.lines[i]), false),
                ]
                .height(Length::Fill)
                .boxed()
            }
            Row::NoNewline => row![
                space().width(self.no_width * 2.0 + 16.0),
                text(format!("⊘ {}", tr::diff_no_newline()))
                    .size(11)
                    .color(theme::DIM),
            ]
            .height(Length::Fill)
            .align_y(Alignment::Center)
            .boxed(),
        }
    }

    /// Collapsed unchanged lines: an expand glyph in the gutter, then the hunk header that
    /// follows (tooltip with the line count) or the "Show N unchanged lines" label.
    fn expand_row<'a>(&'a self, file: usize, region: usize, count: usize, hunk: Option<&'a str>) -> Element<'a, DiffViewMessage> {
        let label = tr::diff_expand_lines(count as i64);
        let body: Element<'a, DiffViewMessage> = match hunk {
            Some(h) => text(h)
                .size(theme::CODE_SIZE)
                .font(theme::mono())
                .wrapping(text::Wrapping::None)
                .boxed(),
            None => text(label.clone()).size(12).boxed(),
        };
        let b = button(
            row![
                text("⇕")
                    .size(13)
                    .width(self.no_width * 2.0)
                    .align_x(Alignment::Center),
                body,
            ]
            .padding(iced::Padding {
                right: 12.0,
                ..iced::Padding::ZERO
            })
            .align_y(Alignment::Center),
        )
        .padding([2, 0])
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_, status| button::Style {
            background: Some(
                match status {
                    button::Status::Hovered | button::Status::Pressed => theme::HUNK_HOVER_BG,
                    _ => theme::HUNK_BG,
                }
                .into(),
            ),
            text_color: theme::HUNK_TEXT,
            ..Default::default()
        })
        .on_press(DiffViewMessage::Expand { file, region })
        .boxed();
        match hunk {
            Some(_) => theme::tip(b, label),
            None => b,
        }
    }

    fn number(&self, n: Option<u32>, bg: Option<Color>) -> Element<'_, DiffViewMessage> {
        container(
            text(n.map(|n| n.to_string()).unwrap_or_default())
                .size(theme::CODE_SIZE)
                .font(theme::mono())
                .color(theme::LINE_NO),
        )
        .width(self.no_width)
        .height(Length::Fill)
        .padding([0, 8])
        .align_x(Alignment::End)
        .align_y(Alignment::Center)
        .style(move |_| iced::widget::container::Style {
            background: bg.map(Into::into),
            ..Default::default()
        })
        .boxed()
    }

    fn unified_line<'a>(&'a self, l: &'a DLine) -> Element<'a, DiffViewMessage> {
        let (bg, no_bg, sign) = line_colors(l.kind);
        let row = row![
            self.number(l.old_no, no_bg),
            self.number(l.new_no, no_bg),
            text(sign)
                .size(theme::CODE_SIZE)
                .font(theme::mono())
                .color(theme::LINE_NO)
                .width(16)
                .align_x(Alignment::Center),
            code_text(l),
        ]
        .height(Length::Fill)
        .align_y(Alignment::Center);
        wrap_bg(row.boxed(), bg)
    }

    fn half<'a>(&'a self, l: Option<&'a DLine>, old: bool) -> Element<'a, DiffViewMessage> {
        let Some(l) = l else {
            return container(space())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::bg(theme::FILLER_BG))
                .boxed();
        };
        let (bg, no_bg, sign) = line_colors(l.kind);
        let n = if old { l.old_no } else { l.new_no };
        container(
            row![
                self.number(n, no_bg),
                text(sign)
                    .size(theme::CODE_SIZE)
                    .font(theme::mono())
                    .color(theme::LINE_NO)
                    .width(16)
                    .align_x(Alignment::Center),
                code_text(l),
            ]
            .height(Length::Fill)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .clip(true)
        .style(move |_| iced::widget::container::Style {
            background: bg.map(Into::into),
            ..Default::default()
        })
        .boxed()
    }
}

fn wrap_bg<'a>(content: Element<'a, DiffViewMessage>, bg: Option<Color>) -> Element<'a, DiffViewMessage> {
    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| iced::widget::container::Style {
            background: bg.map(Into::into),
            ..Default::default()
        })
        .boxed()
}

fn line_colors(kind: LineKind) -> (Option<Color>, Option<Color>, &'static str) {
    match kind {
        LineKind::Add => (Some(theme::ADD_BG), Some(theme::ADD_NO_BG), "+"),
        LineKind::Del => (Some(theme::DEL_BG), Some(theme::DEL_NO_BG), "-"),
        LineKind::Context => (None, None, " "),
    }
}

/// Code of one line: syntax colours plus word-change backgrounds.
fn code_text(l: &DLine) -> Element<'_, DiffViewMessage> {
    let word_bg = match l.kind {
        LineKind::Add => theme::ADD_WORD_BG,
        _ => theme::DEL_WORD_BG,
    };
    let t = l.text.as_str();
    let mut cuts: Vec<usize> = vec![0, t.len()];
    for (r, _) in &l.hl {
        cuts.push(r.start.min(t.len()));
        cuts.push(r.end.min(t.len()));
    }
    for r in &l.words {
        cuts.push(r.start.min(t.len()));
        cuts.push(r.end.min(t.len()));
    }
    cuts.retain(|&c| t.is_char_boundary(c));
    cuts.sort_unstable();
    cuts.dedup();
    let mut spans: Vec<Span<'_, ()>> = Vec::with_capacity(cuts.len());
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a == b {
            continue;
        }
        let color = l
            .hl
            .iter()
            .find(|(r, _)| r.start <= a && b <= r.end)
            .and_then(|(_, c)| syntax::code_color(*c))
            .unwrap_or(theme::CODE_TEXT);
        let mut s = Span::new(&t[a..b]).color(color);
        if l.words.iter().any(|r| r.start <= a && b <= r.end) {
            s = s.background(word_bg).border(iced::Border {
                radius: 2.0.into(),
                ..Default::default()
            });
        }
        spans.push(s);
    }
    rich_text(spans)
        .size(theme::CODE_SIZE)
        .font(theme::mono())
        .wrapping(text::Wrapping::None)
        .boxed()
}

/// Rows of a run of visible lines (+ no-newline markers).
fn push_lines(rows: &mut Vec<Row>, file: usize, run: &[usize], mode: DiffMode, p: &Prepared) {
    match mode {
        DiffMode::Unified => {
            for &i in run {
                rows.push(Row::Line { file, line: i });
                if p.lines[i].no_newline {
                    rows.push(Row::NoNewline);
                }
            }
        }
        DiffMode::Split => {
            let mut k = 0;
            while k < run.len() {
                let i = run[k];
                match p.lines[i].kind {
                    LineKind::Context => {
                        rows.push(Row::Pair {
                            file,
                            old: Some(i),
                            new: Some(i),
                        });
                        k += 1;
                    }
                    _ => {
                        let mut dels = Vec::new();
                        while k < run.len() && p.lines[run[k]].kind == LineKind::Del {
                            dels.push(run[k]);
                            k += 1;
                        }
                        let mut adds = Vec::new();
                        while k < run.len() && p.lines[run[k]].kind == LineKind::Add {
                            adds.push(run[k]);
                            k += 1;
                        }
                        for j in 0..dels.len().max(adds.len()) {
                            rows.push(Row::Pair {
                                file,
                                old: dels.get(j).copied(),
                                new: adds.get(j).copied(),
                            });
                        }
                    }
                }
            }
            if let Some(&last) = run.last()
                && p.lines[last].no_newline
            {
                rows.push(Row::NoNewline);
            }
        }
    }
}

/// Approximate display width in monospace cells (wide CJK counts as two).
fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| match c {
            '\t' => 4,
            '\u{1100}'..='\u{115F}' | '\u{2E80}'..='\u{A4CF}' | '\u{AC00}'..='\u{D7A3}' | '\u{F900}'..='\u{FAFF}' | '\u{FE30}'..='\u{FE4F}' | '\u{FF00}'..='\u{FF60}' | '\u{FFE0}'..='\u{FFE6}' => 2,
            _ => 1,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    #![allow(unused_must_use)]
    use super::*;

    const SAMPLE: &str = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,12 +1,12 @@ fn main\n fn main() {\n-    let x = 1;\n+    let x = 2;\n     a();\n     b();\n     c();\n     d();\n     e();\n     f();\n     g();\n     h();\n-}\n\\ No newline at end of file\n+}\n";

    #[test]
    fn parses_unified() {
        let files = DiffFile::parse_unified(SAMPLE);
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.new_path, "src/a.rs");
        assert_eq!(f.old_path, None);
        assert_eq!((f.added(), f.removed()), (2, 2));
        let h = &f.hunks[0];
        assert_eq!((h.old_start, h.new_start, h.section.as_str()), (1, 1, "fn main"));
        assert_eq!(h.lines[1].kind, LineKind::Del);
        assert_eq!(h.lines[1].old_no, Some(2));
        assert_eq!(h.lines[2].new_no, Some(2));
        assert!(h.lines[11].no_newline);
        assert!(!h.lines[12].no_newline);
    }

    #[test]
    fn word_highlights_only_changed_words() {
        let (a, b) = word_changes("    let x = 1;", "    let x = 2;");
        assert_eq!(a, vec![12..13]);
        assert_eq!(b, vec![12..13]);
        // Completely different lines: no word highlights.
        let (a, b) = word_changes("alpha beta", "gamma delta epsilon");
        assert!(a.is_empty() && b.is_empty());
    }

    #[test]
    fn collapses_long_context_and_expands() {
        let mut v = DiffView::new(DiffFile::parse_unified(SAMPLE));
        // 14 lines + hunk header + no-newline marker, 8 context lines -> 2 hidden (kept 3+3): not
        // collapsed (needs > 2).
        assert!(v.collapsed().is_empty());
        let mut text = String::from("--- a/x.txt\n+++ b/x.txt\n@@ -1,20 +1,20 @@\n-a\n+b\n");
        for i in 0..18 {
            text.push_str(&format!(" ctx {i}\n"));
        }
        text.push_str("-y\n+z\n");
        v.set_files(DiffFile::parse_unified(&text));
        let c = v.collapsed();
        assert_eq!(c, vec![(0, 0, 12)]);
        let before = v.row_count();
        v.update(DiffViewMessage::Expand { file: 0, region: 0 });
        assert_eq!(v.row_count(), before - 1 + 12);
        v.update(DiffViewMessage::SetMode(DiffMode::Split));
        // Split: 2 header-ish rows... pairs: (a,b), 18 ctx, (y,z) + hunk header.
        assert_eq!(v.row_count(), 1 + 1 + 18 + 1);
    }

    #[test]
    fn gaps_between_hunks_use_full_file() {
        let text = "--- a/f.txt\n+++ b/f.txt\n@@ -2,1 +2,1 @@\n-b\n+B\n@@ -9,1 +9,1 @@\n-i\n+I\n";
        let mut files = DiffFile::parse_unified(text);
        files[0].new_lines = Some(
            ["a", "B", "c", "d", "e", "f", "g", "h", "I", "j"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        );
        let v = DiffView::new(files);
        // Before hunk 1: line 1; between: 3..=8 (6 lines); after: 10.
        assert_eq!(v.collapsed(), vec![(0, 0, 1), (0, 1, 6), (0, 2, 1)]);
    }

    #[test]
    fn binary_and_rename_notes() {
        let text = "diff --git a/x.png b/x.png\nBinary files a/x.png and b/x.png differ\ndiff --git a/old.rs b/new.rs\nsimilarity index 100%\nrename from old.rs\nrename to new.rs\n";
        let files = DiffFile::parse_unified(text);
        assert_eq!(files.len(), 2);
        assert!(files[0].binary);
        assert_eq!(files[1].old_path.as_deref(), Some("old.rs"));
        assert_eq!(files[1].new_path, "new.rs");
        let v = DiffView::new(files);
        assert_eq!(v.row_count(), 4);
    }
}
