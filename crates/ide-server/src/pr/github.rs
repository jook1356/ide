//! How `pr.*` reaches GitHub, Hermes Desk's way first (`main/github.ts`): the token git's own
//! credential store has for the host (`git credential fill`, never prompting; Git Credential
//! Manager on Windows, i.e. whatever lets `git push` reach GitHub) and the REST / GraphQL API
//! over HTTPS directly. The `gh` CLI is only the fallback for hosts git has no token for.
//!
//! [`GithubTransport`] implements [`GhApi`], so the PR functions do not know which way a call
//! went. Rules:
//! - git has a token -> HTTPS with `Authorization: Bearer <token>`; any HTTP answer is the
//!   answer, except 401: the token is forgotten (the next call asks git again) and the call is
//!   made once more through `gh` if it is installed, else `github-auth` (`host`).
//! - no token (no credential, helper failed, 20 s timeout) -> `gh`; `gh` missing too ->
//!   `github-no-credentials` (`host`).
//! - no HTTP answer with a token (network, TLS, 20 s timeout) -> `github-failed` (`detail`),
//!   no `gh` retry (Desk: the `fetch` error).
//!
//! The token is never logged, put in an error or written to the audit log.

use std::collections::HashMap;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ide_proto::ProtoError;
use serde_json::Value;

use super::{GhApi, GhRequest, GhResponse, form_encode, run_timeout};

/// Env var replacing the API root of every host (tests point it at a local fake GitHub).
pub const API_ENV: &str = "IDE_GITHUB_API";
/// Desk: `AbortSignal.timeout(20_000)` per `fetch`.
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Desk: `git credential fill` gets 20 s.
const CREDENTIAL_TIMEOUT: Duration = Duration::from_secs(20);
/// Desk `TOKEN_TTL_MS`: a found token is reused this long.
const TOKEN_TTL: Duration = Duration::from_secs(10 * 60);
/// Desk `NO_TOKEN_TTL_MS`: "no credentials" is asked again sooner (the user may sign in).
const NO_TOKEN_TTL: Duration = Duration::from_secs(60);
/// Response bodies up to this size (PR lists with `per_page=100` stay far below).
const BODY_LIMIT: u64 = 32 * 1024 * 1024;

/// Desk `githubApi`: `https://api.github.com` for github.com, else `https://<host>/api/v3`.
pub fn api_base(host: &str) -> String {
    if host == "github.com" {
        "https://api.github.com".to_owned()
    } else {
        format!("https://{host}/api/v3")
    }
}

/// The URL of `path` under `base`; `graphql` as Desk derives it (`.../api/v3` ->
/// `.../api/graphql`, else `<base>/graphql`).
pub fn endpoint(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    if path == "graphql" {
        if let Some(root) = base.strip_suffix("/api/v3") {
            return format!("{root}/api/graphql");
        }
        return format!("{base}/graphql");
    }
    format!("{base}/{}", path.trim_start_matches('/'))
}

// ---- git credential store -----------------------------------------------------------------

struct Cached {
    token: Option<String>,
    expires: Instant,
}

/// Desk `credentialToken` / `forgetToken`: tokens from git's credential store per host,
/// cached 10 min (no token: 1 min). Concurrent callers for one host wait for the lookup in
/// flight instead of starting another.
#[derive(Default)]
pub struct Credentials {
    hosts: Mutex<HashMap<String, Arc<Mutex<Option<Cached>>>>>,
    /// Extra environment of `git credential fill` (tests isolate git's config with it).
    env: Vec<(String, String)>,
}

impl Credentials {
    /// A cache whose lookups run git with `env` added.
    pub fn new(env: Vec<(String, String)>) -> Self {
        Credentials {
            hosts: Mutex::default(),
            env,
        }
    }

    /// The password git would use for `https://<host>` (a token for GitHub), or `None`.
    /// Never prompts.
    pub fn token(&self, host: &str) -> Option<String> {
        let slot = self
            .hosts
            .lock()
            .unwrap()
            .entry(host.to_lowercase())
            .or_default()
            .clone();
        let mut entry = slot.lock().unwrap();
        if let Some(c) = entry.as_ref()
            && c.expires > Instant::now()
        {
            return c.token.clone();
        }
        let token = fill_credential(host, &self.env);
        let ttl = if token.is_some() {
            TOKEN_TTL
        } else {
            NO_TOKEN_TTL
        };
        *entry = Some(Cached {
            token: token.clone(),
            expires: Instant::now() + ttl,
        });
        token
    }

    /// The API refused `token` (401): the next call asks git's credential store again.
    pub fn forget(&self, token: &str) {
        let slots: Vec<_> = self.hosts.lock().unwrap().values().cloned().collect();
        for slot in slots {
            let mut entry = slot.lock().unwrap();
            if entry.as_ref().and_then(|c| c.token.as_deref()) == Some(token) {
                *entry = None;
            }
        }
    }

