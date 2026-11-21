//! Unit tests of the PR backend: a local bare `origin` (reached through `insteadOf` from a
//! GitHub-like URL, so nothing touches the network) and an in-process fake GitHub API that
//! records every call and answers with canned JSON. No real `gh`, no real GitHub. The
//! transport tests run `git credential fill` against a scratch git config only (its own
//! `credential.helper`, `GIT_CONFIG_NOSYSTEM`), never the user's credential store, and a local
//! fake GitHub over HTTP.

use std::io::{BufRead, BufReader};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use super::*;

// ---- fixtures -----------------------------------------------------------------------------

const URL: &str = "https://github.com/acme/widgets";

fn tmp(tag: &str) -> PathBuf {
    let base = std::env::var_os("IDE_TEST_TMP")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("ide-server-tests"));
    let d = base.join(format!("pr-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn commit(dir: &Path, file: &str, text: &str, msg: &str) -> String {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
    git(dir, &["rev-parse", "HEAD"])
}

struct Fx {
    root: PathBuf,
    bare: PathBuf,
    work: PathBuf,
}

impl Fx {
    fn remote_sha(&self, branch: &str) -> Option<String> {
        let out = git(
            &self.bare,
            &[
                "for-each-ref",
                "--format=%(objectname)",
                &format!("refs/heads/{branch}"),
            ],
        );
        (!out.is_empty()).then_some(out)
    }
    fn pr_file(&self) -> PathBuf {
        ide_core::pr_file_path(&self.root.join("session"), "widgets")
    }
}

/// origin (bare, `main` with one commit), a clone whose `origin` is `URL` rewritten to the
/// bare repo, on branch `feature` with one more commit.
fn fixture(tag: &str) -> Fx {
    let root = tmp(tag);
    let bare = root.join("origin.git");
    let seed = root.join("seed");
    let work = root.join("work");
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare", "-b", "main"]);
    std::fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q", "-b", "main"]);
    git(&seed, &["config", "user.email", "t@example.com"]);
    git(&seed, &["config", "user.name", "t"]);
    git(&seed, &["config", "commit.gpgsign", "false"]);
    commit(&seed, "README.md", "hi\n", "init");
    let bare_s = bare.display().to_string().replace('\\', "/");
    git(&seed, &["push", "-q", &bare_s, "main"]);
    let out = Command::new("git")
        .args(["clone", "-q", &bare_s])
        .arg(&work)
        .output()
        .unwrap();
    assert!(out.status.success());
    git(&work, &["config", "user.email", "t@example.com"]);
    git(&work, &["config", "user.name", "t"]);
    git(&work, &["config", "commit.gpgsign", "false"]);
    git(&work, &["config", "core.autocrlf", "false"]);
    git(&work, &["config", "remote.origin.url", URL]);
    git(&work, &["config", &format!("url.{bare_s}.insteadOf"), URL]);
    git(&work, &["checkout", "-q", "-b", "feature"]);
    commit(&work, "a.txt", "a\n", "기능 A");
    Fx { root, bare, work }
}

// ---- fake GitHub --------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Call {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    body: Option<Value>,
}

impl Call {
    fn q(&self, k: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(a, _)| a == k)
            .map(|(_, v)| v.as_str())
    }
}

type Handler = Box<dyn Fn(&Call) -> Result<GhResponse, ProtoError> + Send + Sync>;

struct FakeGh {
    calls: Mutex<Vec<Call>>,
    handler: Handler,
}

impl FakeGh {
    fn new(f: impl Fn(&Call) -> Result<GhResponse, ProtoError> + Send + Sync + 'static) -> Self {
        FakeGh {
            calls: Mutex::new(Vec::new()),
            handler: Box::new(f),
        }
    }
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
    fn gh(&self) -> Gh<'_> {
        Ok(self as &dyn GhApi)
    }
}

impl GhApi for FakeGh {
    fn call(&self, req: &GhRequest<'_>) -> Result<GhResponse, ProtoError> {
        assert_eq!(req.host, "github.com");
        let c = Call {
            method: req.method.to_owned(),
            path: req.path.to_owned(),
            query: req
                .query
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
            body: req.body.cloned(),
        };
        self.calls.lock().unwrap().push(c.clone());
        (self.handler)(&c)
    }
}

fn r(status: u16, body: Value) -> Result<GhResponse, ProtoError> {
    Ok(GhResponse { status, body })
}

fn pr_json(number: u64, state: &str, head_sha: &str) -> Value {
    json!({
        "number": number,
        "state": state,
        "merged": false,
        "draft": false,
        "html_url": format!("{URL}/pull/{number}"),
        "title": "기능 A",
        "mergeable": true,
        "mergeable_state": "clean",
        "head": { "ref": "feature", "sha": head_sha, "repo": { "full_name": "acme/widgets" } },
        "base": { "ref": "main", "repo": { "full_name": "acme/widgets" } }
    })
}

// ---- pure parts ---------------------------------------------------------------------------

