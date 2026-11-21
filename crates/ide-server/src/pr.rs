//! Pull requests and agent prompts of the Changes footer: port of Hermes Desk's
//! `main/github.ts` PR functions (status, gate, open-PR lookup, create, merge, close, update
//! branch, PR text left behind, drift, pending work, commit-based PR text) and the delivery of
//! a prompt to a session's agent. See `docs/changes-actions.md`, "Pull requests and agent
//! prompts".
//!
//! GitHub is reached like Desk does it: the token of git's credential store and the REST /
//! GraphQL API over HTTPS ([`GithubTransport`], `pr/github.rs`), with the `gh` CLI
//! (`gh api --include`, same endpoints and payloads) only as the fallback for hosts git has no
//! token for. [`GhApi`] is the seam the tests replace; [`GhCli`] runs the real `gh` (`IDE_GH` /
//! `ServerConfig::gh`, else `PATH`, else the usual install folders). Git runs locally
//! (`ide_core::git_raw`), network commands (`push`, `fetch`, `ls-remote`) without prompts and
//! with a timeout.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ide_core::LaunchEnv;
use ide_proto::{
    CommitPrPreview, CommitPrTextOptions, DriftCommit, LeftoverMatch, PendingWork, PrCheck,
    PrCheckState, PrClosed, PrCommit, PrDrift, PrGate, PrLeftover, PrLeftoverFile, PrMergeMethod,
    PrMerged, PrResultKind, PrState, PrStatus, PrTargets, PrWorkFile, ProtoError,
    PullRequestResult, UnmergedWork,
};
use serde_json::{Value, json};

use crate::service::core_error;

mod github;
pub use github::{API_ENV, Credentials, GithubTransport, HttpApi, api_base, endpoint};

/// `gh` calls are given this long (Desk: 20 s per `fetch`; `gh` also starts a process).
const GH_TIMEOUT: Duration = Duration::from_secs(30);
/// Desk: `ls-remote` / `fetch` 60 s, `push` 180 s.
const NET_TIMEOUT: Duration = Duration::from_secs(60);
const PUSH_TIMEOUT: Duration = Duration::from_secs(180);
/// Desk: one short retry while GitHub computes `mergeable` right after a push.
const MERGEABLE_RETRY: Duration = Duration::from_millis(1500);

// ---- gh -----------------------------------------------------------------------------------

/// One GitHub API call (`gh api`): `path` relative to the API root (`repos/o/r/pulls`,
/// `graphql`); `query` becomes the query string of a GET; `body` is sent as JSON.
#[derive(Debug, Clone)]
pub struct GhRequest<'a> {
    /// GitHub host (`github.com` or an Enterprise host).
    pub host: &'a str,
    /// HTTP method.
    pub method: &'a str,
    /// Endpoint path.
    pub path: &'a str,
    /// GET parameters.
    pub query: &'a [(&'a str, String)],
    /// JSON body.
    pub body: Option<&'a Value>,
}

/// The HTTP answer of a [`GhRequest`].
#[derive(Debug, Clone, PartialEq)]
pub struct GhResponse {
    /// HTTP status.
    pub status: u16,
    /// Parsed JSON body (`Null` when empty, a string when not JSON).
    pub body: Value,
}

/// GitHub API access ([`GithubTransport`], the real `gh` or a test double). `Err` only when no
/// HTTP answer came: `gh-missing`, `gh-auth` (`host`), `gh-failed` (`detail`),
/// `github-no-credentials` (`host`), `github-auth` (`host`), `github-failed` (`detail`).
pub trait GhApi: Send + Sync {
    /// Make one call.
    fn call(&self, req: &GhRequest<'_>) -> Result<GhResponse, ProtoError>;
}

/// `gh` is not installed (or `IDE_GH` names a missing file).
fn gh_missing() -> ProtoError {
    ProtoError::new("gh-missing")
}

/// The `gh` program: `override_path` (`IDE_GH`), else `gh` on `PATH`, else the GitHub CLI's
/// usual install folders (`%ProgramFiles%\GitHub CLI`, `%LOCALAPPDATA%\Programs\GitHub CLI`;
/// `/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin`), else `gh-missing`.
pub fn resolve_gh(override_path: Option<&Path>, env: &LaunchEnv) -> Result<PathBuf, ProtoError> {
    if let Some(p) = override_path {
        return if p.is_file() {
            Ok(p.to_path_buf())
        } else {
            Err(gh_missing().arg("program", p.display().to_string()))
        };
    }
    if let Ok(p) = ide_core::resolve_program("gh", env) {
        return Ok(p);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
            if let Some(d) = std::env::var_os(var) {
                candidates.push(PathBuf::from(d).join("GitHub CLI").join("gh.exe"));
            }
        }
        if let Some(d) = std::env::var_os("LOCALAPPDATA") {
            candidates.push(
                PathBuf::from(d)
                    .join("Programs")
                    .join("GitHub CLI")
                    .join("gh.exe"),
            );
        }
    } else {
        for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
            candidates.push(Path::new(d).join("gh"));
        }
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(gh_missing)
}

/// The real `gh` CLI.
#[derive(Debug, Clone)]
pub struct GhCli {
    /// Resolved program.
    pub program: PathBuf,
    /// Environment for batch-file wrapping (`COMSPEC`).
    pub env: LaunchEnv,
}

impl GhCli {
    /// Command-line arguments of a call (after `gh`).
    pub fn args(req: &GhRequest<'_>) -> Vec<String> {
        let mut a: Vec<String> = [
            "api",
            "--hostname",
            req.host,
            "--include",
            "-X",
            req.method,
            req.path,
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        for (k, v) in req.query {
            a.push("-f".into());
            a.push(format!("{k}={v}"));
        }
        if req.body.is_some() {
            a.push("--input".into());
            a.push("-".into());
        }
        a
    }
}

impl GhApi for GhCli {
    fn call(&self, req: &GhRequest<'_>) -> Result<GhResponse, ProtoError> {
        let (program, args) =
            ide_core::wrap_batch_file(self.program.clone(), Self::args(req), &self.env)
                .map_err(|e| core_error(&e))?;
        let mut cmd = Command::new(program);
        cmd.args(&args)
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("GH_PAGER", "")
            .env("NO_COLOR", "1")
            .env("CLICOLOR", "0");
        let input = req.body.map(|b| format!("{b}\n").into_bytes());
        let out = match run_timeout(cmd, input, GH_TIMEOUT) {
            Ok(Some(out)) => out,
            Ok(None) => {
                return Err(ProtoError::new("gh-failed")
                    .arg("detail", format!("timeout ({}s)", GH_TIMEOUT.as_secs())));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(gh_missing().arg("program", self.program.display().to_string()));
            }
            Err(e) => return Err(ProtoError::new("gh-failed").arg("detail", e.to_string())),
        };
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        if let Some(r) = parse_gh_output(&out.stdout, &stderr) {
            return Ok(r);
        }
        // No HTTP answer: gh exits 4 when it has no login for the host.
        if out.status.code() == Some(4) {
            return Err(ProtoError::new("gh-auth").arg("host", req.host));
        }
        let detail = if stderr.is_empty() {
            format!("{}", out.status)
        } else {
            stderr
        };
        Err(ProtoError::new("gh-failed").arg("detail", detail))
    }
}

/// `gh api --include` output: `HTTP/2.0 200 OK`, headers, blank line, body. Without a status
/// line (some error paths), the status from gh's `(HTTP 404)` message on stderr and the body
/// from stdout.
pub fn parse_gh_output(stdout: &[u8], stderr: &str) -> Option<GhResponse> {
    let text = String::from_utf8_lossy(stdout);
    let body_of = |s: &str| -> Value {
        let s = s.trim();
        if s.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_owned()))
        }
    };
    if let Some(start) = text.find("HTTP/")
        && text[..start].trim().is_empty()
    {
        let rest = &text[start..];
        let first = rest.lines().next().unwrap_or("");
        let status: u16 = first.split_whitespace().nth(1)?.parse().ok()?;
        // The header block ends at the first empty line.
        let mut offset = 0;
        let mut body_start = rest.len();
        for line in rest.split_inclusive('\n') {
            offset += line.len();
            if line.trim_end_matches(['\r', '\n']).is_empty() {
                body_start = offset;
                break;
            }
        }
        return Some(GhResponse {
            status,
            body: body_of(&rest[body_start.min(rest.len())..]),
        });
    }
    let i = stderr.find("(HTTP ")?;
    let status: u16 = stderr[i + 6..].split(')').next()?.trim().parse().ok()?;
    Some(GhResponse {
        status,
        body: body_of(&text),
    })
}

