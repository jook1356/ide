//! Notification centre store (Hermes Desk's `lib/notices.ts`, docs/notifications.md).
//!
//! Every user-facing outcome that is not a question (toasts, failed background work, PR job
//! results, agent attention, ...) is a [`Notice`] here. The rail's bell shows the unread count
//! and opens the list (`notice_view.rs`); a notice with a toast also pops up for a few seconds.
//!
//! Kept free of widgets: notices carry the app's `Message` for their buttons, so the store is
//! generic over it and unit-tested with a plain type.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// Most notices kept (oldest dropped first; running ones are kept).
pub const MAX_NOTICES: usize = 200;

/// Notice tone (Desk `NoticeTone`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Success,
    Warning,
    Error,
}

impl Tone {
    /// How long a toast of this tone stays (Desk: errors 8 s, warnings 10 s, others 3 s; ide
    /// kept its old 5 s for the others).
    pub fn toast_for(self) -> Duration {
        match self {
            Tone::Info | Tone::Success => Duration::from_secs(5),
            Tone::Warning => Duration::from_secs(10),
            Tone::Error => Duration::from_secs(8),
        }
    }
}

/// Text of a notice: either fixed (user data, agent text, git output) or produced by a
/// translation call each time it is drawn, so a language switch also changes notices already
/// shown.
#[derive(Clone)]
pub enum Text {
    Plain(String),
    Tr(Arc<dyn Fn() -> String + Send + Sync>),
}

impl Text {
    /// A translated text: `Text::tr(move || tr::foo(&x))`.
    pub fn tr(f: impl Fn() -> String + Send + Sync + 'static) -> Self {
        Text::Tr(Arc::new(f))
    }

    pub fn get(&self) -> String {
        match self {
            Text::Plain(s) => s.clone(),
            Text::Tr(f) => f(),
        }
    }
}

impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.get())
    }
}

impl From<String> for Text {
    fn from(s: String) -> Self {
        Text::Plain(s)
    }
}

impl From<&str> for Text {
    fn from(s: &str) -> Self {
        Text::Plain(s.to_owned())
    }
}

/// Identity of a notice that follows one piece of work: pushing with a key that is already
/// listed changes that notice in place instead of adding another.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NoticeKey {
    /// A PR request of a session's repository (`footer.rs` `PrJob`).
    PrJob { session: String, repo: String },
    /// `RepoFooter::pr_error` of a session's repository.
    PrInfo { session: String, repo: String },
    /// The agent of a session needs the user, answered or failed.
    Attention { session: String },
    /// The terminal of a session rang its bell.
    Bell { session: String },
    /// Connection to the server.
    Conn,
    /// Self-update result.
    Update,
    /// Any other single-slot notice.
    Other(String),
}

/// Background work a notice follows (Desk `TaskState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Running,
    Done,
    Failed,
}

/// Look of a notice button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActionStyle {
    #[default]
    Normal,
    Primary,
    Danger,
}

#[derive(Debug, Clone)]
pub struct NoticeAction<M> {
    pub label: Text,
    pub msg: M,
    pub style: ActionStyle,
}

#[derive(Debug, Clone)]
pub struct Notice<M> {
    pub id: u64,
    pub key: Option<NoticeKey>,
    pub tone: Tone,
    pub title: Text,
    pub body: Option<Text>,
    /// The session it is about (chip in the list; cleared with the session).
    pub session: Option<String>,
    /// The repository it is about (shown next to the session).
    pub repo: Option<String>,
    pub at: SystemTime,
    pub read: bool,
    pub task: Option<TaskState>,
    /// Sent when the notice (or its toast) is clicked.
    pub open: Option<M>,
    pub actions: Vec<NoticeAction<M>>,
    /// Shown as a toast: pop it up when pushed.
    pub toast: bool,
    /// The toast is up until then.
    pub toast_until: Option<Instant>,
}

impl<M> Notice<M> {
    pub fn new(tone: Tone, title: impl Into<Text>) -> Self {
        Notice {
            id: 0,
            key: None,
            tone,
            title: title.into(),
            body: None,
            session: None,
            repo: None,
            at: SystemTime::now(),
            read: false,
            task: None,
            open: None,
            actions: Vec::new(),
            toast: false,
            toast_until: None,
        }
    }

    pub fn info(title: impl Into<Text>) -> Self {
        Self::new(Tone::Info, title)
    }

    pub fn success(title: impl Into<Text>) -> Self {
        Self::new(Tone::Success, title)
    }

    pub fn warning(title: impl Into<Text>) -> Self {
        Self::new(Tone::Warning, title)
    }