#[test]
fn origins_urls_and_gh_output() {
    let p = |u: &str| parse_origin(u).map(|r| (r.host, r.owner, r.repo, r.web));
    let gh = |o: &str| {
        Some((
            "github.com".to_owned(),
            o.to_owned(),
            "widgets".to_owned(),
            format!("https://github.com/{o}/widgets"),
        ))
    };
    assert_eq!(p("git@github.com:acme/widgets.git"), gh("acme"));
    assert_eq!(p("ssh://git@github.com/acme/widgets"), gh("acme"));
    assert_eq!(p("https://x:tok@github.com/acme/widgets.git\n"), gh("acme"));
    assert_eq!(
        p("https://gitlab.example.com/g/sub/proj").map(|t| t.1),
        Some("g/sub".to_owned())
    );
    assert_eq!(p("C:/repos/origin.git"), None);
    assert_eq!(p("https://github.com/widgets"), None);

    let repo = parse_origin(URL).unwrap();
    assert_eq!(
        compare_url(&repo, "release/1", "feature", "a b&c", "본문\n"),
        "https://github.com/acme/widgets/compare/release%2F1...feature?expand=1&title=a+b%26c&body=%EB%B3%B8%EB%AC%B8%0A"
    );
    let lab = parse_origin("https://gitlab.com/g/p").unwrap();
    assert!(compare_url(&lab, "main", "f", "t", "").starts_with(
        "https://gitlab.com/g/p/-/merge_requests/new?merge_request%5Bsource_branch%5D=f&"
    ));

    let out = b"HTTP/2.0 201 Created\r\nContent-Type: application/json\r\n\r\n{\"number\":5}";
    assert_eq!(
        parse_gh_output(out, ""),
        Some(GhResponse {
            status: 201,
            body: json!({"number": 5})
        })
    );
    assert_eq!(
        parse_gh_output(b"HTTP/2.0 204 No Content\nX: y\n\n", "").map(|r| (r.status, r.body)),
        Some((204, Value::Null))
    );
    assert_eq!(
        parse_gh_output(b"{\"message\":\"Not Found\"}", "gh: Not Found (HTTP 404)")
            .map(|r| r.status),
        Some(404)
    );
    assert_eq!(parse_gh_output(b"", "To get started with GitHub CLI"), None);
    let q = [("state", "all".to_owned())];
    let body = json!({});
    assert_eq!(
        GhCli::args(&GhRequest {
            host: "github.com",
            method: "GET",
            path: "repos/acme/widgets/pulls",
            query: &q,
            body: None,
        }),
        [
            "api",
            "--hostname",
            "github.com",
            "--include",
            "-X",
            "GET",
            "repos/acme/widgets/pulls",
            "-f",
            "state=all"
        ]
    );
    assert!(
        GhCli::args(&GhRequest {
            host: "h",
            method: "POST",
            path: "graphql",
            query: &[],
            body: Some(&body),
        })
        .ends_with(&["--input".to_owned(), "-".to_owned()])
    );

    let env = LaunchEnv::from_process();
    let d = tmp("gh-resolve");
    assert_eq!(
        resolve_gh(Some(&d.join("nope.exe")), &env).unwrap_err().id,
        "gh-missing"
    );
    std::fs::write(d.join("gh.cmd"), "@echo off\n").unwrap();
    assert_eq!(
        resolve_gh(Some(&d.join("gh.cmd")), &env).unwrap(),
        d.join("gh.cmd")
    );
    let f = |p: &str, s: &str| PrWorkFile {
        path: p.into(),
        status: s.into(),
    };
    assert_eq!(
        porcelain_z(b"R  new.txt\0old.txt\0?? dir/x y.txt\0 M m.rs\0"),
        vec![f("new.txt", "R"), f("dir/x y.txt", "??"), f("m.rs", "M")]
    );
}

#[test]
fn pr_file_reading() {
    let d = tmp("prfile");
    let f = d.join("pr.md");
    assert_eq!(read_pr_file(&f, false).unwrap_err().id, "pr-no-text");
    std::fs::write(&f, "\r\n## 제목 하나\r\n\r\n본문\r\n- a\r\n\r\n").unwrap();
    assert_eq!(
        read_pr_file(&f, false).unwrap(),
        ("제목 하나".to_owned(), "본문\n- a".to_owned())
    );
    assert_eq!(read_pr_file(&f, true).unwrap().0, "## 제목 하나");
    std::fs::write(&f, "#\n\nbody").unwrap();
    assert_eq!(read_pr_file(&f, false).unwrap_err().id, "pr-empty-title");
    reset_pr_file(&f).unwrap();
    assert!(!f.exists() && d.is_dir());
}

// ---- GitHub reads -------------------------------------------------------------------------