/// Run `cmd` with `input` on stdin (else none), killed after `timeout` (`Ok(None)`). No console
/// window on Windows.
fn run_timeout(
    mut cmd: Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
) -> std::io::Result<Option<std::process::Output>> {
    cmd.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn()?;
    if let (Some(bytes), Some(mut stdin)) = (input, child.stdin.take()) {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&bytes);
        });
    }
    let reader = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut b);
            }
            b
        })
    };
    let out_t = reader(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err_t = reader(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let start = Instant::now();
    let status = loop {
        match child.try_wait()? {
            Some(st) => break st,
            None if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    Ok(Some(std::process::Output {
        status,
        stdout: out_t.join().unwrap_or_default(),
        stderr: err_t.join().unwrap_or_default(),
    }))
}

// ---- git helpers --------------------------------------------------------------------------

fn git_text(dir: &Path, args: &[&str]) -> Result<String, ProtoError> {
    Ok(String::from_utf8_lossy(&crate::git::run(dir, args)?)
        .trim()
        .to_owned())
}

fn git_ok(dir: &Path, args: &[&str]) -> bool {
    ide_core::git_raw(dir, args, &[("GIT_OPTIONAL_LOCKS", "0")]).is_ok_and(|o| o.code == Some(0))
}

/// Trimmed stdout, `""` on any failure (Desk's `.catch(() => '')`).
fn git_or_empty(dir: &Path, args: &[&str]) -> String {
    git_text(dir, args).unwrap_or_default()
}

/// A network git command (no prompts, killed after `timeout`; Desk's `NO_PROMPT`).
fn git_net(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, ProtoError> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(["-c", "core.quotepath=false"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never");
    let out = run_timeout(cmd, None, timeout)
        .map_err(|e| core_error(&ide_core::Error::GitSpawn(e)))?
        .ok_or_else(|| {
            ProtoError::new("git-timeout")
                .arg("command", args.first().copied().unwrap_or_default())
                .arg("secs", timeout.as_secs().to_string())
        })?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned());
    }
    let o = ide_core::GitOutput {
        code: out.status.code(),
        stdout: out.stdout,
        stderr: out.stderr,
    };
    Err(core_error(&o.error(args)))
}

/// The checked-out branch, `"HEAD"` when detached (`""` on failure).
fn current_branch(dir: &Path) -> String {
    git_or_empty(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
}

/// Desk: refuse a detached HEAD (`pr-detached`).
fn branch_or_detached(dir: &Path) -> Result<String, ProtoError> {
    let b = git_text(dir, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if b == "HEAD" {
        return Err(ProtoError::new("pr-detached"));
    }
    Ok(b)
}

/// Desk's branch-name check (`잘못된 브랜치입니다`): not empty / `HEAD` / an option / a full
/// ref, and `git check-ref-format refs/heads/<name>` accepts it.
fn check_branch(dir: &Path, name: &str) -> Result<(), ProtoError> {
    let bad = || ProtoError::new("pr-bad-branch").arg("branch", name);
    if name.is_empty() || name == "HEAD" || name.starts_with('-') || name.starts_with("refs/") {
        return Err(bad());
    }
    if !git_ok(dir, &["check-ref-format", &format!("refs/heads/{name}")]) {
        return Err(bad());
    }
    Ok(())
}

fn is_hex(s: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `log --format=%h%x09%s` lines.
fn parse_commits(out: &str) -> Vec<PrCommit> {
    out.lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (sha, subject) = l.split_once('\t').unwrap_or((l, ""));
            PrCommit {
                sha: sha.to_owned(),
                subject: subject.to_owned(),
            }
        })
        .collect()
}

/// Desk `commitList`: at most 30 non-merge commits of `range`.
fn commit_list(dir: &Path, range: &str) -> Vec<PrCommit> {
    parse_commits(&git_or_empty(
        dir,
        &[
            "log",
            "--no-merges",
            "--max-count=30",
            "--format=%h%x09%s",
            range,
        ],
    ))
}

/// Desk `porcelainZ`: `git status --porcelain -z` entries (a rename's old path skipped).
pub fn porcelain_z(out: &[u8]) -> Vec<PrWorkFile> {
    let text = String::from_utf8_lossy(out);
    let parts: Vec<&str> = text.split('\0').collect();
    let mut files = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        let e = parts[i];
        if e.chars().count() >= 4 && e.is_char_boundary(3) {
            let status = &e[..2];
            files.push(PrWorkFile {
                status: status.trim().to_owned(),
                path: e[3..].to_owned(),
            });
            if status.starts_with('R') || status.starts_with('C') {
                i += 1;
            }
        }
        i += 1;
    }
    files
}

fn status_files(dir: &Path) -> Vec<PrWorkFile> {
    crate::git::run(
        dir,
        &["status", "--porcelain", "-z", "--untracked-files=all"],
    )
    .map(|o| porcelain_z(&o))
    .unwrap_or_default()
}

// ---- origin -------------------------------------------------------------------------------

/// origin as host / owner / repo (Desk `RemoteRepo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRepo {
    /// Host.
    pub host: String,
    /// Owner (may contain `/` for nested groups).
    pub owner: String,
    /// Repository name.
    pub repo: String,
    /// `https://host/owner/repo`.
    pub web: String,
}

impl RemoteRepo {
    fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo).to_lowercase()
    }
    fn api(&self, rest: &str) -> String {
        format!("repos/{}/{}/{rest}", self.owner, self.repo)
    }
    fn gitlab(&self) -> bool {
        self.host.to_lowercase().contains("gitlab")
    }
}

/// Desk `readOriginRepo`: https or ssh remotes, credentials dropped.
pub fn parse_origin(url: &str) -> Option<RemoteRepo> {
    let mut url = url.trim().trim_end_matches(".git").to_owned();
    let ssh = url.strip_prefix("ssh://").unwrap_or(&url);
    if let Some(rest) = ssh.strip_prefix("git@") {
        let i = rest.find([':', '/'])?;
        url = format!("https://{}/{}", &rest[..i], &rest[i + 1..]);
    }
    for scheme in ["https://", "http://"] {
        if let Some(rest) = url.strip_prefix(scheme) {
            let slash = rest.find('/').unwrap_or(rest.len());
            if let Some(at) = rest[..slash].find('@') {
                url = format!("https://{}", &rest[at + 1..]);
            }
            break;
        }
    }
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    let (owner, repo) = path.rsplit_once('/')?;
    if host.is_empty() || owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(RemoteRepo {
        host: host.to_owned(),
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        web: format!("https://{host}/{owner}/{repo}"),
    })
}

/// origin of `dir`: `git remote get-url origin` (Desk), else the configured
/// `remote.origin.url` as written (before `insteadOf` rewriting).
pub fn origin_repo(dir: &Path) -> Result<RemoteRepo, ProtoError> {
    let url = git_text(dir, &["remote", "get-url", "origin"])
        .map_err(|_| ProtoError::new("pr-no-origin"))?;
    if let Some(r) = parse_origin(&url) {
        return Ok(r);
    }
    let raw = git_or_empty(dir, &["config", "--get", "remote.origin.url"]);
    parse_origin(&raw).ok_or_else(|| ProtoError::new("pr-origin-unparsable").arg("url", url))
}

// ---- GitHub calls -------------------------------------------------------------------------

/// The GitHub API, or why it is not available.
pub type Gh<'a> = Result<&'a dyn GhApi, ProtoError>;

fn call(
    gh: &dyn GhApi,
    r: &RemoteRepo,
    method: &str,
    path: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
) -> Result<GhResponse, ProtoError> {
    let res = gh.call(&GhRequest {
        host: &r.host,
        method,
        path,
        query,
        body,
    })?;
    if res.status == 401 {
        return Err(ProtoError::new("gh-auth").arg("host", &r.host));
    }
    Ok(res)
}

fn ok(res: &GhResponse) -> bool {
    (200..300).contains(&res.status)
}

/// Desk `apiMessage`: `message` and every `errors[].message`, joined with ` · `.
fn api_message(body: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(m) = body.get("message").and_then(Value::as_str) {
        parts.push(m.to_owned());
    }
    if let Some(errs) = body.get("errors").and_then(Value::as_array) {
        parts.extend(
            errs.iter()
                .filter_map(|e| e.get("message").and_then(Value::as_str))
                .map(str::to_owned),
        );
    }
    parts.retain(|p| !p.is_empty());
    parts.join(" · ")
}

fn api_error(id: &str, res: &GhResponse) -> ProtoError {
    ProtoError::new(id)
        .arg("status", res.status.to_string())
        .arg("detail", api_message(&res.body))
}

