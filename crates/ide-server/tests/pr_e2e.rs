//! Pull requests and agent prompts end to end through `ide_client::Client`: `pr.*` against a
//! fake `gh` script (records its arguments and stdin, answers with canned `gh api --include`
//! output in call order; no real GitHub), prompts built for a session's worktree, and
//! `session.send_prompt` into a shell session and a paste-mode provider. Over the memory
//! transport (`ServerConfig::gh`) and a real `ide-server` child (`IDE_GH`).
//! Run with `IDE_TEST_TMP=<dir inside the workspace>`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::*;
use ide_client::{
    Client, ClientOptions, LocalChild, NewSession, PrExclusions, PrState, SessionInfo,
};
use ide_core::Paths;
use ide_server::{Server, ServerConfig};

const URL: &str = "https://github.com/acme/widgets";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A repo whose `origin` is `URL`, rewritten (`insteadOf`) to a local bare clone.
fn repo_with_origin(dir: &Path) -> PathBuf {
    let r = dir.join("widgets");
    init_repo(&r);
    let bare = dir.join("origin.git");
    let bare_s = bare.display().to_string().replace('\\', "/");
    let out = std::process::Command::new("git")
        .args(["clone", "-q", "--bare"])
        .arg(&r)
        .arg(&bare)
        .output()
        .unwrap();
    assert!(out.status.success());
    git(&r, &["remote", "add", "origin", URL]);
    git(&r, &["config", &format!("url.{bare_s}.insteadOf"), URL]);
    git(&r, &["fetch", "-q", "origin"]);
    r
}

/// A fake `gh`: call N appends its arguments to `args.log`, saves stdin to `stdinN.txt` and
/// prints `respN.txt`.
struct FakeGh {
    dir: PathBuf,
    program: PathBuf,
}