#[test]
fn status_gate_and_open_lookup() {
    let fx = fixture("status");
    let head = git(&fx.work, &["rev-parse", "HEAD"]);
    let h = head.clone();
    let fake = FakeGh::new(move |c| match (c.method.as_str(), c.path.as_str()) {
        ("GET", "repos/acme/widgets/pulls") if c.q("state") == Some("all") => r(
            200,
            json!([
                { "number": 3, "state": "closed", "head": { "ref": "feature", "repo": { "full_name": "acme/widgets" } } },
                { "number": 9, "state": "open", "head": { "ref": "feature", "repo": { "full_name": "fork/widgets" } } },
                { "number": 5, "state": "open", "head": { "ref": "feature", "repo": { "full_name": "Acme/Widgets" } } }
            ]),
        ),
        ("GET", "repos/acme/widgets/pulls") => r(200, json!([pr_json(5, "open", &h)])),
        ("GET", "repos/acme/widgets/pulls/5") => r(200, pr_json(5, "open", &h)),
        ("GET", "repos/acme/widgets/pulls/5/reviews") => r(
            200,
            json!([
                { "user": { "login": "a" }, "state": "APPROVED" },
                { "user": { "login": "b" }, "state": "CHANGES_REQUESTED" },
                { "user": { "login": "b" }, "state": "APPROVED" },
                { "user": { "login": "c" }, "state": "COMMENTED" }
            ]),
        ),
        ("POST", "graphql") => r(
            200,
            json!({ "data": { "repository": { "pullRequest": {
                "headRefOid": h, "reviewDecision": null, "mergeStateStatus": "BLOCKED",
                "commits": { "nodes": [ { "commit": { "statusCheckRollup": { "contexts": { "nodes": [
                    { "__typename": "CheckRun", "name": "ci", "status": "IN_PROGRESS", "conclusion": null, "isRequired": true },
                    { "__typename": "CheckRun", "name": "lint", "status": "COMPLETED", "conclusion": "SKIPPED", "isRequired": false },
                    { "__typename": "StatusContext", "context": "deploy", "state": "ERROR", "isRequired": false }
                ] } } } } ] }
            } } } }),
        ),
        other => panic!("unexpected {other:?}"),
    });
    let st = status(&fx.work, fake.gh()).unwrap().unwrap();
    assert_eq!(
        (
            st.number,
            st.state,
            st.head_sha.as_str(),
            st.base_ref.as_str()
        ),
        (5, PrState::Open, head.as_str(), "main")
    );
    assert_eq!((st.approved, st.changes_requested), (true, false));
    assert_eq!(
        (st.mergeable, st.mergeable_state.as_str()),
        (Some(true), "clean")
    );
    let list = &fake.calls()[0];
    assert_eq!(list.q("head"), Some("acme:feature"));
    assert_eq!(list.q("per_page"), Some("20"));

    let g = gate(&fx.work, fake.gh(), 5).unwrap();
    assert_eq!(g.merge_state, "BLOCKED");
    assert_eq!(
        g.checks
            .iter()
            .map(|c| (c.name.as_str(), c.state, c.required))
            .collect::<Vec<_>>(),
        vec![
            ("ci", PrCheckState::Pending, true),
            ("lint", PrCheckState::Skipped, false),
            ("deploy", PrCheckState::Failed, false)
        ]
    );
    let gq = fake.calls().last().unwrap().clone();
    assert_eq!(
        gq.body.as_ref().unwrap()["variables"],
        json!({"owner": "acme", "name": "widgets", "number": 5})
    );
    assert_eq!(
        gate(&fx.work, fake.gh(), 0).unwrap_err().id,
        "pr-bad-number"
    );

    let open = find_open(&fx.work, fake.gh(), "dev").unwrap().unwrap();
    assert_eq!(open.kind, PrResultKind::Exists);
    assert_eq!(open.number, Some(5));
    assert_eq!(open.base.as_deref(), Some("main"));
    assert_eq!(open.mergeable_state.as_deref(), Some("clean"));
    assert_eq!(open.session_base.as_deref(), Some("dev"));
    assert_eq!(
        find_open(&fx.work, fake.gh(), "main")
            .unwrap()
            .unwrap()
            .session_base,
        None
    );

    // No PR, gh missing / not logged in / 401, a refusal, a detached HEAD.
    let none = FakeGh::new(|_| r(200, json!([])));
    assert_eq!(status(&fx.work, none.gh()).unwrap(), None);
    assert_eq!(find_open(&fx.work, none.gh(), "main").unwrap(), None);
    assert_eq!(
        status(&fx.work, Err(ProtoError::new("gh-missing")))
            .unwrap_err()
            .id,
        "gh-missing"
    );
    let unauth = FakeGh::new(|_| r(401, json!({"message": "Bad credentials"})));
    assert_eq!(status(&fx.work, unauth.gh()).unwrap_err().id, "gh-auth");
    let refused = FakeGh::new(|_| r(403, json!({"message": "rate limited"})));
    let e = status(&fx.work, refused.gh()).unwrap_err();
    assert_eq!(
        (e.id.as_str(), e.get("status"), e.get("detail")),
        ("pr-api", Some("403"), Some("rate limited"))
    );
    // mergeable still being computed: one retry.
    let h2 = head.clone();
    let n = std::sync::atomic::AtomicUsize::new(0);
    let computing = FakeGh::new(move |c| match c.path.as_str() {
        "repos/acme/widgets/pulls" => r(200, json!([pr_json(5, "open", &h2)])),
        "repos/acme/widgets/pulls/5" => {
            let mut p = pr_json(5, "open", &h2);
            if n.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                p["mergeable"] = Value::Null;
            }
            r(200, p)
        }
        _ => r(200, json!([])),
    });
    let st = status_with_retry(&fx.work, computing.gh(), Duration::ZERO)
        .unwrap()
        .unwrap();
    assert_eq!(st.mergeable, Some(true));
    assert_eq!(
        computing
            .calls()
            .iter()
            .filter(|c| c.path.ends_with("pulls/5"))
            .count(),
        2
    );
    git(&fx.work, &["checkout", "-q", "--detach"]);
    assert_eq!(status(&fx.work, none.gh()).unwrap_err().id, "pr-detached");
}

// ---- create -------------------------------------------------------------------------------

fn request<'a>(base: &'a str, pr_file: &'a Path) -> CreateRequest<'a> {
    CreateRequest {
        base,
        pr_file,
        draft: false,
        push: true,
        push_sha: None,
        options: None,
    }
}