/// Desk `githubContext`: origin (GitLab refused) and the API.
fn github<'a>(dir: &Path, gh: &Gh<'a>) -> Result<(RemoteRepo, &'a dyn GhApi), ProtoError> {
    let r = origin_repo(dir)?;
    if r.gitlab() {
        return Err(ProtoError::new("pr-gitlab"));
    }
    let g = gh.as_ref().map_err(Clone::clone)?;
    Ok((r, *g))
}

fn s<'v>(v: &'v Value, path: &[&str]) -> Option<&'v str> {
    let mut cur = v;
    for k in path {
        cur = cur.get(k)?;
    }
    cur.as_str()
}

fn number_ok(number: u64) -> Result<(), ProtoError> {
    if number == 0 {
        return Err(ProtoError::new("pr-bad-number"));
    }
    Ok(())
}

/// Desk `lookupOpenPullRequest`: the open PR of `head` from this repository into it.
fn lookup_open(
    gh: &dyn GhApi,
    r: &RemoteRepo,
    head: &str,
) -> Result<Option<PullRequestResult>, ProtoError> {
    let res = call(
        gh,
        r,
        "GET",
        &r.api("pulls"),
        &[
            ("state", "open".into()),
            ("head", format!("{}:{head}", r.owner)),
            ("per_page", "100".into()),
        ],
        None,
    )?;
    if !ok(&res) {
        return Err(api_error("pr-api", &res));
    }
    let list = res
        .body
        .as_array()
        .ok_or_else(|| api_error("pr-api", &res))?;
    let full = r.full_name();
    let same = |v: Option<&str>| v.is_some_and(|n| n.to_lowercase() == full);
    let pr = list.iter().find(|p| {
        s(p, &["state"]) == Some("open")
            && s(p, &["head", "ref"]) == Some(head)
            && same(s(p, &["head", "repo", "full_name"]))
            && same(s(p, &["base", "repo", "full_name"]))
            && s(p, &["html_url"]).is_some_and(|u| !u.is_empty())
    });
    Ok(pr.map(|p| PullRequestResult {
        kind: PrResultKind::Exists,
        url: s(p, &["html_url"]).unwrap_or_default().to_owned(),
        number: p.get("number").and_then(Value::as_u64),
        title: s(p, &["title"]).unwrap_or(head).to_owned(),
        reason: None,
        base: s(p, &["base", "ref"]).map(str::to_owned),
        mergeable_state: None,
        session_base: None,
    }))
}

/// Desk `findOpenPullRequest` + the `github:findOpenPr` handler's `sessionBase`.
pub fn find_open(
    dir: &Path,
    gh: Gh<'_>,
    session_base: &str,
) -> Result<Option<PullRequestResult>, ProtoError> {
    let head = branch_or_detached(dir)?;
    let r = origin_repo(dir)?;
    if r.gitlab() {
        return Err(ProtoError::new("pr-gitlab"));
    }
    let gh = gh?;
    let mut found = lookup_open(gh, &r, &head)?;
    if let Some(f) = found.as_mut()
        && let Some(n) = f.number
    {
        // List responses omit mergeability: one detail GET.
        if let Ok(d) = call(gh, &r, "GET", &r.api(&format!("pulls/{n}")), &[], None)
            && ok(&d)
            && d.body.get("number").and_then(Value::as_u64) == Some(n)
        {
            let pr = &d.body;
            let merged = pr.get("merged").and_then(Value::as_bool).unwrap_or(false);
            if s(pr, &["state"]) == Some("closed") || merged {
                found = None;
            } else if s(pr, &["state"]) == Some("open") && s(pr, &["head", "ref"]) == Some(&head) {
                if let Some(b) = s(pr, &["base", "ref"]) {
                    f.base = Some(b.to_owned());
                }
                f.mergeable_state =
                    Some(s(pr, &["mergeable_state"]).unwrap_or("unknown").to_owned());
            }
        }
    }
    if let Some(f) = found.as_mut()
        && let Some(b) = &f.base
        && !session_base.is_empty()
        && session_base != b
    {
        f.session_base = Some(session_base.to_owned());
    }
    Ok(found)
}

/// Desk `reviewState`: latest review per reviewer.
fn review_state(gh: &dyn GhApi, r: &RemoteRepo, number: u64) -> (bool, bool) {
    let Ok(res) = call(
        gh,
        r,
        "GET",
        &r.api(&format!("pulls/{number}/reviews")),
        &[("per_page", "100".into())],
        None,
    ) else {
        return (false, false);
    };
    let Some(list) = res.body.as_array().filter(|_| ok(&res)) else {
        return (false, false);
    };
    let mut latest: HashMap<String, String> = HashMap::new();
    for rv in list {
        let (Some(who), Some(state)) = (s(rv, &["user", "login"]), s(rv, &["state"])) else {
            continue;
        };
        if state == "COMMENTED" || state == "PENDING" {
            continue;
        }
        latest.insert(who.to_owned(), state.to_owned());
    }
    let has = |x: &str| latest.values().any(|v| v == x);
    (
        has("APPROVED") && !has("CHANGES_REQUESTED"),
        has("CHANGES_REQUESTED"),
    )
}

fn to_status(pr: &Value, review: (bool, bool)) -> PrStatus {
    let merged = pr.get("merged").and_then(Value::as_bool).unwrap_or(false);
    PrStatus {
        number: pr.get("number").and_then(Value::as_u64).unwrap_or(0),
        url: s(pr, &["html_url"]).unwrap_or_default().to_owned(),
        title: s(pr, &["title"]).unwrap_or_default().to_owned(),
        state: if merged {
            PrState::Merged
        } else if s(pr, &["state"]) == Some("closed") {
            PrState::Closed
        } else {
            PrState::Open
        },
        draft: pr.get("draft").and_then(Value::as_bool).unwrap_or(false),
        head_ref: s(pr, &["head", "ref"]).unwrap_or_default().to_owned(),
        head_sha: s(pr, &["head", "sha"]).unwrap_or_default().to_owned(),
        base_ref: s(pr, &["base", "ref"]).unwrap_or_default().to_owned(),
        mergeable: pr.get("mergeable").and_then(Value::as_bool),
        mergeable_state: s(pr, &["mergeable_state"]).unwrap_or("unknown").to_owned(),
        approved: review.0,
        changes_requested: review.1,
    }
}

/// Desk `pullRequestStatus`: the branch's PR (open first, else the latest), `None` = none.
pub fn status(dir: &Path, gh: Gh<'_>) -> Result<Option<PrStatus>, ProtoError> {
    status_with_retry(dir, gh, MERGEABLE_RETRY)
}

fn status_with_retry(
    dir: &Path,
    gh: Gh<'_>,
    retry: Duration,
) -> Result<Option<PrStatus>, ProtoError> {
    let head = branch_or_detached(dir)?;
    let (r, gh) = github(dir, &gh)?;
    let list = call(
        gh,
        &r,
        "GET",
        &r.api("pulls"),
        &[
            ("state", "all".into()),
            ("head", format!("{}:{head}", r.owner)),
            ("per_page", "20".into()),
            ("sort", "created".into()),
            ("direction", "desc".into()),
        ],
        None,
    )?;
    let Some(items) = list.body.as_array().filter(|_| ok(&list)) else {
        return Err(api_error("pr-api", &list));
    };
    let full = r.full_name();
    let mine: Vec<&Value> = items
        .iter()
        .filter(|p| {
            s(p, &["head", "ref"]) == Some(&head)
                && s(p, &["head", "repo", "full_name"]).is_some_and(|n| n.to_lowercase() == full)
        })
        .collect();
    let pick = mine
        .iter()
        .find(|p| s(p, &["state"]) == Some("open"))
        .or(mine.first());
    let Some(number) = pick.and_then(|p| p.get("number")).and_then(Value::as_u64) else {
        return Ok(None);
    };
    let path = r.api(&format!("pulls/{number}"));
    let mut detail = call(gh, &r, "GET", &path, &[], None)?;
    if ok(&detail)
        && s(&detail.body, &["state"]) == Some("open")
        && detail.body.get("mergeable").is_some_and(Value::is_null)
    {
        std::thread::sleep(retry);
        detail = call(gh, &r, "GET", &path, &[], None)?;
    }
    if !ok(&detail) || !detail.body.is_object() {
        return Err(api_error("pr-api", &detail));
    }
    let review = if s(&detail.body, &["state"]) == Some("open") {
        review_state(gh, &r, number)
    } else {
        (false, false)
    };
    Ok(Some(to_status(&detail.body, review)))
}

