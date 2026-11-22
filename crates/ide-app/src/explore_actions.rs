//! Explore's menu and drag actions beyond create / rename (Hermes Desk's `ExploreTree.tsx`):
//! 입력창에 첨부, 잘라내기 / 복사 / 붙여넣기, OS 탐색기에서 보기, 경로 / 상대 경로 / 원격 파일 URL
//! 복사, delete to the recycle bin, and dragging entries (into a folder of the tree: a move;
//! anywhere else: the input box / the agent's input, `composer.rs`).
//!
//! - Cut / copy remember the entries (session, repo, root-relative paths) and, with the server
//!   on this machine, also put the files on the OS clipboard (Windows Explorer can paste them).
//! - Paste reads the OS clipboard first: files copied in the file manager (not ours) are copied
//!   in from their paths (a local server only); else the remembered entries are moved (cut,
//!   then forgotten) or copied (`fs.copy`, a taken name gets ` copy`).

use std::path::PathBuf;

use iced::Task;
use ide_client::ProtoError;
use ide_views::TreeEvent;

use crate::app::{App, Message};
use crate::detail::{DetailMsg, FsOp, file_name, join, msg};
use crate::i18n::{self, tr};

/// Entries cut or copied in Explore.
#[derive(Debug, Clone)]
pub struct ExploreClip {
    pub session: String,
    pub repo: String,
    /// Root-relative paths.
    pub paths: Vec<String>,
    /// Their absolute server paths (to tell our OS clipboard entries from others).
    pub abs: Vec<String>,
    pub cut: bool,
}

impl App {
    /// The absolute server path of `rel` in repo `repo` of session `sid` (its worktree).
    pub(crate) fn explore_abs(&self, sid: &str, repo: &str, rel: &str) -> Option<String> {
        let base = self
            .live(sid)
            .and_then(|l| l.info.repos.iter().find(|x| x.repo == repo))
            .map(|x| x.path.clone())?;
        let sep = if base.contains('\\') { '\\' } else { '/' };
        let base = base.trim_end_matches(['/', '\\']);
        if rel.is_empty() {
            return Some(base.to_owned());
        }
        let rel = if sep == '\\' {
            rel.replace('/', "\\")
        } else {
            rel.to_owned()
        };
        Some(format!("{base}{sep}{rel}"))
    }

    fn explore_toast(&mut self, sid: &str, text: String) {
        if let Some(d) = self.detail_mut(sid) {
            d.toast(text);
        }
    }

    /// Explore's menu / drag events beyond create and rename.
    pub(crate) fn on_explore_event(&mut self, sid: &str, repo: &str, ev: TreeEvent) -> Task<Message> {
        let abs = |this: &Self, rels: &[String]| -> Vec<String> {
            rels.iter()
                .filter_map(|r| this.explore_abs(sid, repo, r))
                .collect()
        };
        let is_cut = matches!(ev, TreeEvent::Cut(_));
        match ev {
            TreeEvent::DragStart(paths) => {
                let list = abs(self, &paths);
                crate::script::log(&format!("drag from the tree: {list:?}"));
                self.file_drag = (!list.is_empty()).then(|| (sid.to_owned(), list));
                Task::none()
            }
            TreeEvent::DragEnd => self.end_file_drag(),
            TreeEvent::Move { paths, to_dir } => {
                // A move inside the tree is not a drop into the input box.
                self.file_drag = None;
                self.drop_target = None;
                self.drag_pointer = None;
                self.explore_move(sid, repo, paths, &to_dir)
            }
            TreeEvent::Attach(paths) => {
                let dest = self.attach_dest(sid);
                let list = abs(self, &paths);
                Task::batch(
                    list.into_iter()
                        .map(|p| self.deliver(sid, &p, dest))
                        .collect::<Vec<_>>(),
                )
            }
            TreeEvent::Cut(paths) | TreeEvent::Copy(paths) if !paths.is_empty() => {
                let cut = is_cut;
                let list = abs(self, &paths);
                let n = paths.len() as i64;
                self.explore_clip = Some(ExploreClip {
                    session: sid.to_owned(),
                    repo: repo.to_owned(),
                    paths,
                    abs: list.clone(),
                    cut,
                });
                self.explore_toast(
                    sid,
                    if cut {
                        tr::explore_cut_done(n)
                    } else {
                        tr::explore_copied_done(n)
                    },
                );
                // The server's files are this machine's: other apps can paste them too.
                if self.target.local {
                    let files: Vec<PathBuf> = list.into_iter().map(PathBuf::from).collect();
                    iced::clipboard::write(iced::clipboard::Content::Files(files)).discard()
                } else {
                    Task::none()
                }
            }
            TreeEvent::Cut(_) | TreeEvent::Copy(_) => Task::none(),
            TreeEvent::Paste(dir) => {
                let (s, r) = (sid.to_owned(), repo.to_owned());
                iced::clipboard::read_files().map(move |files| {
                    msg(
                        &s,
                        DetailMsg::ExplorePaste(r.clone(), dir.clone(), files.ok().map(|f| f.to_vec())),
                    )
                })
            }
            TreeEvent::Reveal(path) => {
                if !self.target.local {
                    self.explore_toast(sid, tr::explore_reveal_remote());
                    return Task::none();
                }
                let Some(p) = self.explore_abs(sid, repo, path.as_deref().unwrap_or("")) else {
                    return Task::none();
                };
                if let Err(e) = crate::item_menu::reveal_in_file_manager(&p) {
                    self.explore_toast(sid, tr::item_reveal_os_failed(&e));
                }
                Task::none()
            }
            TreeEvent::CopyPath(path) => {
                let Some(p) = self.explore_abs(sid, repo, &path) else {
                    return Task::none();
                };
                self.explore_toast(sid, tr::item_copied_path());
                iced::clipboard::write(p).discard()
            }
            TreeEvent::CopyRelPath(path) => {
                self.explore_toast(sid, tr::item_copied_rel());
                iced::clipboard::write(path).discard()
            }
            TreeEvent::CopyRemoteUrl(path) => {
                let Some(c) = self.online() else {
                    return Task::none();
                };
                let (s, r, s2) = (sid.to_owned(), repo.to_owned(), sid.to_owned());
                Task::perform(
                    async move { c.git_remote_url(&s, &r, &path).await },
                    move |res| msg(&s2, DetailMsg::RemoteUrl(res)),
                )
            }
            _ => Task::none(),
        }
    }