#[test]
fn create_pushes_then_opens_the_pr() {
    let fx = fixture("create");
    let f = fx.pr_file();
    reset_pr_file(&f).unwrap();
    let created = FakeGh::new(|c| match (c.method.as_str(), c.path.as_str()) {
        ("GET", "repos/acme/widgets/pulls") => r(200, json!([])),
        ("POST", "repos/acme/widgets/pulls") => r(
            201,
            json!({ "number": 7, "html_url": format!("{URL}/pull/7") }),
        ),
        other => panic!("unexpected {other:?}"),
    });
    // The agent wrote nothing: refused before anything is pushed.
    assert_eq!(
        create(&fx.work, created.gh(), &request("main", &f))
            .unwrap_err()
            .id,
        "pr-no-text"
    );
    assert_eq!(fx.remote_sha("feature"), None);

    std::fs::write(&f, "# 기능 A 추가\n\n요약\n").unwrap();
    let draft = CreateRequest {
        draft: true,
        ..request("main", &f)
    };
    let res = create(&fx.work, created.gh(), &draft).unwrap();
    assert_eq!(
        (res.kind, res.number, res.title.as_str()),
        (PrResultKind::Created, Some(7), "기능 A 추가")
    );
    let head = git(&fx.work, &["rev-parse", "HEAD"]);
    assert_eq!(fx.remote_sha("feature"), Some(head.clone()));
    assert_eq!(
        git(&fx.work, &["rev-parse", "--abbrev-ref", "@{u}"]),
        "origin/feature"
    );
    assert!(!f.exists(), "the used PR text is removed");
    let post = created
        .calls()
        .into_iter()
        .find(|c| c.method == "POST")
        .unwrap();
    assert_eq!(
        post.body.unwrap(),
        json!({ "title": "기능 A 추가", "body": "요약", "head": "feature", "base": "main", "draft": true })
    );

    // An open PR already exists: settled, nothing pushed.
    commit(&fx.work, "b.txt", "b\n", "기능 B");
    std::fs::write(&f, "B\n\nb").unwrap();
    let existing = FakeGh::new(move |c| match (c.method.as_str(), c.path.as_str()) {
        ("GET", "repos/acme/widgets/pulls") => r(200, json!([pr_json(7, "open", "x")])),
        other => panic!("unexpected {other:?}"),
    });
    let res = create(&fx.work, existing.gh(), &request("main", &f)).unwrap();
    assert_eq!((res.kind, res.number), (PrResultKind::Exists, Some(7)));
    assert!(!f.exists());
    assert_eq!(fx.remote_sha("feature"), Some(head));

    // Same branch as the target; no remote branch without pushing.
    std::fs::write(&f, "B\n\nb").unwrap();
    assert_eq!(
        create(&fx.work, existing.gh(), &request("feature", &f))
            .unwrap_err()
            .id,
        "pr-same-branch"
    );
    git(&fx.work, &["checkout", "-q", "-b", "local-only"]);
    let no_push = CreateRequest {
        push: false,
        ..request("main", &f)
    };
    assert_eq!(
        create(&fx.work, existing.gh(), &no_push).unwrap_err().id,
        "pr-no-remote-branch"
    );
}

#[test]
fn create_without_api_or_refused_leaves_the_text() {
    let fx = fixture("create-page");
    let f = fx.pr_file();
    reset_pr_file(&f).unwrap();
    std::fs::write(&f, "제목 & 기호\n\n본문").unwrap();
    let head = git(&fx.work, &["rev-parse", "HEAD"]);
    // gh not logged in: pushed, then the prefilled page.
    let unauth = FakeGh::new(|_| Err(ProtoError::new("gh-auth").arg("host", "github.com")));
    let res = create(&fx.work, unauth.gh(), &request("main", &f)).unwrap();
    assert_eq!(res.kind, PrResultKind::Page);
    assert_eq!(res.reason.as_ref().map(|e| e.id.as_str()), Some("gh-auth"));
    assert!(
        res.url
            .starts_with(&format!("{URL}/compare/main...feature?expand=1&title=")),
        "{}",
        res.url
    );
    assert_eq!(fx.remote_sha("feature"), Some(head.clone()));
    let left = leftover(&fx.work, Some(&f), Some("main")).unwrap();
    let file = left.file.unwrap();
    assert_eq!(
        (file.title.as_str(), file.matches, file.used_for.as_deref()),
        ("제목 & 기호", LeftoverMatch::Same, Some(head.as_str()))
    );
    assert_eq!(left.changes, Some(true));
    assert_eq!(left.commits.len(), 1);
    // gh missing: the same page.
    let res = create(
        &fx.work,
        Err(ProtoError::new("gh-missing")),
        &request("main", &f),
    )
    .unwrap();
    assert_eq!(res.reason.map(|e| e.id), Some("gh-missing".to_owned()));
    // Neither a credential token nor gh (the transport's error): the same page.
    let nocred =
        FakeGh::new(|_| Err(ProtoError::new("github-no-credentials").arg("host", "github.com")));
    let res = create(&fx.work, nocred.gh(), &request("main", &f)).unwrap();
    assert_eq!(
        (res.kind, res.reason.map(|e| e.id)),
        (PrResultKind::Page, Some("github-no-credentials".to_owned()))
    );

    // The API refuses (422): reconciled once, then Failed with the reason kept beside the text.
    let refused = FakeGh::new(|c| match c.method.as_str() {
        "GET" => r(200, json!([])),
        _ => r(
            422,
            json!({ "message": "Validation Failed", "errors": [{ "message": "No commits between main and feature" }] }),
        ),
    });
    let res = create(&fx.work, refused.gh(), &request("main", &f)).unwrap();
    assert_eq!(res.kind, PrResultKind::Failed);
    let reason = res.reason.unwrap();
    assert_eq!(
        (
            reason.id.as_str(),
            reason.get("status"),
            reason.get("detail")
        ),
        (
            "pr-api",
            Some("422"),
            Some("Validation Failed · No commits between main and feature")
        )
    );
    assert_eq!(
        refused.calls().iter().filter(|c| c.method == "GET").count(),
        2
    );
    commit(&fx.work, "c.txt", "c\n", "기능 C");
    let file = leftover(&fx.work, Some(&f), None).unwrap().file.unwrap();
    assert_eq!(file.matches, LeftoverMatch::Ahead);
    assert_eq!(file.new_commits.len(), 1);
    assert_eq!(file.reason.map(|e| e.id), Some("pr-api".to_owned()));
    // A rewritten branch.
    git(&fx.work, &["reset", "-q", "--hard", "HEAD~2"]);
    let file = leftover(&fx.work, Some(&f), None).unwrap().file.unwrap();
    assert_eq!(file.matches, LeftoverMatch::Diverged);
}