/// Desk `GATE_QUERY`.
const GATE_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      headRefOid
      reviewDecision
      mergeStateStatus
      commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100) { nodes {
        __typename
        ... on CheckRun { name status conclusion isRequired(pullRequestNumber: $number) }
        ... on StatusContext { context state isRequired(pullRequestNumber: $number) }
      } } } } } }
    }
  }
}";

/// Desk `checkState`.
pub fn check_state(node: &Value) -> PrCheckState {
    if let Some(state) = s(node, &["state"]) {
        return match state {
            "SUCCESS" => PrCheckState::Passed,
            "FAILURE" | "ERROR" => PrCheckState::Failed,
            _ => PrCheckState::Pending,
        };
    }
    if s(node, &["status"]) != Some("COMPLETED") {
        return PrCheckState::Pending;
    }
    match s(node, &["conclusion"]) {
        Some("SUCCESS") => PrCheckState::Passed,
        Some("NEUTRAL") | Some("SKIPPED") => PrCheckState::Skipped,
        _ => PrCheckState::Failed,
    }
}

/// Desk `pullRequestGate` (GraphQL).
pub fn gate(dir: &Path, gh: Gh<'_>, number: u64) -> Result<PrGate, ProtoError> {
    number_ok(number)?;
    let (r, gh) = github(dir, &gh)?;
    let body = json!({
        "query": GATE_QUERY,
        "variables": { "owner": r.owner, "name": r.repo, "number": number },
    });
    let res = call(gh, &r, "POST", "graphql", &[], Some(&body))?;
    let pr = res
        .body
        .pointer("/data/repository/pullRequest")
        .filter(|p| p.is_object());
    let Some(pr) = pr.filter(|_| ok(&res)) else {
        let errors = json!({ "errors": res.body.get("errors").cloned().unwrap_or(Value::Null) });
        return Err(ProtoError::new("pr-api")
            .arg("status", res.status.to_string())
            .arg("detail", api_message(&errors)));
    };
    let nodes = pr
        .pointer("/commits/nodes/0/commit/statusCheckRollup/contexts/nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(PrGate {
        head_sha: s(pr, &["headRefOid"]).unwrap_or_default().to_owned(),
        review_decision: s(pr, &["reviewDecision"]).map(str::to_owned),
        merge_state: s(pr, &["mergeStateStatus"]).unwrap_or("UNKNOWN").to_owned(),
        checks: nodes
            .iter()
            .map(|n| PrCheck {
                name: s(n, &["name"])
                    .or_else(|| s(n, &["context"]))
                    .unwrap_or("?")
                    .to_owned(),
                state: check_state(n),
                required: n
                    .get("isRequired")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
            .collect(),
    })
}

/// Desk `mergePullRequest`: only if the PR's head is still `sha`; `delete_branch` removes the
/// remote head branch afterwards (same repository only).
pub fn merge(
    dir: &Path,
    gh: Gh<'_>,
    number: u64,
    sha: &str,
    method: PrMergeMethod,
    delete_branch: bool,
) -> Result<PrMerged, ProtoError> {
    number_ok(number)?;
    if !is_hex(sha, 7, 64) {
        return Err(ProtoError::new("pr-bad-sha").arg("sha", sha));
    }
    let (r, gh) = github(dir, &gh)?;
    let path = r.api(&format!("pulls/{number}"));
    let current = call(gh, &r, "GET", &path, &[], None)?;
    if !ok(&current) || !current.body.is_object() {
        return Err(api_error("pr-api", &current));
    }
    let pr = &current.body;
    if pr.get("merged").and_then(Value::as_bool) == Some(true) {
        return Err(ProtoError::new("pr-already-merged").arg("number", number.to_string()));
    }
    if s(pr, &["state"]) != Some("open") {
        return Err(ProtoError::new("pr-not-open").arg("number", number.to_string()));
    }
    let head_sha = s(pr, &["head", "sha"]).unwrap_or_default().to_owned();
    if !head_sha.to_lowercase().starts_with(&sha.to_lowercase()) {
        let short = |x: &str| x.chars().take(7).collect::<String>();
        return Err(ProtoError::new("pr-head-moved")
            .arg("number", number.to_string())
            .arg("expected", short(sha))
            .arg("actual", short(&head_sha)));
    }
    let res = call(
        gh,
        &r,
        "PUT",
        &format!("{path}/merge"),
        &[],
        Some(&json!({ "sha": head_sha, "merge_method": method.as_str() })),
    )?;
    if !ok(&res) || res.body.get("merged").and_then(Value::as_bool) != Some(true) {
        return Err(api_error("pr-merge-refused", &res));
    }
    // The merged base, fetched: "커밋 보유" compares HEAD with the local origin/<base>.
    if let Some(base) = s(pr, &["base", "ref"])
        && check_branch(dir, base).is_ok()
    {
        let _ = git_net(dir, &["fetch", "--quiet", "origin", base], NET_TIMEOUT);
    }
    let mut branch_deleted = false;
    let same_repo =
        s(pr, &["head", "repo", "full_name"]).is_some_and(|n| n.to_lowercase() == r.full_name());
    if delete_branch
        && same_repo
        && let Some(head_ref) = s(pr, &["head", "ref"]).filter(|x| !x.is_empty())
    {
        let enc: Vec<String> = head_ref.split('/').map(encode_uri_component).collect();
        let del = call(
            gh,
            &r,
            "DELETE",
            &r.api(&format!("git/refs/heads/{}", enc.join("/"))),
            &[],
            None,
        );
        branch_deleted = del.is_ok_and(|d| d.status == 204 || d.status == 422);
    }
    Ok(PrMerged {
        sha: s(&res.body, &["sha"]).unwrap_or_default().to_owned(),
        branch_deleted,
        message: s(&res.body, &["message"]).unwrap_or_default().to_owned(),
    })
}

/// Desk `closePullRequest`: close open PR `number` without merging.
pub fn close(dir: &Path, gh: Gh<'_>, number: u64) -> Result<PrClosed, ProtoError> {
    number_ok(number)?;
    let (r, gh) = github(dir, &gh)?;
    let path = r.api(&format!("pulls/{number}"));
    let current = call(gh, &r, "GET", &path, &[], None)?;
    if !ok(&current) || !current.body.is_object() {
        return Err(api_error("pr-api", &current));
    }
    let pr = &current.body;
    if pr.get("merged").and_then(Value::as_bool) == Some(true) || s(pr, &["state"]) != Some("open")
    {
        return Err(ProtoError::new("pr-not-open").arg("number", number.to_string()));
    }
    let res = call(
        gh,
        &r,
        "PATCH",
        &path,
        &[],
        Some(&json!({ "state": "closed" })),
    )?;
    if !ok(&res) || s(&res.body, &["state"]) != Some("closed") {
        return Err(api_error("pr-close-failed", &res));
    }
    Ok(PrClosed {
        number,
        url: s(pr, &["html_url"]).unwrap_or_default().to_owned(),
    })
}

/// Desk `updatePullRequestBranch`: GitHub's "Update branch" (merge the base in, server side).
pub fn update_branch(
    dir: &Path,
    gh: Gh<'_>,
    number: u64,
    sha: &str,
) -> Result<Option<String>, ProtoError> {
    number_ok(number)?;
    let (r, gh) = github(dir, &gh)?;
    let body = if is_hex(sha, 40, 40) {
        json!({ "expected_head_sha": sha })
    } else {
        json!({})
    };
    let res = call(
        gh,
        &r,
        "PUT",
        &r.api(&format!("pulls/{number}/update-branch")),
        &[],
        Some(&body),
    )?;
    if !ok(&res) {
        return Err(api_error("pr-update-refused", &res));
    }
    Ok(s(&res.body, &["message"]).map(str::to_owned))
}

// ---- PR file ------------------------------------------------------------------------------

/// Desk `readPrFile`: first line = title (leading `#`s dropped unless `literal`), blank line,
/// body.
pub fn read_pr_file(path: &Path, literal: bool) -> Result<(String, String), ProtoError> {
    let text = std::fs::read_to_string(path).map_err(|_| ProtoError::new("pr-no-text"))?;
    let text = text.trim_start().replace("\r\n", "\n");
    let text = text.trim();
    let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
    let title = if literal {
        first
    } else {
        first.trim_start_matches('#').trim_start()
    }
    .trim()
    .to_owned();
    if title.is_empty() {
        return Err(ProtoError::new("pr-empty-title"));
    }
    Ok((title, rest.trim().to_owned()))
}

fn meta_path(pr_file: &Path) -> PathBuf {
    let mut s = pr_file.as_os_str().to_owned();
    s.push(".meta.json");
    PathBuf::from(s)
}

fn read_meta(pr_file: &Path) -> serde_json::Map<String, Value> {
    std::fs::read_to_string(meta_path(pr_file))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// Desk `writePrMeta`: merge `patch` into the meta beside the PR file (best effort).
fn write_meta(pr_file: &Path, patch: Value) {
    let mut m = read_meta(pr_file);
    if let Some(p) = patch.as_object() {
        for (k, v) in p {
            m.insert(k.clone(), v.clone());
        }
    }
    let _ = std::fs::write(meta_path(pr_file), Value::Object(m).to_string());
}

fn reason_meta(pr_file: &Path, reason: &ProtoError) {
    write_meta(
        pr_file,
        json!({ "reason": serde_json::to_value(reason).unwrap_or(Value::Null) }),
    );
}

fn remove_pr_file(pr_file: &Path) {
    let _ = std::fs::remove_file(pr_file);
    let _ = std::fs::remove_file(meta_path(pr_file));
}

/// Desk `resetPrFile`: remove a stale PR file (a missing file then means the agent wrote none)
/// and make its folder.
pub fn reset_pr_file(pr_file: &Path) -> Result<(), ProtoError> {
    remove_pr_file(pr_file);
    if let Some(dir) = pr_file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| {
            core_error(&ide_core::Error::io(
                ide_core::IoAction::CreatingDirectory,
                dir,
                e,
            ))
        })?;
    }
    Ok(())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Desk `pullRequestLeftover`.
pub fn leftover(
    dir: &Path,
    pr_file: Option<&Path>,
    base: Option<&str>,
) -> Result<PrLeftover, ProtoError> {
    let head = git_text(dir, &["rev-parse", "HEAD"])?;
    let mut file = None;
    if let Some(path) = pr_file.filter(|p| p.is_file()) {
        let (title, body) = read_pr_file(path, false).unwrap_or_default();
        let meta = read_meta(path);
        let used_for = meta.get("head").and_then(Value::as_str).map(str::to_owned);
        let mut matches = LeftoverMatch::Unknown;
        let mut new_commits = Vec::new();
        if let Some(h) = &used_for {
            if *h == head {
                matches = LeftoverMatch::Same;
            } else if is_hex(h, 7, 64) && git_ok(dir, &["merge-base", "--is-ancestor", h, "HEAD"]) {
                matches = LeftoverMatch::Ahead;
                new_commits = commit_list(dir, &format!("{h}..HEAD"));
            } else {
                matches = LeftoverMatch::Diverged;
            }
        }
        let written_at = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        file = Some(PrLeftoverFile {
            path: path.display().to_string(),
            title,
            body,
            written_at,
            used_for,
            reason: meta
                .get("reason")
                .and_then(|v| serde_json::from_value(v.clone()).ok()),
            matches,
            new_commits,
        });
    }
    let mut changes = None;
    let mut commits = Vec::new();
    if let Some(base) = base.filter(|b| !b.is_empty()) {
        check_branch(dir, base)?;
        let r = format!("origin/{base}");
        if git_ok(dir, &["rev-parse", "--verify", "--quiet", &r]) {
            let same = git_ok(dir, &["diff", "--quiet", &r, "HEAD"]);
            changes = Some(!same);
            if !same {
                commits = commit_list(dir, &format!("{r}..HEAD"));
            }
        }
    }
    Ok(PrLeftover {
        head,
        file,
        changes,
        commits,
    })
}

// ---- local state --------------------------------------------------------------------------

/// Desk `undeliveredCommits` (without a merged-PR record): with an upstream `upstream..HEAD`,
/// else `HEAD --not --remotes`; at most 500.
fn undelivered_commits(dir: &Path) -> Vec<PrCommit> {
    let upstream = git_or_empty(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    );
    let range_owned;
    let mut args = vec!["log", "--max-count=500", "--format=%h%x09%s"];
    if upstream.is_empty() {
        args.extend(["HEAD", "--not", "--remotes"]);
    } else {
        range_owned = format!("{upstream}..HEAD");
        args.push(&range_owned);
    }
    parse_commits(&git_or_empty(dir, &args))
}

/// Desk `pendingWork`.
pub fn pending_work(dir: &Path) -> PendingWork {
    let branch = current_branch(dir);
    let commits = undelivered_commits(dir);
    let published = !branch.is_empty()
        && branch != "HEAD"
        && git_ok(
            dir,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/remotes/origin/{branch}"),
            ],
        );
    PendingWork {
        commit_count: commits.len() as u32,
        commits: commits.into_iter().take(30).collect(),
        files: status_files(dir),
        published,
        branch,
    }
}