    /// What the cache holds for `host` (`None`: nothing, `Some(None)`: "no token").
    #[cfg(test)]
    pub fn cached(&self, host: &str) -> Option<Option<String>> {
        let slot = self
            .hosts
            .lock()
            .unwrap()
            .get(&host.to_lowercase())?
            .clone();
        let entry = slot.lock().unwrap();
        entry.as_ref().map(|c| c.token.clone())
    }
}

/// Desk `fillCredential`: `git credential fill` with `protocol=https`, `host=<host>` on stdin,
/// no prompts (`GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=never`), no console window, 20 s;
/// the `password=` line, `None` on any failure.
fn fill_credential(host: &str, env: &[(String, String)]) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.args(["credential", "fill"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let input = format!("protocol=https\nhost={host}\n\n").into_bytes();
    let out = run_timeout(cmd, Some(input), CREDENTIAL_TIMEOUT).ok()??;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("password="))
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
}

// ---- HTTPS --------------------------------------------------------------------------------

/// The REST / GraphQL API over HTTPS (Desk's `fetch` with `githubHeaders`).
pub struct HttpApi {
    agent: ureq::Agent,
    /// API root for every host instead of [`api_base`] (tests).
    base: Option<String>,
}

impl HttpApi {
    /// An API client; `base` replaces the per-host API root (tests only).
    pub fn new(base: Option<String>) -> Self {
        let mut config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(FETCH_TIMEOUT));
        if base.is_some() {
            // A local fake GitHub: never through a proxy from the environment.
            config = config.proxy(None);
        }
        HttpApi {
            agent: config.build().new_agent(),
            base,
        }
    }

    /// One call with `token`. `Err` (`github-failed`) only when no HTTP answer came.
    pub fn call(&self, token: &str, req: &GhRequest<'_>) -> Result<GhResponse, ProtoError> {
        let base = self.base.clone().unwrap_or_else(|| api_base(req.host));
        let mut url = endpoint(&base, req.path);
        if !req.query.is_empty() {
            let q: Vec<String> = req
                .query
                .iter()
                .map(|(k, v)| format!("{}={}", form_encode(k), form_encode(v)))
                .collect();
            url.push('?');
            url.push_str(&q.join("&"));
        }
        let failed = |detail: String| ProtoError::new("github-failed").arg("detail", detail);
        let builder = ureq::http::Request::builder()
            .method(req.method)
            .uri(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "ide-server");
        let sent = match req.body {
            Some(body) => builder
                .header("Content-Type", "application/json")
                .body(body.to_string().into_bytes())
                .map_err(|e| failed(e.to_string()))
                .and_then(|r| self.agent.run(r).map_err(|e| failed(e.to_string()))),
            None => builder
                .body(())
                .map_err(|e| failed(e.to_string()))
                .and_then(|r| self.agent.run(r).map_err(|e| failed(e.to_string()))),
        };
        let mut res = sent?;
        let status = res.status().as_u16();
        // Desk `res.json().catch(() => null)`: an unreadable body is no body.
        let text = res
            .body_mut()
            .with_config()
            .limit(BODY_LIMIT)
            .read_to_string()
            .unwrap_or_default();
        let text = text.trim();
        let body = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()))
        };
        Ok(GhResponse { status, body })
    }
}

// ---- the transport ------------------------------------------------------------------------

/// Credential token + HTTPS first, `gh` as the fallback (see the module docs).
pub struct GithubTransport {
    /// Token cache (one per user context).
    pub credentials: Arc<Credentials>,
    /// The HTTPS client.
    pub http: Arc<HttpApi>,
    /// The fallback: `gh`, or why not (`gh-missing`).
    pub gh: Result<Arc<dyn GhApi>, ProtoError>,
}

impl GhApi for GithubTransport {
    fn call(&self, req: &GhRequest<'_>) -> Result<GhResponse, ProtoError> {
        let Some(token) = self.credentials.token(req.host) else {
            return match &self.gh {
                Ok(gh) => gh.call(req),
                Err(e) if e.id == "gh-missing" => {
                    Err(ProtoError::new("github-no-credentials").arg("host", req.host))
                }
                Err(e) => Err(e.clone()),
            };
        };
        let res = self.http.call(&token, req)?;
        if res.status != 401 {
            return Ok(res);
        }
        self.credentials.forget(&token);
        match &self.gh {
            Ok(gh) => {
                tracing::info!(
                    "github: {} refused git's credential token (401), retrying through gh",
                    req.host
                );
                gh.call(req)
            }
            Err(_) => Err(ProtoError::new("github-auth").arg("host", req.host)),
        }
    }
}