#[test]
fn commit_based_pr_is_pinned() {
    let fx = fixture("commit-pr");
    commit(&fx.work, "b.txt", "b\n", "기능 B");
    let f = fx.pr_file();
    let p = commit_preview(&fx.work, "main", true).unwrap();
    assert_eq!(p.title, "기능 B 외 1개");
    assert_eq!(
        p.commits
            .iter()
            .map(|c| c.subject.as_str())
            .collect::<Vec<_>>(),
        ["기능 A", "기능 B"]
    );
    assert!(p.body.starts_with("## 커밋\n\n- 기능 A ("));
    assert!(p.body.contains("## 변경 파일\n\n```\n"));
    assert_eq!(p.branch, "feature");
    assert_eq!(p.head, git(&fx.work, &["rev-parse", "HEAD"]));
    // Nothing published yet: the source must be on origin when not pushing.
    assert!(commit_preview(&fx.work, "main", false).is_err());

    let pins = CommitPrTextOptions {
        expected_base: Some("main".into()),
        push: Some(true),
        title: Some("  커밋 제목  ".into()),
        expected_head: Some(p.head.clone()),
        expected_source_head: Some(p.source_head.clone()),
        expected_base_head: Some(p.base_head.clone()),
        expected_branch: Some(p.branch.clone()),
    };
    let bad = CommitPrTextOptions {
        title: Some("두\n줄".into()),
        ..pins.clone()
    };
    assert_eq!(
        commit_text(&fx.work, "main", &f, &bad).unwrap_err().id,
        "pr-bad-title"
    );
    assert_eq!(
        commit_text(&fx.work, "main", &f, &pins).unwrap(),
        "커밋 제목"
    );
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.starts_with("커밋 제목\n\n## 커밋\n"), "{text}");

    let ok_gh = FakeGh::new(|c| match c.method.as_str() {
        "GET" => r(200, json!([])),
        _ => r(
            201,
            json!({ "number": 8, "html_url": format!("{URL}/pull/8") }),
        ),
    });
    // Pins that no longer hold.
    let stale = CommitPrTextOptions {
        expected_head: Some("0".repeat(40)),
        ..pins.clone()
    };
    let mut req = request("main", &f);
    req.options = Some(&stale);
    assert_eq!(
        create(&fx.work, ok_gh.gh(), &req).unwrap_err().id,
        "pr-changed"
    );
    // A later local commit: the commit-based request no longer matches (Desk refuses too)...
    let approved = p.head.clone();
    commit(&fx.work, "later.txt", "x\n", "나중 커밋");
    let pinned = CommitPrTextOptions {
        expected_head: None,
        expected_source_head: None,
        ..pins.clone()
    };
    let mut req = request("main", &f);
    req.options = Some(&pinned);
    req.push_sha = Some(&approved);
    assert_eq!(
        create(&fx.work, ok_gh.gh(), &req).unwrap_err().id,
        "pr-changed"
    );
    assert_eq!(fx.remote_sha("feature"), None);
    // ...while a recheck's "leave the new commits out" (push_sha, no pins) pushes only up to
    // the approved commit; later commits stay local.
    let mut req = request("main", &f);
    req.push_sha = Some(&approved);
    let res = create(&fx.work, ok_gh.gh(), &req).unwrap();
    assert_eq!((res.kind, res.number), (PrResultKind::Created, Some(8)));
    assert_eq!(fx.remote_sha("feature"), Some(approved));
    assert_eq!(
        git(&fx.work, &["rev-parse", "--abbrev-ref", "@{u}"]),
        "origin/feature"
    );
    // The title line is read as written (a commit-based text has no markdown heading).
    let post = ok_gh
        .calls()
        .into_iter()
        .find(|c| c.method == "POST")
        .unwrap();
    assert_eq!(post.body.unwrap()["title"], "커밋 제목");
    // A commit-based create with matching pins.
    git(&fx.work, &["reset", "-q", "--hard", "HEAD~1"]);
    let p2 = commit_preview(&fx.work, "main", true).unwrap();
    let pins2 = CommitPrTextOptions {
        title: None,
        expected_head: Some(p2.head.clone()),
        expected_source_head: Some(p2.source_head.clone()),
        expected_base_head: Some(p2.base_head.clone()),
        ..pins.clone()
    };
    assert_eq!(
        commit_text(&fx.work, "main", &f, &pins2).unwrap(),
        "기능 B 외 1개"
    );
    let mut req = request("main", &f);
    req.options = Some(&pins2);
    req.push_sha = Some(&p2.head);
    assert_eq!(
        create(&fx.work, ok_gh.gh(), &req).unwrap().kind,
        PrResultKind::Created
    );
    // A changed base pin.
    let other = CommitPrTextOptions {
        expected_base: Some("dev".into()),
        ..pins
    };
    assert_eq!(
        commit_text(&fx.work, "main", &f, &other).unwrap_err().id,
        "pr-changed"
    );
}