/// Desk `prDrift`.
pub fn drift(dir: &Path, since: Option<&str>, allowed: &[String]) -> Result<PrDrift, ProtoError> {
    let head = git_text(dir, &["rev-parse", "HEAD"])?;
    let files = status_files(dir);
    let Some(since) = since.filter(|s| !s.is_empty() && *s != head) else {
        return Ok(PrDrift {
            head,
            rewritten: false,
            commits: Vec::new(),
            files,
        });
    };
    let ancestor =
        is_hex(since, 7, 40) && git_ok(dir, &["merge-base", "--is-ancestor", since, "HEAD"]);
    if !ancestor {
        return Ok(PrDrift {
            head,
            rewritten: true,
            commits: Vec::new(),
            files,
        });
    }
    let log = git_text(
        dir,
        &[
            "log",
            "--first-parent",
            "--reverse",
            "--max-count=50",
            "--format=%H%x1f%h%x1f%P%x1f%s",
            &format!("{since}..HEAD"),
        ],
    )?;
    let ok_paths: HashSet<String> = allowed.iter().map(|p| p.replace('\\', "/")).collect();
    let commits = log
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let mut f = line.splitn(4, '\x1f');
            let full = f.next().unwrap_or_default().to_owned();
            let sha = f.next().unwrap_or_default().to_owned();
            let parents = f.next().unwrap_or_default();
            let subject = f.next().unwrap_or_default().to_owned();
            let mut expected = false;
            if !ok_paths.is_empty() && parents.split_whitespace().count() == 1 {
                let touched = crate::git::run(
                    dir,
                    &[
                        "diff-tree",
                        "--no-commit-id",
                        "--name-only",
                        "-r",
                        "-z",
                        &full,
                    ],
                )
                .unwrap_or_default();
                let touched = String::from_utf8_lossy(&touched).into_owned();
                let paths: Vec<&str> = touched.split('\0').filter(|p| !p.is_empty()).collect();
                expected = !paths.is_empty() && paths.iter().all(|p| ok_paths.contains(*p));
            }
            DriftCommit {
                sha,
                subject,
                full,
                expected,
            }
        })
        .collect();
    Ok(PrDrift {
        head,
        rewritten: false,
        commits,
        files,
    })
}

/// Desk `unmergedWork` / `measureUnmerged` (no cache): `None` when `origin/<base>` or HEAD is
/// unknown.
pub fn unmerged(dir: &Path, base: &str) -> Option<UnmergedWork> {
    if base.is_empty() || check_branch(dir, base).is_err() {
        return None;
    }
    let r = format!("origin/{base}");
    let base_sha = git_or_empty(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{r}^{{commit}}"),
        ],
    );
    let head = git_or_empty(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]);
    if base_sha.is_empty() || head.is_empty() {
        return None;
    }
    if git_ok(dir, &["diff", "--quiet", &r, "HEAD"]) {
        return Some(UnmergedWork {
            commits: 0,
            conflict: false,
        });
    }
    let fork = git_or_empty(dir, &["merge-base", &r, "HEAD"]);
    if fork == base_sha {
        let n = git_or_empty(
            dir,
            &["rev-list", "--count", "--no-merges", &format!("{r}..HEAD")],
        );
        return Some(UnmergedWork {
            commits: n.parse().unwrap_or(0),
            conflict: false,
        });
    }
    let merged = git_text(dir, &["merge-tree", "--write-tree", &r, "HEAD"]);
    let conflict = merged.is_err();
    let base_tree = git_or_empty(dir, &["rev-parse", &format!("{r}^{{tree}}")]);
    if let Ok(m) = &merged
        && m.lines().next().unwrap_or("").trim() == base_tree
    {
        return Some(UnmergedWork {
            commits: 0,
            conflict: false,
        });
    }
    let own = git_or_empty(
        dir,
        &[
            "log",
            "--cherry-pick",
            "--right-only",
            "--no-merges",
            "--format=%h",
            &format!("{r}...HEAD"),
        ],
    );
    let mut n = own.lines().filter(|l| !l.is_empty()).count() as u32;
    if !conflict
        && n > 1
        && let Some(since) = contained_since(dir, &r, &base_tree, n.min(20))
    {
        n = since;
    }
    Some(UnmergedWork {
        commits: n.max(1),
        conflict,
    })
}

