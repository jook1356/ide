//! TypeScript pack additions (port of Hermes Desk `langpacks/typescript/*`): type-level
//! declarations and tsconfig `paths` / `baseUrl` resolution.

use super::Ctx;
use super::js;
use super::util::*;

const PREFIX: &str = r"\s*(?:export\s+)?(?:default\s+)?(?:declare\s+)?";

/// Compiled TS declaration matcher (type-level patterns, then the JS ones).
pub struct TsDecls {
    ts: Compiled,
    js: js::JsDecls,
}

impl TsDecls {
    /// Compile for `name`.
    pub fn new(name: &str) -> Self {
        static PATTERNS: std::sync::LazyLock<Vec<(String, &'static str)>> =
            std::sync::LazyLock::new(|| {
                vec![
                    (
                        format!(r"{PREFIX}(?:abstract\s+)?(?:class|interface)\s+"),
                        "",
                    ),
                    (format!(r"{PREFIX}(?:const\s+)?enum\s+"), ""),
                    (format!(r"{PREFIX}type\s+"), r"\s*(?:<|=)"),
                    (format!(r"{PREFIX}(?:namespace|module)\s+"), ""),
                    (format!(r"{PREFIX}(?:async\s+)?function\s*\*?\s*"), ""),
                ]
            });
        let pats: Vec<DeclPattern<'_>> =
            PATTERNS.iter().map(|(p, s)| pat(p, s, Kind::Top)).collect();
        TsDecls {
            ts: Compiled::new(&pats, name),
            js: js::JsDecls::new(name),
        }
    }

    /// The column of the name when `line` declares it.
    pub fn column(&self, line: &str) -> Option<(u32, Kind)> {
        let t = line.trim_start();
        if t.starts_with("//") || t.starts_with('*') || t.starts_with("/*") {
            return None;
        }
        self.ts.first(line).or_else(|| self.js.column(line))
    }
}

/// The parts of a tsconfig that matter for resolution (repo-relative directories).
#[derive(Debug, Clone, Default)]
pub struct TsPaths {
    /// Repo-relative baseUrl directory (`""` = repo root), if set.
    pub base_url: Option<String>,
    /// Pattern -> repo-relative target patterns.
    pub paths: Vec<(String, Vec<String>)>,
}

fn read_config(path: &str, ctx: &dyn Ctx) -> Option<serde_json::Value> {
    let text = ctx.read(path)?;
    parse_jsonc(&text).filter(|v| v.is_object())
}

/// A config's baseUrl / paths, following relative `extends` (depth-limited).
fn config_paths(path: &str, ctx: &dyn Ctx, depth: u32) -> Option<TsPaths> {
    let raw = read_config(path, ctx)?;
    let dir = dirname(path).to_owned();
    let mut inherited: Option<TsPaths> = None;
    let ext: Vec<String> = match raw.get("extends") {
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    };
    for e in ext {
        if depth >= 3 || !e.starts_with('.') {
            continue;
        }
        let file = if e.ends_with(".json") {
            e.clone()
        } else {
            format!("{e}.json")
        };
        let got = join_path(&[&dir, &file]).and_then(|t| config_paths(&t, ctx, depth + 1));
        if let Some(got) = got {
            let prev = inherited.take().unwrap_or_default();
            inherited = Some(TsPaths {
                base_url: got.base_url.or(prev.base_url),
                paths: if got.paths.is_empty() {
                    prev.paths
                } else {
                    got.paths
                },
            });
        }
    }
    let opts = raw.get("compilerOptions");
    let base_url = match opts.and_then(|o| o.get("baseUrl")).and_then(|v| v.as_str()) {
        Some(b) => join_path(&[&dir, b]),
        None => inherited.as_ref().and_then(|i| i.base_url.clone()),
    };
    let mut paths = inherited.map(|i| i.paths).unwrap_or_default();
    if let Some(serde_json::Value::Object(map)) = opts.and_then(|o| o.get("paths")) {
        let from = base_url.clone().unwrap_or_else(|| dir.clone());
        paths = map
            .iter()
            .filter_map(|(pattern, targets)| {
                let arr = targets.as_array()?;
                Some((
                    pattern.clone(),
                    arr.iter()
                        .filter_map(|t| t.as_str())
                        .filter_map(|t| join_path(&[&from, t]))
                        .collect(),
                ))
            })
            .collect();
    }
    Some(TsPaths { base_url, paths })
}

/// baseUrl / paths of the nearest tsconfig.json above `from`; a solution-style config with
/// only `references` merges its references' settings.
pub fn nearest_ts_paths(from: &str, ctx: &dyn Ctx) -> Option<TsPaths> {
    let mut dir = dirname(from).to_owned();
    for _ in 0..40 {
        let path = if dir.is_empty() {
            "tsconfig.json".to_owned()
        } else {
            format!("{dir}/tsconfig.json")
        };
        if ctx.exists(&path) {
            let own = config_paths(&path, ctx, 0)?;
            if !own.paths.is_empty() || own.base_url.is_some() {
                return Some(own);
            }
            let raw = read_config(&path, ctx);
            let mut merged = TsPaths::default();
            let refs = raw
                .as_ref()
                .and_then(|r| r.get("references"))
                .and_then(|r| r.as_array())
                .cloned()
                .unwrap_or_default();
            for r in refs {
                let Some(p) = r.get("path").and_then(|p| p.as_str()) else {
                    continue;
                };
                let Some(mut target) = join_path(&[&dir, p]) else {
                    continue;
                };
                if !target.ends_with(".json") {
                    target = format!("{target}/tsconfig.json");
                }
                let Some(got) = config_paths(&target, ctx, 0) else {
                    continue;
                };
                if merged.base_url.is_none() {
                    merged.base_url = got.base_url;
                }
                merged.paths.extend(got.paths);
            }
            return Some(merged);
        }
        if dir.is_empty() {
            return None;
        }
        dir = dirname(&dir).to_owned();
    }
    None
}

/// Repo-relative base paths a bare specifier maps to through paths / baseUrl.
pub fn alias_bases(spec: &str, cfg: &TsPaths) -> Vec<String> {
    let mut out = Vec::new();
    for (pattern, targets) in &cfg.paths {
        let rest = match pattern.find('*') {
            None => (pattern == spec).then(String::new),
            Some(star) => {
                let (pre, post) = (&pattern[..star], &pattern[star + 1..]);
                (spec.starts_with(pre)
                    && spec.ends_with(post)
                    && spec.len() >= pre.len() + post.len())
                .then(|| spec[pre.len()..spec.len() - post.len()].to_owned())
            }
        };
        let Some(rest) = rest else { continue };
        for t in targets {
            out.push(t.replacen('*', &rest, 1));
        }
    }
    if let Some(b) = &cfg.base_url
        && let Some(p) = join_path(&[b, spec])
    {
        out.push(p);
    }
    out
}

/// Relative like JS (TS extensions first), then tsconfig aliases.
pub fn resolve(spec: &str, from: &str, ctx: &dyn Ctx) -> Option<String> {
    if let Some(r) = js::resolve(spec, from, ctx, false) {
        return Some(r);
    }
    if spec.starts_with('.') {
        return None;
    }
    let cfg = nearest_ts_paths(from, ctx)?;
    let exts = js::source_extensions();
    alias_bases(spec, &cfg)
        .iter()
        .find_map(|base| js::first_existing(&js::file_candidates(base, &exts), ctx))
}

#[cfg(test)]
mod tests {
    use super::super::{MemCtx, Pack, find_declarations};