// ---- merge / close / update ---------------------------------------------------------------

#[test]
fn merge_close_update() {
    let fx = fixture("merge");
    let head = "ab".repeat(20);
    let h = head.clone();
    let state = std::sync::Arc::new(Mutex::new(("open", false)));
    let st = state.clone();
    let fake = FakeGh::new(move |c| {
        let (s, merged) = *st.lock().unwrap();
        match (c.method.as_str(), c.path.as_str()) {
            ("GET", "repos/acme/widgets/pulls/5") => {
                let mut p = pr_json(5, s, &h);
                p["merged"] = json!(merged);
                r(200, p)
            }
            ("PUT", "repos/acme/widgets/pulls/5/merge") => r(
                200,
                json!({ "merged": true, "sha": "m".repeat(40), "message": "Pull Request successfully merged" }),
            ),
            ("DELETE", "repos/acme/widgets/git/refs/heads/feature") => r(204, Value::Null),
            ("PATCH", "repos/acme/widgets/pulls/5") => r(200, json!({ "state": "closed" })),
            ("PUT", "repos/acme/widgets/pulls/5/update-branch") => {
                r(202, json!({ "message": "Updating pull request branch." }))
            }
            other => panic!("unexpected {other:?}"),
        }
    });
    let e = merge(
        &fx.work,
        fake.gh(),
        5,
        "cdcdcdc",
        PrMergeMethod::Squash,
        true,
    )
    .unwrap_err();
    assert_eq!(
        (e.id.as_str(), e.get("expected"), e.get("actual")),
        ("pr-head-moved", Some("cdcdcdc"), Some("abababa"))
    );
    assert_eq!(
        merge(&fx.work, fake.gh(), 5, "xyz", PrMergeMethod::Squash, true)
            .unwrap_err()
            .id,
        "pr-bad-sha"
    );
    let m = merge(
        &fx.work,
        fake.gh(),
        5,
        &head[..7],
        PrMergeMethod::Rebase,
        true,
    )
    .unwrap();
    assert!(m.branch_deleted);
    assert_eq!(m.sha, "m".repeat(40));
    let put = fake
        .calls()
        .into_iter()
        .find(|c| c.method == "PUT")
        .unwrap();
    assert_eq!(
        put.body.unwrap(),
        json!({ "sha": head, "merge_method": "rebase" })
    );
    *state.lock().unwrap() = ("closed", true);
    assert_eq!(
        merge(&fx.work, fake.gh(), 5, &head, PrMergeMethod::Merge, false)
            .unwrap_err()
            .id,
        "pr-already-merged"
    );
    assert_eq!(close(&fx.work, fake.gh(), 5).unwrap_err().id, "pr-not-open");
    *state.lock().unwrap() = ("open", false);
    let c = close(&fx.work, fake.gh(), 5).unwrap();
    assert_eq!((c.number, c.url), (5, format!("{URL}/pull/5")));
    assert_eq!(
        update_branch(&fx.work, fake.gh(), 5, &head)
            .unwrap()
            .as_deref(),
        Some("Updating pull request branch.")
    );
    let upd = fake
        .calls()
        .into_iter()
        .rev()
        .find(|c| c.path.ends_with("update-branch"))
        .unwrap();
    assert_eq!(upd.body.unwrap(), json!({ "expected_head_sha": head }));
    update_branch(&fx.work, fake.gh(), 5, "short").unwrap();
    assert_eq!(fake.calls().last().unwrap().body, Some(json!({})));

    let refusing = FakeGh::new(move |c| match c.method.as_str() {
        "GET" => r(200, pr_json(5, "open", &"ab".repeat(20))),
        _ => r(
            405,
            json!({ "message": "Required status check \"ci\" is expected." }),
        ),
    });
    let e = merge(
        &fx.work,
        refusing.gh(),
        5,
        &"ab".repeat(20),
        PrMergeMethod::Squash,
        false,
    )
    .unwrap_err();
    assert_eq!(
        (e.id.as_str(), e.get("status")),
        ("pr-merge-refused", Some("405"))
    );
    assert_eq!(
        close(&fx.work, refusing.gh(), 5).unwrap_err().id,
        "pr-close-failed"
    );
    assert_eq!(
        update_branch(&fx.work, refusing.gh(), 5, "x")
            .unwrap_err()
            .id,
        "pr-update-refused"
    );

    // GitLab origins are refused for the API calls.
    let bare_s = fx.bare.display().to_string().replace('\\', "/");
    git(
        &fx.work,
        &["config", "--unset", &format!("url.{bare_s}.insteadOf")],
    );
    git(
        &fx.work,
        &[
            "config",
            "remote.origin.url",
            "https://gitlab.com/acme/widgets",
        ],
    );
    assert_eq!(close(&fx.work, fake.gh(), 5).unwrap_err().id, "pr-gitlab");
}

// ---- local state --------------------------------------------------------------------------