/// Desk `containedSince`.
fn contained_since(dir: &Path, r: &str, base_tree: &str, limit: u32) -> Option<u32> {
    let chain = git_or_empty(
        dir,
        &[
            "rev-list",
            "--first-parent",
            &format!("--max-count={}", limit + 1),
            "HEAD",
            &format!("^{r}"),
        ],
    );
    let chain: Vec<&str> = chain.lines().filter(|l| !l.is_empty()).collect();
    for c in chain.iter().skip(1) {
        let tree = git_text(dir, &["merge-tree", "--write-tree", r, c])
            .map(|o| o.lines().next().unwrap_or("").trim().to_owned())
            .unwrap_or_default();
        if tree != base_tree {
            continue;
        }
        let n = git_or_empty(
            dir,
            &["rev-list", "--count", "--no-merges", &format!("{c}..HEAD")],
        );
        return Some(n.parse().unwrap_or(0));
    }
    None
}

/// Desk `remoteBranches`: origin's branches (`ls-remote`), its default and the current branch.
pub fn remote_branches(dir: &Path) -> Result<PrTargets, ProtoError> {
    let out = git_net(
        dir,
        &["ls-remote", "--symref", "origin", "HEAD", "refs/heads/*"],
        NET_TIMEOUT,
    )?;
    let mut branches = Vec::new();
    let mut default_branch = None;
    for line in out.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("ref: refs/heads/")
            && let Some((name, what)) = rest.split_once(char::is_whitespace)
            && what.trim() == "HEAD"
        {
            default_branch = Some(name.to_owned());
        }
        if let Some((sha, r)) = line.split_once(char::is_whitespace)
            && is_hex(sha, 4, 64)
            && let Some(name) = r.trim().strip_prefix("refs/heads/")
        {
            branches.push(name.to_owned());
        }
    }
    branches.sort();
    Ok(PrTargets {
        branches,
        default_branch,
        current: current_branch(dir),
    })
}

// ---- commit-based PR text -----------------------------------------------------------------

fn changed() -> ProtoError {
    ProtoError::new("pr-changed")
}

/// Desk `commitPrPreview`: read-only PR text from the commits `origin/<base>` lacks.
pub fn commit_preview(dir: &Path, base: &str, push: bool) -> Result<CommitPrPreview, ProtoError> {
    let branch = git_text(dir, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    check_branch(dir, base)?;
    check_branch(dir, &branch)?;
    let head = git_text(dir, &["rev-parse", "HEAD"])?;
    let base_ref = format!("refs/remotes/origin/{base}");
    git_net(
        dir,
        &[
            "fetch",
            "--quiet",
            "origin",
            &format!("+refs/heads/{base}:{base_ref}"),
        ],
        NET_TIMEOUT,
    )?;
    let source_ref = if push {
        "HEAD".to_owned()
    } else {
        format!("refs/remotes/origin/{branch}")
    };
    if !push {
        git_net(
            dir,
            &[
                "fetch",
                "--quiet",
                "origin",
                &format!("+refs/heads/{branch}:{source_ref}"),
            ],
            NET_TIMEOUT,
        )?;
    }
    let source_head = git_text(dir, &["rev-parse", &source_ref])?;
    let base_head = git_text(dir, &["rev-parse", &base_ref])?;
    let commits: Vec<PrCommit> = parse_commits(&git_text(
        dir,
        &[
            "log",
            "--no-merges",
            "--reverse",
            "--format=%h%x09%s",
            &format!("{base_head}..{source_head}"),
        ],
    )?)
    .into_iter()
    .map(|c| PrCommit {
        subject: c.subject.trim().to_owned(),
        ..c
    })
    .collect();
    let Some(newest) = commits.last() else {
        return Err(ProtoError::new("pr-no-commits").arg("base", base_ref));
    };
    let stat = String::from_utf8_lossy(&crate::git::run(
        dir,
        &["diff", "--stat", &format!("{base_head}...{source_head}")],
    )?)
    .trim_end()
    .to_owned();
    let title = if commits.len() == 1 {
        newest.subject.clone()
    } else {
        format!("{} 외 {}개", newest.subject, commits.len() - 1)
    };
    let mut body: Vec<String> = vec!["## 커밋".into(), String::new()];
    body.extend(
        commits
            .iter()
            .map(|c| format!("- {} ({})", c.subject, c.sha)),
    );
    if !stat.is_empty() {
        body.extend([
            String::new(),
            "## 변경 파일".into(),
            String::new(),
            "```".into(),
            stat,
            "```".into(),
        ]);
    }
    body.extend([String::new(), "_커밋 기록으로 작성했어요 (ide)._".into()]);
    let preview = CommitPrPreview {
        title,
        body: body.join("\n"),
        commits,
        head,
        source_head,
        base_head,
        branch,
    };
    // Desk `validateCommitPrSnapshot`: re-read the refs after assembling.
    let heads = git_text(dir, &["rev-parse", "HEAD", &source_ref, &base_ref])?;
    let heads: Vec<&str> = heads.split_whitespace().collect();
    if heads.len() != 3
        || heads[0] != preview.head
        || heads[1] != preview.source_head
        || heads[2] != preview.base_head
        || current_branch(dir) != preview.branch
    {
        return Err(changed());
    }
    Ok(preview)
}

/// Desk `writeCommitPrText`: write the PR file from the commits; returns the title.
pub fn commit_text(
    dir: &Path,
    base: &str,
    pr_file: &Path,
    options: &CommitPrTextOptions,
) -> Result<String, ProtoError> {
    if options.expected_base.as_deref().is_some_and(|b| b != base) {
        return Err(changed());
    }
    let preview = commit_preview(dir, base, options.push.unwrap_or(true))?;
    let pins = [
        (&options.expected_head, &preview.head),
        (&options.expected_source_head, &preview.source_head),
        (&options.expected_base_head, &preview.base_head),
        (&options.expected_branch, &preview.branch),
    ];
    if pins
        .iter()
        .any(|(want, got)| want.as_ref().is_some_and(|w| w != *got))
    {
        return Err(changed());
    }
    let title = match &options.title {
        None => preview.title.clone(),
        Some(t) => t.trim().to_owned(),
    };
    let raw = options.title.as_deref().unwrap_or(&title);
    if title.is_empty() || raw.contains(['\r', '\n']) {
        return Err(ProtoError::new("pr-bad-title"));
    }
    if let Some(d) = pr_file.parent() {
        std::fs::create_dir_all(d).map_err(|e| {
            core_error(&ide_core::Error::io(
                ide_core::IoAction::CreatingDirectory,
                d,
                e,
            ))
        })?;
    }
    std::fs::write(pr_file, format!("{title}\n\n{}\n", preview.body)).map_err(|e| {
        core_error(&ide_core::Error::io(
            ide_core::IoAction::Writing,
            pr_file,
            e,
        ))
    })?;
    Ok(title)
}

/// The refs a commit-based PR was approved at (Desk `CommitPrTextOptions` with every field).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    head: String,
    source_head: Option<String>,
    base_head: String,
    branch: String,
    base: String,
    push: bool,
}

/// Desk `validateCommitPrCreation`: fresh remote reads, then local refs.
fn validate_creation(
    dir: &Path,
    base: &str,
    branch: &str,
    push: bool,
    expected: &CommitPrTextOptions,
    published: bool,
) -> Result<Snapshot, ProtoError> {
    check_branch(dir, base)?;
    check_branch(dir, branch)?;
    let base_ref = format!("refs/heads/{base}");
    let branch_ref = format!("refs/heads/{branch}");
    let mut refs = vec!["ls-remote", "origin", base_ref.as_str()];
    if !push || published {
        refs.push(&branch_ref);
    }
    let remote = git_net(dir, &refs, NET_TIMEOUT)?;
    let shas: HashMap<&str, &str> = remote
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let sha = it.next()?;
            Some((it.next()?, sha))
        })
        .collect();
    let base_head = shas.get(base_ref.as_str()).map(|s| (*s).to_owned());
    let remote_source = shas.get(branch_ref.as_str()).map(|s| (*s).to_owned());
    let head = git_text(dir, &["rev-parse", "HEAD"])?;
    let current = current_branch(dir);
    let source_head = if push {
        Some(head.clone())
    } else {
        remote_source.clone()
    };
    let Some(base_head) = base_head else {
        return Err(changed());
    };
    if ((!push || published) && remote_source.is_none())
        || current != branch
        || (published && remote_source != source_head)
    {
        return Err(changed());
    }
    let actual = Snapshot {
        head,
        source_head,
        base_head,
        branch: current,
        base: base.to_owned(),
        push,
    };
    let mismatch = expected
        .expected_head
        .as_ref()
        .is_some_and(|v| *v != actual.head)
        || expected
            .expected_source_head
            .as_ref()
            .is_some_and(|v| Some(v) != actual.source_head.as_ref())
        || expected
            .expected_base_head
            .as_ref()
            .is_some_and(|v| *v != actual.base_head)
        || expected
            .expected_branch
            .as_ref()
            .is_some_and(|v| *v != actual.branch)
        || expected
            .expected_base
            .as_ref()
            .is_some_and(|v| *v != actual.base)
        || expected.push.is_some_and(|v| v != actual.push);
    if mismatch {
        return Err(changed());
    }
    Ok(actual)
}