    pub fn error(title: impl Into<Text>) -> Self {
        Self::new(Tone::Error, title)
    }

    pub fn body(mut self, body: impl Into<Text>) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn session(mut self, sid: impl Into<String>) -> Self {
        self.session = Some(sid.into());
        self
    }

    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = Some(repo.into());
        self
    }

    pub fn key(mut self, key: NoticeKey) -> Self {
        self.key = Some(key);
        self
    }

    pub fn task(mut self, task: TaskState) -> Self {
        self.task = Some(task);
        self
    }

    pub fn open(mut self, msg: M) -> Self {
        self.open = Some(msg);
        self
    }

    pub fn action(mut self, label: impl Into<Text>, msg: M, style: ActionStyle) -> Self {
        self.actions.push(NoticeAction {
            label: label.into(),
            msg,
            style,
        });
        self
    }

    /// Also pop up as a toast.
    pub fn toast(mut self) -> Self {
        self.toast = true;
        self
    }

    /// Already read (history only, the badge does not count it).
    pub fn quiet(mut self) -> Self {
        self.read = true;
        self
    }

    pub fn running(&self) -> bool {
        self.task == Some(TaskState::Running)
    }

    pub fn toast_up(&self, now: Instant) -> bool {
        self.toast_until.is_some_and(|t| now < t)
    }
}

/// The notices, newest first.
#[derive(Debug)]
pub struct Notices<M> {
    items: Vec<Notice<M>>,
    next: u64,
}

impl<M> Default for Notices<M> {
    fn default() -> Self {
        Notices {
            items: Vec::new(),
            next: 1,
        }
    }
}