#[test]
fn pending_drift_unmerged_targets() {
    let fx = fixture("local");
    std::fs::write(fx.work.join("new.txt"), "n\n").unwrap();
    let p = pending_work(&fx.work);
    assert_eq!(
        (p.branch.as_str(), p.commit_count, p.published),
        ("feature", 1, false)
    );
    assert_eq!(p.commits[0].subject, "기능 A");
    assert_eq!(
        p.files,
        vec![PrWorkFile {
            path: "new.txt".into(),
            status: "??".into()
        }]
    );

    let t = remote_branches(&fx.work).unwrap();
    assert_eq!(
        (t.branches, t.default_branch.as_deref(), t.current.as_str()),
        (vec!["main".to_owned()], Some("main"), "feature")
    );
    assert_eq!(
        unmerged(&fx.work, "main"),
        Some(UnmergedWork {
            commits: 1,
            conflict: false
        })
    );
    assert_eq!(unmerged(&fx.work, "nope"), None);

    let before = git(&fx.work, &["rev-parse", "HEAD"]);
    let d = drift(&fx.work, Some(&before), &[]).unwrap();
    assert!(!d.rewritten && d.commits.is_empty() && d.files.len() == 1);
    commit(&fx.work, "new.txt", "n\n", "요청한 커밋");
    commit(&fx.work, "other.txt", "o\n", "다른 커밋");
    let d = drift(&fx.work, Some(&before), &["new.txt".into()]).unwrap();
    assert_eq!(
        d.commits
            .iter()
            .map(|c| (c.subject.as_str(), c.expected))
            .collect::<Vec<_>>(),
        [("요청한 커밋", true), ("다른 커밋", false)]
    );
    let change = ide_proto::drift_change(&d, &before, &["new.txt".into()], true).unwrap();
    assert_eq!(change.keep_up_to, d.commits[0].full);
    assert!(drift(&fx.work, Some("deadbeef"), &[]).unwrap().rewritten);

    git(&fx.work, &["push", "-q", "-u", "origin", "feature"]);
    let p = pending_work(&fx.work);
    assert_eq!((p.commit_count, p.published), (0, true));
    // Merged into main on origin: nothing left.
    git(&fx.work, &["push", "-q", "origin", "feature:main"]);
    git(&fx.work, &["fetch", "-q", "origin"]);
    assert_eq!(
        unmerged(&fx.work, "main"),
        Some(UnmergedWork {
            commits: 0,
            conflict: false
        })
    );
    assert_eq!(
        leftover(&fx.work, None, Some("main")).unwrap().changes,
        Some(false)
    );
    assert_eq!(
        leftover(&fx.work, None, Some("-x")).unwrap_err().id,
        "pr-bad-branch"
    );
}

// ---- transport: git credential token + HTTPS, gh fallback ---------------------------------

/// A helper answering every `get` with a token.
const TOKEN_HELPER: &str =
    "!f() { test \"$1\" = get && echo username=x-access-token && echo password=test-token-1; }; f";
/// A helper telling git to stop (no credential, no prompt).
const QUIT_HELPER: &str = "!f() { echo quit=1; }; f";

/// Environment for `git credential fill` that never reaches the user's credential store: a
/// scratch global config holding only `credential.helper = helper`, no system config.
fn isolated_credentials(dir: &Path, helper: &str) -> Vec<(String, String)> {
    let cfg = dir.join("gitconfig");
    std::fs::write(&cfg, "").unwrap();
    let out = Command::new("git")
        .args(["config", "--file"])
        .arg(&cfg)
        .args(["credential.helper", helper])
        .output()
        .unwrap();
    assert!(out.status.success());
    vec![
        ("GIT_CONFIG_GLOBAL".into(), cfg.display().to_string()),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ]
}

type Seen = Arc<Mutex<Vec<(Call, Option<String>)>>>;

/// A local fake GitHub over HTTP: records each request with its `Authorization` header.
struct FakeHttp {
    base: String,
    seen: Seen,
}

impl FakeHttp {
    fn start(handler: impl Fn(&Call) -> (u16, Value) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Seen::default();
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &handler, &log);
            }
        });
        FakeHttp { base, seen }
    }
    fn seen(&self) -> Vec<(Call, Option<String>)> {
        self.seen.lock().unwrap().clone()
    }
}

fn form_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8(out).unwrap()
}

fn serve(mut stream: TcpStream, handler: &dyn Fn(&Call) -> (u16, Value), seen: &Seen) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();
    let mut headers: HashMap<String, String> = HashMap::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).unwrap() == 0 || h.trim_end().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_owned());
        }
    }
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body).unwrap();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let call = Call {
        method,
        path: path.trim_start_matches('/').to_owned(),
        query: query
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|p| {
                let (k, v) = p.split_once('=').unwrap_or((p, ""));
                (form_decode(k), form_decode(v))
            })
            .collect(),
        body: (!body.is_empty()).then(|| serde_json::from_slice(&body).unwrap()),
    };
    let (status, answer) = handler(&call);
    seen.lock()
        .unwrap()
        .push((call, headers.get("authorization").cloned()));
    let text = if answer.is_null() {
        String::new()
    } else {
        answer.to_string()
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
}

fn transport(
    creds: &Arc<Credentials>,
    http: &FakeHttp,
    gh: Result<Arc<dyn GhApi>, ProtoError>,
) -> GithubTransport {
    GithubTransport {
        credentials: creds.clone(),
        http: Arc::new(HttpApi::new(Some(http.base.clone()))),
        gh,
    }
}