impl Snapshot {
    fn options(&self) -> CommitPrTextOptions {
        CommitPrTextOptions {
            expected_base: Some(self.base.clone()),
            push: Some(self.push),
            title: None,
            expected_head: Some(self.head.clone()),
            expected_source_head: self.source_head.clone(),
            expected_base_head: Some(self.base_head.clone()),
            expected_branch: Some(self.branch.clone()),
        }
    }
}

/// Desk `verifiedCommit`: `sha` as a full commit id that HEAD contains.
fn verified_commit(dir: &Path, sha: &str) -> Result<String, ProtoError> {
    if !is_hex(sha, 7, 40) {
        return Err(ProtoError::new("pr-bad-sha").arg("sha", sha));
    }
    let full = git_text(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{sha}^{{commit}}"),
        ],
    )
    .map_err(|_| ProtoError::new("pr-bad-sha").arg("sha", sha))?;
    if !git_ok(dir, &["merge-base", "--is-ancestor", &full, "HEAD"]) {
        return Err(
            ProtoError::new("pr-not-on-branch").arg("sha", sha.chars().take(7).collect::<String>())
        );
    }
    Ok(full)
}

/// `encodeURIComponent`.
fn encode_uri_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `URLSearchParams` value encoding (`application/x-www-form-urlencoded`).
fn form_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"*-._".contains(&b) {
            out.push(b as char);
        } else if b == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Desk `compareUrl`: the prefilled "new PR" page (GitLab: new merge request).
pub fn compare_url(r: &RemoteRepo, base: &str, head: &str, title: &str, body: &str) -> String {
    let q = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(k, v)| format!("{}={}", form_encode(k), form_encode(v)))
            .collect::<Vec<_>>()
            .join("&")
    };
    if r.gitlab() {
        return format!(
            "{}/-/merge_requests/new?{}",
            r.web,
            q(&[
                ("merge_request[source_branch]", head),
                ("merge_request[target_branch]", base),
                ("merge_request[title]", title),
                ("merge_request[description]", body),
            ])
        );
    }
    format!(
        "{}/compare/{}...{}?{}",
        r.web,
        encode_uri_component(base),
        encode_uri_component(head),
        q(&[("expand", "1"), ("title", title), ("body", body)])
    )
}

/// Desk `trackOrigin`: make the branch track `origin/<head>` after a push by sha (best effort).
fn track_origin(dir: &Path, head: &str) {
    if git_or_empty(dir, &["branch", "--show-current"]) != head {
        return;
    }
    let _ = crate::git::run(
        dir,
        &["branch", &format!("--set-upstream-to=origin/{head}")],
    );
}

/// Inputs of [`create`].
#[derive(Debug, Clone)]
pub struct CreateRequest<'a> {
    /// Target branch on origin.
    pub base: &'a str,
    /// The PR file (title/body).
    pub pr_file: &'a Path,
    /// Draft PR.
    pub draft: bool,
    /// Push first (false: the PR is the branch as on origin).
    pub push: bool,
    /// Push this commit as the branch instead of HEAD.
    pub push_sha: Option<&'a str>,
    /// A commit-based request's pins.
    pub options: Option<&'a CommitPrTextOptions>,
}

/// Errors meaning "no usable login for the API": `create` returns the prefilled page.
const NO_API: [&str; 4] = [
    "gh-auth",
    "gh-missing",
    "github-auth",
    "github-no-credentials",
];

/// Desk `createPullRequest`: push the branch (unless `push` false) and open a PR into `base`
/// from the PR file. Without the API (GitLab, no credential token and no usable `gh`) the prefilled page
/// is returned (`Page`); an API refusal is `Failed` with that page.
pub fn create(
    dir: &Path,
    gh: Gh<'_>,
    req: &CreateRequest<'_>,
) -> Result<PullRequestResult, ProtoError> {
    let CreateRequest {
        base,
        pr_file,
        draft,
        push,
        push_sha,
        options,
    } = *req;
    let head = git_text(dir, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let mut snapshot: Option<Snapshot> = None;
    if let Some(o) = options {
        if o.push.is_some_and(|p| p != push)
            || o.expected_base.as_deref().is_some_and(|b| b != base)
            || (push
                && push_sha.is_some()
                && o.expected_head.is_some()
                && push_sha != o.expected_head.as_deref())
        {
            return Err(changed());
        }
        snapshot = Some(validate_creation(dir, base, &head, push, o, false)?);
    }
    if head == "HEAD" {
        return Err(ProtoError::new("pr-detached"));
    }
    if head == base {
        return Err(ProtoError::new("pr-same-branch").arg("branch", head));
    }
    let remote_sha = if push {
        String::new()
    } else {
        match snapshot.as_ref().and_then(|s| s.source_head.clone()) {
            Some(s) => s,
            None => git_or_empty(
                dir,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/remotes/origin/{head}"),
                ],
            ),
        }
    };
    if !push && remote_sha.is_empty() {
        return Err(ProtoError::new("pr-no-remote-branch").arg("branch", head));
    }
    let repo = origin_repo(dir)?;
    // The API, or why it is not available (the page is returned instead).
    let mut api: Result<&dyn GhApi, ProtoError> = if repo.gitlab() {
        Err(ProtoError::new("pr-gitlab-page"))
    } else {
        gh
    };
    if let Ok(g) = api {
        match lookup_open(g, &repo, &head) {
            Ok(existing) => {
                if let Some(s) = &snapshot {
                    validate_creation(dir, base, &head, push, &s.options(), false)?;
                }
                if let Some(e) = existing {
                    remove_pr_file(pr_file);
                    return Ok(e);
                }
            }
            // Desk: no token -> straight to the page.
            Err(e) if NO_API.contains(&e.id.as_str()) => api = Err(e),
            Err(e) => return Err(e),
        }
    }
    let (title, body) = read_pr_file(pr_file, options.is_some())?;
    let page = compare_url(&repo, base, &head, &title, &body);
    let pushed = match push_sha.filter(|_| push) {
        Some(sha) => verified_commit(dir, sha)?,
        None => String::new(),
    };
    if let Some(s) = &snapshot
        && !pushed.is_empty()
        && Some(&pushed) != s.source_head.as_ref()
    {
        return Err(changed());
    }
    let sha = if push {
        if pushed.is_empty() {
            git_text(dir, &["rev-parse", "HEAD"])?
        } else {
            pushed.clone()
        }
    } else {
        remote_sha
    };
    if let Some(s) = &snapshot {
        validate_creation(dir, base, &head, push, &s.options(), false)?;
    }
    write_meta(
        pr_file,
        json!({ "head": sha, "branch": head, "base": base, "attemptAt": now_secs() }),
    );
    if push {
        let pushed_by_sha = if !pushed.is_empty() {
            Some(pushed.clone())
        } else {
            snapshot.as_ref().map(|s| s.head.clone())
        };
        let res = match &pushed_by_sha {
            // Only up to that commit: later local commits stay local.
            Some(sha) => git_net(
                dir,
                &["push", "-u", "origin", &format!("{sha}:refs/heads/{head}")],
                PUSH_TIMEOUT,
            ),
            None => git_net(dir, &["push", "-u", "origin", "HEAD"], PUSH_TIMEOUT),
        };
        if let Err(e) = res {
            reason_meta(
                pr_file,
                &ProtoError::new("pr-push-failed")
                    .arg("detail", e.get("detail").unwrap_or(&e.id).to_owned()),
            );
            return Err(e);
        }
        if pushed_by_sha.is_some() {
            track_origin(dir, &head);
        }
    }
    let gh = match api {
        Ok(g) => g,
        Err(reason) => {
            return Ok(PullRequestResult {
                kind: PrResultKind::Page,
                url: page,
                title,
                number: None,
                reason: Some(reason),
                base: None,
                mergeable_state: None,
                session_base: None,
            });
        }
    };
    if let Some(s) = &snapshot {
        validate_creation(dir, base, &head, push, &s.options(), push)?;
    }
    let payload =
        json!({ "title": title, "body": body, "head": head, "base": base, "draft": draft });
    let res = match call(gh, &repo, "POST", &repo.api("pulls"), &[], Some(&payload)) {
        Ok(r) => r,
        Err(err) => {
            // The POST may have succeeded remotely: reconcile once, never repeat it.
            if let Ok(Some(e)) = lookup_open(gh, &repo, &head) {
                remove_pr_file(pr_file);
                return Ok(e);
            }
            let reason = ProtoError::new("pr-create-unconfirmed")
                .arg("detail", err.get("detail").unwrap_or(&err.id).to_owned());
            reason_meta(pr_file, &reason);
            return Err(reason);
        }
    };
    if ok(&res)
        && let Some(url) = s(&res.body, &["html_url"])
    {
        remove_pr_file(pr_file);
        return Ok(PullRequestResult {
            kind: PrResultKind::Created,
            url: url.to_owned(),
            title,
            number: res.body.get("number").and_then(Value::as_u64),
            reason: None,
            base: None,
            mergeable_state: None,
            session_base: None,
        });
    }
    if let Ok(Some(e)) = lookup_open(gh, &repo, &head) {
        remove_pr_file(pr_file);
        return Ok(e);
    }
    let reason = api_error("pr-api", &res);
    reason_meta(pr_file, &reason);
    Ok(PullRequestResult {
        kind: PrResultKind::Failed,
        url: page,
        title,
        number: None,
        reason: Some(reason),
        base: None,
        mergeable_state: None,
        session_base: None,
    })
}