    /// Move entries into `to_dir` (one rename each; the tree follows each result).
    fn explore_move(&mut self, sid: &str, repo: &str, paths: Vec<String>, to_dir: &str) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        crate::script::log(&format!("move {paths:?} -> {to_dir:?}"));
        let tasks: Vec<Task<Message>> = paths
            .into_iter()
            .map(|from| {
                let to = join(to_dir, file_name(&from));
                let op = FsOp::Renamed {
                    from: from.clone(),
                    to: to.clone(),
                };
                let (c, s, r, s2, r2) = (c.clone(), sid.to_owned(), repo.to_owned(), sid.to_owned(), repo.to_owned());
                Task::perform(async move { c.fs_rename(&s, &r, &from, &to).await }, move |res| {
                    msg(&s2, DetailMsg::FsDone(r2.clone(), op.clone(), res))
                })
            })
            .collect();
        Task::batch(tasks)
    }

    /// Explore's 붙여넣기 into `dir`, with what the OS clipboard held.
    pub(crate) fn explore_paste(
        &mut self,
        sid: &str,
        repo: &str,
        dir: &str,
        os_files: Option<Vec<PathBuf>>,
    ) -> Task<Message> {
        if self.online().is_none() {
            return Task::none();
        }
        let os: Vec<String> = os_files
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.display().to_string())
            .collect();
        let ours = self
            .explore_clip
            .as_ref()
            .is_some_and(|k| !os.is_empty() && k.abs == os);
        // Files copied in the file manager (not ours): copied in from their paths.
        if !os.is_empty() && !ours {
            if !self.target.local {
                self.explore_toast(sid, tr::explore_paste_remote());
                return Task::none();
            }
            return self.explore_copy(sid, repo, os, true, dir);
        }
        let Some(clip) = self.explore_clip.clone() else {
            return Task::none();
        };
        if clip.session != sid || clip.repo != repo {
            self.explore_toast(sid, tr::explore_paste_other_repo());
            return Task::none();
        }
        if clip.cut {
            // A cut moves once.
            self.explore_clip = None;
            let movable: Vec<String> = clip
                .paths
                .into_iter()
                .filter(|p| crate::detail::parent(p) != dir && p != dir)
                .collect();
            return self.explore_move(sid, repo, movable, dir);
        }
        self.explore_copy(sid, repo, clip.paths, false, dir)
    }

    /// Copy entries (root-relative, or absolute with `external`) into `dir`.
    fn explore_copy(&mut self, sid: &str, repo: &str, from: Vec<String>, external: bool, dir: &str) -> Task<Message> {
        let Some(c) = self.online() else {
            return Task::none();
        };
        crate::script::log(&format!("copy {from:?} -> {dir:?} (external {external})"));
        let tasks: Vec<Task<Message>> = from
            .into_iter()
            .map(|f| {
                let (c, s, r, d, s2, r2) = (
                    c.clone(),
                    sid.to_owned(),
                    repo.to_owned(),
                    dir.to_owned(),
                    sid.to_owned(),
                    repo.to_owned(),
                );
                Task::perform(async move { c.fs_copy(&s, &r, &f, external, &d).await }, move |res| {
                    // The tree lists the copy's folder (`is_dir`: nothing is opened).
                    let (op, res) = match res {
                        Ok(p) => (FsOp::Created { path: p, is_dir: true }, Ok(())),
                        Err(e) => (
                            FsOp::Created {
                                path: String::new(),
                                is_dir: true,
                            },
                            Err(e),
                        ),
                    };
                    msg(&s2, DetailMsg::FsDone(r2.clone(), op, res))
                })
            })
            .collect();
        Task::batch(tasks)
    }

    /// 원격 파일 URL 복사 answered: on the clipboard, or why not.
    pub(crate) fn remote_url_done(&mut self, sid: &str, r: Result<String, ProtoError>) -> Task<Message> {
        match r {
            Ok(url) => {
                self.explore_toast(sid, tr::explore_remote_url_copied());
                iced::clipboard::write(url).discard()
            }
            Err(e) => {
                self.explore_toast(sid, i18n::proto_error(&e));
                Task::none()
            }
        }
    }
}