const LIST: GhRequest<'static> = GhRequest {
    host: "github.com",
    method: "GET",
    path: "repos/acme/widgets/pulls",
    query: &[],
    body: None,
};

#[test]
fn api_roots_like_desk() {
    assert_eq!(api_base("github.com"), "https://api.github.com");
    assert_eq!(
        api_base("ghe.example.com"),
        "https://ghe.example.com/api/v3"
    );
    assert_eq!(
        endpoint("https://api.github.com", "repos/a/b/pulls"),
        "https://api.github.com/repos/a/b/pulls"
    );
    assert_eq!(
        endpoint("https://api.github.com", "graphql"),
        "https://api.github.com/graphql"
    );
    assert_eq!(
        endpoint("https://ghe.example.com/api/v3", "graphql"),
        "https://ghe.example.com/api/graphql"
    );
}

#[test]
fn transport_sends_the_credential_token_over_https() {
    let d = tmp("transport-token");
    let fx = fixture("transport-token-repo");
    let http = FakeHttp::start(|c| match (c.method.as_str(), c.path.as_str()) {
        ("GET", "repos/acme/widgets/pulls") => (200, json!([])),
        ("POST", "graphql") => (200, json!({ "data": null })),
        _ => (404, json!({ "message": "Not Found" })),
    });
    let fallback = Arc::new(FakeGh::new(|_| r(200, json!([]))));
    let creds = Arc::new(Credentials::new(isolated_credentials(&d, TOKEN_HELPER)));
    let t = transport(&creds, &http, Ok(fallback.clone()));
    // A PR function through the transport: the query arrives decoded, the token as Bearer.
    assert_eq!(status(&fx.work, Ok(&t)).unwrap(), None);
    let body = json!({ "query": "q", "variables": { "n": 1 } });
    let res = t
        .call(&GhRequest {
            host: "github.com",
            method: "POST",
            path: "graphql",
            query: &[],
            body: Some(&body),
        })
        .unwrap();
    assert_eq!(
        res,
        GhResponse {
            status: 200,
            body: json!({ "data": null })
        }
    );
    let seen = http.seen();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(
        (
            seen[0].0.q("state"),
            seen[0].0.q("head"),
            seen[0].0.q("per_page")
        ),
        (Some("all"), Some("acme:feature"), Some("20"))
    );
    assert_eq!(seen[1].0.body, Some(body));
    assert!(
        seen.iter()
            .all(|(_, auth)| auth.as_deref() == Some("Bearer test-token-1"))
    );
    assert!(fallback.calls().is_empty());
    // Cached per host (case-insensitive): the helper is not asked again for a while.
    assert_eq!(
        creds.cached("GitHub.com"),
        Some(Some("test-token-1".into()))
    );
}

#[test]
fn transport_falls_back_to_gh_without_a_token() {
    let d = tmp("transport-gh");
    let http = FakeHttp::start(|_| (500, Value::Null));
    let fallback = Arc::new(FakeGh::new(|_| r(200, json!([{ "number": 1 }]))));
    let creds = Arc::new(Credentials::new(isolated_credentials(&d, QUIT_HELPER)));
    let res = transport(&creds, &http, Ok(fallback.clone()))
        .call(&LIST)
        .unwrap();
    assert_eq!(res.body, json!([{ "number": 1 }]));
    assert_eq!(fallback.calls().len(), 1);
    assert!(http.seen().is_empty());
    assert_eq!(creds.cached("github.com"), Some(None));
    // Neither a token nor gh.
    let e = transport(&creds, &http, Err(ProtoError::new("gh-missing")))
        .call(&LIST)
        .unwrap_err();
    assert_eq!(
        (e.id.as_str(), e.get("host")),
        ("github-no-credentials", Some("github.com"))
    );
    assert!(http.seen().is_empty());
}

#[test]
fn transport_forgets_a_refused_token() {
    let d = tmp("transport-401");
    let http = FakeHttp::start(|_| (401, json!({ "message": "Bad credentials" })));
    let fallback = Arc::new(FakeGh::new(|_| r(200, json!({ "via": "gh" }))));
    let creds = Arc::new(Credentials::new(isolated_credentials(&d, TOKEN_HELPER)));
    // 401 with the token: forgotten, the call made once more through gh.
    let res = transport(&creds, &http, Ok(fallback.clone()))
        .call(&LIST)
        .unwrap();
    assert_eq!(res.body, json!({ "via": "gh" }));
    assert_eq!((http.seen().len(), fallback.calls().len()), (1, 1));
    assert_eq!(creds.cached("github.com"), None);
    // Without gh: `github-auth`; the token was asked for again (not served from the cache).
    let e = transport(&creds, &http, Err(ProtoError::new("gh-missing")))
        .call(&LIST)
        .unwrap_err();
    assert_eq!(
        (e.id.as_str(), e.get("host")),
        ("github-auth", Some("github.com"))
    );
    assert_eq!(http.seen().len(), 2);
    assert_eq!(creds.cached("github.com"), None);
    // No HTTP answer at all with a token: `github-failed`, no gh retry.
    let dead = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = dead.local_addr().unwrap();
    drop(dead);
    let t = GithubTransport {
        credentials: creds.clone(),
        http: Arc::new(HttpApi::new(Some(format!("http://{addr}")))),
        gh: Ok(fallback.clone()),
    };
    assert_eq!(t.call(&LIST).unwrap_err().id, "github-failed");
    assert_eq!(fallback.calls().len(), 1);
}