// ---- RPC dispatch -------------------------------------------------------------------------

/// A `(session, repo)` worktree as the PR requests see it.
#[derive(Debug, Clone)]
pub struct PrTarget {
    /// The worktree (jail root).
    pub dir: PathBuf,
    /// Registered repository name.
    pub repo_name: String,
    /// The session's base branch for it (`""` = none).
    pub session_base: String,
    /// The worktree's recorded branch.
    pub branch: String,
    /// The PR file (workspace sessions only: it lives in the session folder).
    pub pr_file: Option<PathBuf>,
}

impl PrTarget {
    fn pr_file(&self) -> Result<&Path, ProtoError> {
        self.pr_file
            .as_deref()
            .ok_or_else(|| ProtoError::new("pr-needs-workspace"))
    }
}

/// Handle a `pr.*` / `prompt.*` / `session.send_prompt` request (blocking).
pub(crate) fn handle(
    ctx: &crate::service::UserCtx,
    req: ide_proto::Request,
) -> Result<ide_proto::Response, ProtoError> {
    use ide_proto::{Request as Q, Response as A};
    let gh = ctx.github();
    let g: Gh<'_> = Ok(&gh);
    match req {
        Q::PrStatus { session, repo } => {
            let t = ctx.pr_target(&session, &repo)?;
            status(&t.dir, g.clone()).map(A::PrStatus)
        }
        Q::PrGate {
            session,
            repo,
            number,
        } => {
            let t = ctx.pr_target(&session, &repo)?;
            gate(&t.dir, g.clone(), number).map(A::PrGate)
        }
        Q::PrFindOpen { session, repo } => {
            let t = ctx.pr_target(&session, &repo)?;
            find_open(&t.dir, g.clone(), &t.session_base).map(A::PrFound)
        }
        Q::PrRemoteBranches { session, repo } => {
            let t = ctx.pr_target(&session, &repo)?;
            remote_branches(&t.dir).map(A::PrTargets)
        }
        Q::PrPendingWork { session, repo } => {
            let t = ctx.pr_target(&session, &repo)?;
            Ok(A::PrPendingWork(pending_work(&t.dir)))
        }
        Q::PrLeftover {
            session,
            repo,
            base,
        } => {
            let t = ctx.pr_target(&session, &repo)?;
            leftover(&t.dir, t.pr_file.as_deref(), base.as_deref()).map(A::PrLeftover)
        }
        Q::PrResetFile { session, repo } => {
            let t = ctx.pr_target(&session, &repo)?;
            let f = t.pr_file()?;
            reset_pr_file(f)?;
            Ok(A::PrFile {
                path: f.display().to_string(),
            })
        }
        Q::PrDrift {
            session,
            repo,
            since,
            allowed,
        } => {
            let t = ctx.pr_target(&session, &repo)?;
            drift(&t.dir, since.as_deref(), &allowed).map(A::PrDrift)
        }
        Q::PrUnmerged { session, repo } => {
            let t = ctx.pr_target(&session, &repo)?;
            Ok(A::PrUnmerged(unmerged(&t.dir, &t.session_base)))
        }
        Q::PrCommitPreview {
            session,
            repo,
            base,
            push,
        } => {
            let t = ctx.pr_target(&session, &repo)?;
            commit_preview(&t.dir, &base, push).map(A::PrCommitPreview)
        }
        Q::PrCommitText {
            session,
            repo,
            base,
            options,
        } => {
            let t = ctx.pr_target(&session, &repo)?;
            commit_text(&t.dir, &base, t.pr_file()?, &options).map(A::PrTitle)
        }
        Q::PrCreate {
            session,
            repo,
            base,
            draft,
            push,
            push_sha,
            options,
        } => {
            let t = ctx.pr_target(&session, &repo)?;
            let res = create(
                &t.dir,
                g.clone(),
                &CreateRequest {
                    base: &base,
                    pr_file: t.pr_file()?,
                    draft,
                    push,
                    push_sha: push_sha.as_deref(),
                    options: options.as_ref(),
                },
            )?;
            ctx.audit_action(
                "pr.create",
                &format!("{session} {repo} {:?} {}", res.kind, res.url),
            );
            Ok(A::PrCreated(res))
        }
        Q::PrMerge {
            session,
            repo,
            number,
            sha,
            method,
            delete_branch,
        } => {
            let t = ctx.pr_target(&session, &repo)?;
            let res = merge(&t.dir, g.clone(), number, &sha, method, delete_branch)?;
            ctx.audit_action("pr.merge", &format!("{session} {repo} #{number} {sha}"));
            Ok(A::PrMerged(res))
        }
        Q::PrClose {
            session,
            repo,
            number,
        } => {
            let t = ctx.pr_target(&session, &repo)?;
            let res = close(&t.dir, g.clone(), number)?;
            ctx.audit_action("pr.close", &format!("{session} {repo} #{number}"));
            Ok(A::PrClosed(res))
        }
        Q::PrUpdateBranch {
            session,
            repo,
            number,
            sha,
        } => {
            let t = ctx.pr_target(&session, &repo)?;
            let message = update_branch(&t.dir, g.clone(), number, &sha)?;
            ctx.audit_action("pr.update_branch", &format!("{session} {repo} #{number}"));
            Ok(A::PrBranchUpdated { message })
        }
        Q::PromptCommit {
            session,
            repo,
            files,
            all,
        } => {
            let t = ctx.pr_target(&session, &repo)?;
            Ok(A::Prompt(ide_core::commit_prompt(
                &t.repo_name,
                &t.dir.display().to_string(),
                &files,
                all,
            )))
        }
        Q::PromptPr {
            session,
            repo,
            target,
            exclude,
        } => {
            let t = ctx.pr_target(&session, &repo)?;
            check_branch(&t.dir, &target)?;
            let f = t.pr_file()?;
            Ok(A::Prompt(ide_core::pr_prompt(
                &t.repo_name,
                &t.dir.display().to_string(),
                &target,
                &f.display().to_string(),
                exclude.commits,
                exclude.files,
                Some(&t.branch),
            )))
        }
        Q::SessionSendPrompt { session, text } => {
            ctx.send_prompt(&session, &text)?;
            Ok(A::Unit)
        }
        other => Err(ProtoError::new("bad-request").arg("method", other.method())),
    }
}

// ---- prompts ------------------------------------------------------------------------------

/// Submit `text` to the session's process the way its provider takes a prompt.
pub fn send_prompt(
    host: &crate::pty::TermHost,
    input: ide_agents::PromptInput,
    text: &str,
) -> Result<(), ProtoError> {
    if !host.is_running() {
        return Err(ProtoError::new("session-not-running").arg("id", host.session()));
    }
    let writes =
        ide_agents::prompt_writes(input, text, host.bracketed_paste()).map_err(|r| match r {
            ide_agents::PromptRefusal::Empty => ProtoError::new("prompt-empty"),
            ide_agents::PromptRefusal::NotReady => {
                ProtoError::new("prompt-not-ready").arg("id", host.session())
            }
        })?;
    for w in writes {
        if w.delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(w.delay_ms));
        }
        host.write(w.bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