impl<M> Notices<M> {
    pub fn iter(&self) -> impl Iterator<Item = &Notice<M>> {
        self.items.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn get(&self, id: u64) -> Option<&Notice<M>> {
        self.items.iter().find(|n| n.id == id)
    }

    pub fn find_key(&self, key: &NoticeKey) -> Option<&Notice<M>> {
        self.items.iter().find(|n| n.key.as_ref() == Some(key))
    }

    /// Adds `n` at the top, or, when its key is listed, puts it in that notice's place (same
    /// id, same position). Returns the id.
    pub fn push(&mut self, mut n: Notice<M>) -> u64 {
        if n.toast {
            n.toast_until = Some(Instant::now() + n.tone.toast_for());
        }
        if let Some(key) = &n.key
            && let Some(old) = self.items.iter_mut().find(|o| o.key.as_ref() == Some(key))
        {
            n.id = old.id;
            // A quiet update keeps an earlier toast up.
            if !n.toast {
                n.toast_until = old.toast_until.filter(|_| !n.read);
            }
            *old = n;
            return old.id;
        }
        n.id = self.next;
        self.next += 1;
        let id = n.id;
        self.items.insert(0, n);
        self.trim();
        id
    }

    fn trim(&mut self) {
        while self.items.len() > MAX_NOTICES {
            match self.items.iter().rposition(|n| !n.running()) {
                Some(i) => {
                    self.items.remove(i);
                }
                None => break,
            }
        }
    }

    /// Changes a notice in place. Unless `quiet`, it becomes unread again (a result worth
    /// seeing; progress updates pass `quiet`). The app follows its work with keys (`push`).
    #[allow(dead_code)]
    pub fn update(&mut self, id: u64, quiet: bool, f: impl FnOnce(&mut Notice<M>)) {
        if let Some(n) = self.items.iter_mut().find(|n| n.id == id) {
            f(n);
            if !quiet {
                n.read = false;
            }
        }
    }

    pub fn remove(&mut self, id: u64) {
        self.items.retain(|n| n.id != id);
    }

    pub fn remove_key(&mut self, key: &NoticeKey) {
        self.items.retain(|n| n.key.as_ref() != Some(key));
    }

    /// A deleted session: its notices go too.
    pub fn forget_session(&mut self, sid: &str) {
        self.items.retain(|n| n.session.as_deref() != Some(sid));
    }

    pub fn mark_read(&mut self, id: u64) {
        if let Some(n) = self.items.iter_mut().find(|n| n.id == id) {
            n.read = true;
        }
    }

    pub fn mark_all_read(&mut self) {
        for n in &mut self.items {
            n.read = true;
        }
    }

    /// The user looks at `sid`: notices with `key` about it are read.
    pub fn mark_key_read(&mut self, key: &NoticeKey) {
        for n in &mut self.items {
            if n.key.as_ref() == Some(key) {
                n.read = true;
            }
        }
    }

    /// 지우기: everything but running work.
    pub fn clear_finished(&mut self) {
        self.items.retain(|n| n.running());
    }

    pub fn hide_toast(&mut self, id: u64) {
        if let Some(n) = self.items.iter_mut().find(|n| n.id == id) {
            n.toast_until = None;
        }
    }

    pub fn unread(&self) -> usize {
        self.items.iter().filter(|n| !n.read).count()
    }

    pub fn running(&self) -> usize {
        self.items.iter().filter(|n| n.running()).count()
    }

    /// Toasts up at `now`, oldest first (drawn bottom-up, newest at the bottom).
    pub fn toasts(&self, now: Instant) -> impl Iterator<Item = &Notice<M>> {
        self.items.iter().rev().filter(move |n| n.toast_up(now))
    }

    pub fn has_toast(&self) -> bool {
        self.items.iter().any(|n| n.toast_until.is_some())
    }

    /// Drops toasts whose time is over. Returns whether any went.
    pub fn expire(&mut self, now: Instant) -> bool {
        let mut any = false;
        for n in &mut self.items {
            if n.toast_until.is_some_and(|t| now >= t) {
                n.toast_until = None;
                any = true;
            }
        }
        any
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type N = Notice<u8>;

    #[test]
    fn push_puts_newest_first_and_counts_unread() {
        let mut s = Notices::<u8>::default();
        let a = s.push(N::info("a"));
        let b = s.push(N::error("b").quiet());
        let ids: Vec<u64> = s.iter().map(|n| n.id).collect();
        assert_eq!(ids, vec![b, a]);
        assert_eq!(s.unread(), 1);
        s.mark_all_read();
        assert_eq!(s.unread(), 0);
    }

    #[test]
    fn keyed_push_replaces_in_place() {
        let mut s = Notices::<u8>::default();
        let key = NoticeKey::Attention {
            session: "s1".into(),
        };
        let first = s.push(N::warning("needs input").key(key.clone()));
        s.push(N::info("other"));
        s.mark_read(first);
        let again = s.push(N::success("answered").key(key.clone()));
        assert_eq!(first, again);
        assert_eq!(s.iter().count(), 2);
        let n = s.get(first).unwrap();
        assert_eq!(n.title.get(), "answered");
        assert!(!n.read);
        // Still below the newer notice.
        assert_eq!(s.iter().last().unwrap().id, first);
    }

    #[test]
    fn update_is_unread_unless_quiet() {
        let mut s = Notices::<u8>::default();
        let id = s.push(N::info("job").task(TaskState::Running).quiet());
        s.update(id, true, |n| n.body = Some("step".into()));
        assert_eq!(s.unread(), 0);
        s.update(id, false, |n| n.task = Some(TaskState::Done));
        assert_eq!(s.unread(), 1);
        assert_eq!(s.running(), 0);
    }

    #[test]
    fn clear_keeps_running_and_trim_drops_oldest() {
        let mut s = Notices::<u8>::default();
        let run = s.push(N::info("run").task(TaskState::Running));
        for i in 0..MAX_NOTICES + 5 {
            s.push(N::info(format!("{i}")));
        }
        assert_eq!(s.iter().count(), MAX_NOTICES);
        assert!(s.get(run).is_some());
        assert_eq!(s.iter().next().unwrap().title.get(), format!("{}", MAX_NOTICES + 4));
        s.clear_finished();
        assert_eq!(s.iter().count(), 1);
    }

    #[test]
    fn forget_session_drops_its_notices() {
        let mut s = Notices::<u8>::default();
        s.push(N::info("a").session("s1"));
        s.push(N::info("b").session("s2"));
        s.push(N::info("c"));
        s.forget_session("s1");
        let left: Vec<String> = s.iter().map(|n| n.title.get()).collect();
        assert_eq!(left, vec!["c", "b"]);
    }

    #[test]
    fn toasts_expire() {
        let mut s = Notices::<u8>::default();
        let id = s.push(N::success("done").toast());
        let now = Instant::now();
        assert_eq!(s.toasts(now).count(), 1);
        assert!(s.has_toast());
        assert!(s.expire(now + Duration::from_secs(6)));
        assert!(!s.has_toast());
        assert!(s.get(id).is_some(), "history keeps it");
    }

    #[test]
    fn translated_text_is_produced_when_read() {
        let lang = Arc::new(std::sync::Mutex::new("en"));
        let l = lang.clone();
        let t = Text::tr(move || format!("lang {}", l.lock().unwrap()));
        assert_eq!(t.get(), "lang en");
        *lang.lock().unwrap() = "ko";
        assert_eq!(t.get(), "lang ko");
    }
}