    #[test]
    fn tsconfig_resolution() {
        let tsconfig = r#"{
    // comment with "quotes"
    "compilerOptions": {
      /* block */ "baseUrl": ".",
      "paths": { "@/*": ["src/*"], "@lib": ["src/lib/index.ts"], },
    },
  }"#;
        let ctx = MemCtx::new(&[
            ("tsconfig.json", tsconfig),
            ("src/util/x.ts", ""),
            ("src/lib/index.ts", ""),
            ("src/app/main.ts", ""),
            ("src/plain.tsx", ""),
        ]);
        let p = Pack::Ts;
        let r = |s: &str| p.resolve(s, "src/app/main.ts", &ctx);
        assert_eq!(r("@/util/x").as_deref(), Some("src/util/x.ts"));
        assert_eq!(r("@lib").as_deref(), Some("src/lib/index.ts"));
        assert_eq!(r("src/plain").as_deref(), Some("src/plain.tsx"));
        assert_eq!(r("../util/x").as_deref(), Some("src/util/x.ts"));
        assert_eq!(r("nope"), None);
        let solution = MemCtx::new(&[
            (
                "tsconfig.json",
                r#"{ "files": [], "references": [{ "path": "./tsconfig.web.json" }] }"#,
            ),
            (
                "tsconfig.web.json",
                r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@renderer/*": ["src/renderer/src/*"] } } }"#,
            ),
            ("src/renderer/src/lib/a.ts", ""),
        ]);
        assert_eq!(
            p.resolve("@renderer/lib/a", "src/renderer/src/App.tsx", &solution)
                .as_deref(),
            Some("src/renderer/src/lib/a.ts")
        );
    }

    #[test]
    fn ts_declarations() {
        let src = [
            "export interface Props {",
            "type Alias<T> = T[]",
            "export const enum Color { Red }",
            "declare namespace NS {}",
            "export declare function ambient(): void",
            "export abstract class Base {}",
            "  async load(id: string): Promise<void> {",
            "const value: number = 1",
            "type",
            "let typed = 1",
        ]
        .join("\n");
        let lines = |p: Pack, n: &str| -> Vec<u32> {
            find_declarations(p, &src, n)
                .iter()
                .map(|d| d.line)
                .collect()
        };
        for (n, l) in [
            ("Props", 1),
            ("Alias", 2),
            ("Color", 3),
            ("NS", 4),
            ("ambient", 5),
            ("Base", 6),
            ("load", 7),
            ("value", 8),
        ] {
            assert_eq!(lines(Pack::Ts, n), [l], "{n}");
        }
        assert!(lines(Pack::Js, "Props").is_empty());
    }
}