impl FakeGh {
    fn new(dir: &Path) -> Self {
        std::fs::create_dir_all(dir).unwrap();
        let program = if cfg!(windows) {
            let p = dir.join("gh.cmd");
            std::fs::write(
                &p,
                "@echo off\r\nsetlocal\r\nset n=0\r\nif exist \"%~dp0counter\" set /p n=<\"%~dp0counter\"\r\nset /a n=n+1\r\n>\"%~dp0counter\" echo %n%\r\n>>\"%~dp0args.log\" echo %*\r\n\"%SystemRoot%\\System32\\findstr.exe\" \"^\" > \"%~dp0stdin%n%.txt\"\r\ntype \"%~dp0resp%n%.txt\"\r\nexit /b 0\r\n",
            )
            .unwrap();
            p
        } else {
            let p = dir.join("gh");
            std::fs::write(
                &p,
                "#!/bin/sh\nd=$(dirname \"$0\")\nn=$(cat \"$d/counter\" 2>/dev/null || echo 0)\nn=$((n+1))\necho $n > \"$d/counter\"\necho \"$*\" >> \"$d/args.log\"\ncat > \"$d/stdin$n.txt\"\ncat \"$d/resp$n.txt\"\n",
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            p
        };
        FakeGh {
            dir: dir.to_path_buf(),
            program,
        }
    }
    /// Queue the answers of the next calls (numbered after the calls so far).
    fn answers(&self, bodies: &[(u16, &str)]) {
        let done = self.calls().len();
        for (i, (status, body)) in bodies.iter().enumerate() {
            std::fs::write(
                self.dir.join(format!("resp{}.txt", done + i + 1)),
                format!("HTTP/2.0 {status} X\r\nContent-Type: application/json\r\n\r\n{body}"),
            )
            .unwrap();
        }
    }
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("args.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| l.trim().to_owned())
            .collect()
    }
    fn stdin(&self, n: usize) -> String {
        std::fs::read_to_string(self.dir.join(format!("stdin{n}.txt"))).unwrap_or_default()
    }
}

fn pr_json(number: u64, branch: &str, state: &str) -> String {
    format!(
        r#"{{"number":{number},"state":"{state}","merged":false,"draft":false,"html_url":"{URL}/pull/{number}","title":"t","mergeable":true,"mergeable_state":"clean","head":{{"ref":"{branch}","sha":"{sha}","repo":{{"full_name":"acme/widgets"}}}},"base":{{"ref":"main","repo":{{"full_name":"acme/widgets"}}}}}}"#,
        sha = "a".repeat(40)
    )
}

async fn connect(server: &Arc<Server>) -> Client {
    Client::connect(
        memory(server),
        ClientOptions {
            reconnect: false,
            ..ClientOptions::default()
        },
    )
    .await
    .unwrap()
}

async fn session(c: &Client, provider: &str, repo: &str, original: bool) -> SessionInfo {
    let (s, failures) = c
        .create_session(NewSession {
            provider: provider.into(),
            account: None,
            configuration: "default".into(),
            repos: if original { vec![] } else { vec![repo.into()] },
            original_repo: original.then(|| repo.into()),
            refresh_base: false,
            bases: Default::default(),
            title: "pr".into(),
            start: false,
            cols: 100,
            rows: 30,
        })
        .await
        .expect("create session");
    assert!(failures.is_empty(), "{failures:?}");
    s
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pr_and_prompts_through_the_client() {
    let dir = tmp_dir("pr-e2e");
    let data = dir.join("data");
    let repo = repo_with_origin(&dir);
    let gh = FakeGh::new(&dir.join("fake-gh"));
    // A paste-mode provider whose program never turns bracketed paste on.
    std::fs::create_dir_all(data.join("providers")).unwrap();
    std::fs::write(
        data.join("providers").join("pastefake.toml"),
        format!(
            "id = \"pastefake\"\nname = \"Paste fake\"\nprogram = \"{}\"\nversion_args = []\n\n[prompt]\nmode = \"paste\"\nsubmit_delay_ms = 10\n",
            if cfg!(windows) { "cmd.exe" } else { "sh" }
        ),
    )
    .unwrap();
    let mut cfg = ServerConfig::for_paths(Paths::new(&data));
    cfg.gh = Some(gh.program.clone());
    let server = Server::new(cfg);
    let c = connect(&server).await;
    let col = collect(&c, Arc::default());
    let caps = c.welcome().unwrap().capabilities;
    assert!(caps.iter().any(|x| x == "pr") && caps.iter().any(|x| x == "send-prompt"));

    let r = c.register_repo(&repo.display().to_string()).await.unwrap();
    let s = session(&c, "shell", &r.id, false).await;
    let wt = &s.repos[0];
    let branch = wt.branch.clone();

    // Prompts for this worktree.
    let p = c
        .prompt_commit(&s.id, &r.id, vec!["README.md".into()], false)
        .await
        .unwrap();
    assert!(
        p.starts_with(&format!("[커밋 요청] {}\n작업 폴더: {}\n", r.name, wt.path)),
        "{p}"
    );
    assert!(p.ends_with("- README.md\n- push는 하지 마세요."));
    let pr_file = c.pr_reset_file(&s.id, &r.id).await.unwrap();
    assert!(
        Path::new(&pr_file).starts_with(&s.root) && pr_file.ends_with("pr-widgets.md"),
        "{pr_file}"
    );
    assert!(Path::new(&pr_file).parent().unwrap().is_dir());
    let p = c
        .prompt_pr(
            &s.id,
            &r.id,
            "main",
            PrExclusions {
                commits: true,
                files: false,
            },
        )
        .await
        .unwrap();
    assert!(p.starts_with(&format!("[PR 요청] {} → main\n", r.name)));
    assert!(p.contains(&format!(
        "3. PR 제목과 본문을 이 파일에 저장하세요: {pr_file}\n"
    )));
    assert!(p.contains(&format!(
        "앱은 push하지 않고 origin/{branch}로 PR을 만듭니다."
    )));
    assert_eq!(
        c.prompt_pr(&s.id, &r.id, "-x", PrExclusions::default())
            .await
            .unwrap_err()
            .id,
        "pr-bad-branch"
    );

    // Local state: nothing beyond origin yet.
    let pending = c.pr_pending_work(&s.id, &r.id).await.unwrap();
    assert_eq!(
        (
            pending.branch.as_str(),
            pending.commit_count,
            pending.published
        ),
        (branch.as_str(), 0, false)
    );
    let u = c.pr_unmerged(&s.id, &r.id).await.unwrap().unwrap();
    assert_eq!(u.commits, 0);
    let targets = c.pr_remote_branches(&s.id, &r.id).await.unwrap();
    assert_eq!(targets.branches, vec!["main".to_owned()]);
    assert_eq!(targets.current, branch);
    let left = c.pr_leftover(&s.id, &r.id, Some("main")).await.unwrap();
    assert_eq!((left.file, left.changes), (None, Some(false)));

    // pr.status through the fake gh: list, detail, reviews.
    let mine = pr_json(4, &branch, "open");
    gh.answers(&[
        (200, &format!("[{mine}]")),
        (200, &mine),
        (200, r#"[{"user":{"login":"a"},"state":"APPROVED"}]"#),
    ]);
    let st = c.pr_status(&s.id, &r.id).await.unwrap().unwrap();
    assert_eq!((st.number, st.state, st.approved), (4, PrState::Open, true));
    let calls = gh.calls();
    assert_eq!(calls.len(), 3);
    assert!(
        calls[0].starts_with(
            "api --hostname github.com --include -X GET repos/acme/widgets/pulls -f state=all"
        ),
        "{}",
        calls[0]
    );
    assert!(calls[0].contains(&format!("-f head=acme:{branch}")));
    assert_eq!(
        calls[2],
        "api --hostname github.com --include -X GET repos/acme/widgets/pulls/4/reviews -f per_page=100"
    );

    // pr.close: detail, then PATCH with the JSON body on stdin.
    gh.answers(&[(200, &mine), (200, r#"{"state":"closed"}"#)]);
    let closed = c.pr_close(&s.id, &r.id, 4).await.unwrap();
    assert_eq!(
        (closed.number, closed.url.as_str()),
        (4, format!("{URL}/pull/4").as_str())
    );
    assert_eq!(
        gh.calls()[4],
        "api --hostname github.com --include -X PATCH repos/acme/widgets/pulls/4 --input -"
    );
    assert_eq!(gh.stdin(5).trim(), r#"{"state":"closed"}"#);
    // GitHub refusing a merge comes back as an error id with the status.
    gh.answers(&[
        (200, &mine),
        (405, r#"{"message":"Base branch was modified"}"#),
    ]);
    let e = c
        .pr_merge(&s.id, &r.id, 4, &"a".repeat(40), Default::default(), false)
        .await
        .unwrap_err();
    assert_eq!(
        (e.id.as_str(), e.get("status")),
        ("pr-merge-refused", Some("405"))
    );
    assert_eq!(
        gh.stdin(7).trim(),
        format!(r#"{{"merge_method":"squash","sha":"{}"}}"#, "a".repeat(40))
    );

    // session.send_prompt: not running, then typed into the shell.
    assert_eq!(
        c.send_prompt(&s.id, "echo hi").await.unwrap_err().id,
        "session-not-running"
    );
    c.start_session(&s.id).await.unwrap();
    c.attach(&s.id, None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    c.send_prompt(&s.id, "echo PRMARK_1\x1b[31m\n")
        .await
        .unwrap();
    assert!(
        wait_until(Duration::from_secs(20), || term(&col, &s.id)
            .count("PRMARK_1")
            >= 2)
        .await,
        "{}",
        term(&col, &s.id).text()
    );
    assert!(!term(&col, &s.id).text().contains("PRMARK_1\x1b[31m"));
    assert_eq!(
        c.send_prompt(&s.id, " \x07 ").await.unwrap_err().id,
        "prompt-empty"
    );

    // A paste-mode provider whose terminal has no bracketed paste: refused, nothing typed.
    let tui = session(&c, "pastefake", &r.id, false).await;
    c.start_session(&tui.id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let e = c.send_prompt(&tui.id, "hello").await.unwrap_err();
    assert_eq!(e.id, "prompt-not-ready");

    // Original-repo sessions have no PR file (Desk: PR requests need a workspace session).
    let orig = session(&c, "shell", &r.id, true).await;
    assert_eq!(
        c.pr_reset_file(&orig.id, &r.id).await.unwrap_err().id,
        "pr-needs-workspace"
    );
    // Commit prompts work there too.
    assert!(
        c.prompt_commit(&orig.id, &r.id, vec![], true)
            .await
            .unwrap()
            .contains("변경사항 전체(0개 파일)를 커밋해 주세요.")
    );

    for id in [&s.id, &tui.id, &orig.id] {
        c.delete_session(id, true).await.unwrap();
    }
    drop(c);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// A real `ide-server` child resolves `gh` from `IDE_GH`; a missing program is `gh-missing`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gh_from_env_in_a_server_child() {
    let dir = tmp_dir("pr-child");
    let repo = repo_with_origin(&dir);
    let gh = FakeGh::new(&dir.join("fake-gh"));
    gh.answers(&[(200, "[]")]);
    let child = |data: &str, gh_path: &Path| {
        LocalChild::new(env!("CARGO_BIN_EXE_ide-server"))
            .no_daemon()
            .env("IDE_DATA_DIR", dir.join(data).display().to_string())
            .env("IDE_GH", gh_path.display().to_string())
    };
    let opts = || ClientOptions {
        reconnect: false,
        ..ClientOptions::default()
    };
    let c = Client::connect(child("data", &gh.program), opts())
        .await
        .unwrap();
    let r = c.register_repo(&repo.display().to_string()).await.unwrap();
    let s = session(&c, "shell", &r.id, false).await;
    assert_eq!(c.pr_status(&s.id, &r.id).await.unwrap(), None);
    assert_eq!(gh.calls().len(), 1);
    c.delete_session(&s.id, true).await.unwrap();
    drop(c);

    let c = Client::connect(child("data2", &dir.join("missing-gh.exe")), opts())
        .await
        .unwrap();
    let r = c.register_repo(&repo.display().to_string()).await.unwrap();
    let s = session(&c, "shell", &r.id, false).await;
    assert_eq!(
        c.pr_status(&s.id, &r.id).await.unwrap_err().id,
        "gh-missing"
    );
    c.delete_session(&s.id, true).await.unwrap();
    drop(c);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = std::fs::remove_dir_all(&dir);
}
