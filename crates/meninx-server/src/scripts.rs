//! `scripts.list`: runnable scripts of a worktree (port of Hermes Desk's `main/scripts.ts`), VS
//! Code "NPM Scripts" style for every common ecosystem: files that name their tasks
//! (package.json, composer.json, deno.json, Makefile, justfile, Taskfile, pyproject task
//! runners, Rakefile) and build tools with well-known commands (Cargo, Go, Gradle, Maven, .NET,
//! Docker Compose). One group per file; its commands run in the file's folder.

use std::collections::HashSet;
use std::path::Path;

use meninx_proto::{ScriptGroup, ScriptItem};
use ignore::WalkBuilder;

/// Files of a worktree that are looked at (not ignored), at most this many.
const MAX_FILES: usize = 200_000;
/// Script files bigger than this are skipped.
const MAX_SIZE: u64 = 1024 * 1024;

struct Ctx<'a> {
    folder: &'a str,
    name: &'a str,
    present: &'a HashSet<String>,
}

fn item(name: &str, run: String, detail: Option<&str>) -> ScriptItem {
    ScriptItem {
        name: name.to_owned(),
        run,
        detail: detail.map(str::trim).filter(|d| !d.is_empty()).map(str::to_owned),
    }
}

/// The scripts of the worktree at `root`: shallow files first (the root before sub-packages),
/// then by path.
pub fn discover(root: &Path) -> Vec<ScriptGroup> {
    let mut files: Vec<String> = Vec::new();
    let mut w = WalkBuilder::new(root);
    w.hidden(false)
        .parents(true)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git");
    for ent in w.build().flatten() {
        if !ent.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if let Ok(rel) = ent.path().strip_prefix(root) {
            let parts: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            files.push(parts.join("/"));
        }
        if files.len() >= MAX_FILES {
            break;
        }
    }
    let present: HashSet<String> = files.iter().cloned().collect();
    let mut groups = Vec::new();
    for rel in &files {
        let (folder, name) = match rel.rsplit_once('/') {
            Some((f, n)) => (f, n),
            None => ("", rel.as_str()),
        };
        let Some((kind, needs_text)) = detect(name) else {
            continue;
        };
        let text = if needs_text {
            let p = root.join(rel);
            if std::fs::metadata(&p).map_or(true, |m| m.len() > MAX_SIZE) {
                continue;
            }
            match std::fs::read_to_string(&p) {
                Ok(t) => t,
                Err(_) => continue,
            }
        } else {
            String::new()
        };
        let ctx = Ctx {
            folder,
            name,
            present: &present,
        };
        let scripts = parse(kind, &text, &ctx);
        if !scripts.is_empty() {
            groups.push(ScriptGroup {
                file: rel.clone(),
                folder: folder.to_owned(),
                kind: kind.to_owned(),
                scripts,
            });
        }
    }
    groups.sort_by(|a, b| {
        let depth = |p: &str| p.split('/').count();
        depth(&a.file)
            .cmp(&depth(&b.file))
            .then_with(|| a.file.cmp(&b.file))
    });
    groups
}

/// The kind of a script file and whether its text is needed.
fn detect(name: &str) -> Option<(&'static str, bool)> {
    let lower = name.to_ascii_lowercase();
    Some(match lower.as_str() {
        "package.json" => ("npm", true),
        "composer.json" => ("composer", true),
        "deno.json" | "deno.jsonc" => ("deno", true),
        "makefile" | "gnumakefile" => ("make", true),
        "justfile" | ".justfile" => ("just", true),
        "taskfile.yml" | "taskfile.yaml" => ("task", true),
        "rakefile" => ("rake", true),
        "pyproject.toml" => ("python", true),
        _ if name == "Cargo.toml" => ("cargo", false),
        "go.mod" => ("go", false),
        "build.gradle" | "build.gradle.kts" => ("gradle", false),
        "pom.xml" => ("maven", false),
        "docker-compose.yml" | "docker-compose.yaml" | "compose.yml" | "compose.yaml" => {
            ("compose", false)
        }
        _ if [".csproj", ".fsproj", ".vbproj", ".sln"]
            .iter()
            .any(|e| lower.ends_with(e)) =>
        {
            ("dotnet", true)
        }
        _ => return None,
    })
}

fn parse(kind: &str, text: &str, ctx: &Ctx) -> Vec<ScriptItem> {
    let fixed = |list: &[(&str, &str)]| -> Vec<ScriptItem> {
        list.iter().map(|(n, r)| item(n, (*r).to_owned(), None)).collect()
    };
    match kind {
        "npm" => package_json(text, ctx),
        "composer" => composer_json(text),
        "deno" => deno_json(text),
        "make" => makefile(text),
        "just" => justfile(text),
        "task" => taskfile(text),
        "rake" => rakefile(text),
        "python" => pyproject(text),
        "cargo" => fixed(&[
            ("build", "cargo build"),
            ("run", "cargo run"),
            ("test", "cargo test"),
            ("check", "cargo check"),
            ("clippy", "cargo clippy"),
        ]),
        "go" => fixed(&[
            ("build", "go build ./..."),
            ("run", "go run ."),
            ("test", "go test ./..."),
            ("vet", "go vet ./..."),
        ]),
        "gradle" => gradle(ctx),
        "maven" => fixed(&[
            ("package", "mvn package"),
            ("test", "mvn test"),
            ("install", "mvn clean install"),
            ("spring-boot:run", "mvn spring-boot:run"),
        ]),
        "compose" => {
            let f = format!("-f \"{}\"", ctx.name);
            vec![
                item("up", format!("docker compose {f} up"), None),
                item("up (build)", format!("docker compose {f} up --build"), None),
                item("down", format!("docker compose {f} down"), None),
                item("logs", format!("docker compose {f} logs -f"), None),
            ]
        }
        "dotnet" => dotnet(text, ctx),
        _ => Vec::new(),
    }
}

/// A JSON object's entries in file order (`serde_json`'s map sorts its keys; Desk lists scripts
/// as written).
struct Ordered(Vec<(String, serde_json::Value)>);

impl<'de> serde::Deserialize<'de> for Ordered {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Ordered;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut m: A) -> Result<Ordered, A::Error> {
                let mut out = Vec::new();
                while let Some((k, v)) = m.next_entry::<String, serde_json::Value>()? {
                    out.push((k, v));
                }
                Ok(Ordered(out))
            }
        }
        d.deserialize_map(V)
    }
}

/// The entries of the top-level object `key` of a JSON text, in file order.
fn json_section(text: &str, key: &str) -> Vec<(String, serde_json::Value)> {
    struct Section<'k>(&'k str);
    impl<'de> serde::de::DeserializeSeed<'de> for Section<'_> {
        type Value = Vec<(String, serde_json::Value)>;
        fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_map(self)
        }
    }
    impl<'de> serde::de::Visitor<'de> for Section<'_> {
        type Value = Vec<(String, serde_json::Value)>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an object")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
            let mut found = Vec::new();
            while let Some(k) = m.next_key::<String>()? {
                if k == self.0 {
                    found = m.next_value::<Ordered>()?.0;
                } else {
                    m.next_value::<serde::de::IgnoredAny>()?;
                }
            }
            Ok(found)
        }
    }
    let mut de = serde_json::Deserializer::from_str(text);
    serde::de::DeserializeSeed::deserialize(Section(key), &mut de).unwrap_or_default()
}

// ---- package.json (npm / yarn / pnpm / bun)

/// The nearest lock file up to the root decides (monorepos keep one at the root).
fn package_manager(ctx: &Ctx) -> &'static str {
    let mut folder = ctx.folder.to_owned();
    loop {
        let at = |f: &str| {
            ctx.present.contains(&if folder.is_empty() {
                f.to_owned()
            } else {
                format!("{folder}/{f}")
            })
        };
        if at("pnpm-lock.yaml") {
            return "pnpm";
        }
        if at("yarn.lock") {
            return "yarn";
        }
        if at("bun.lockb") || at("bun.lock") {
            return "bun";
        }
        if at("package-lock.json") || folder.is_empty() {
            return "npm";
        }
        folder = folder.rsplit_once('/').map_or(String::new(), |(p, _)| p.to_owned());
    }
}

fn package_json(text: &str, ctx: &Ctx) -> Vec<ScriptItem> {
    let scripts = json_section(text, "scripts");
    let pm = package_manager(ctx);
    scripts
        .iter()
        // Keys used as comments / separators ("//", "=== DEV ===") are not scripts.
        .filter(|(name, _)| {
            !(name.chars().any(char::is_whitespace)
                || name.starts_with("//")
                || name.starts_with('#')
                || name.starts_with('=')
                || name.starts_with("--"))
        })
        .map(|(name, cmd)| {
            let run = if pm == "yarn" {
                format!("yarn {name}")
            } else {
                format!("{pm} run {name}")
            };
            item(name, run, cmd.as_str())
        })
        .collect()
}

// ---- composer.json / deno.json

fn composer_json(text: &str) -> Vec<ScriptItem> {
    json_section(text, "scripts")
        .iter()
        .map(|(name, cmd)| {
            let detail = match cmd {
                serde_json::Value::Array(a) => a
                    .iter()
                    .map(|x| x.as_str().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join(" && "),
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            item(name, format!("composer run-script {name}"), Some(&detail))
        })
        .collect()
}

fn deno_json(text: &str) -> Vec<ScriptItem> {
    json_section(&strip_json_comments(text), "tasks")
        .iter()
        .map(|(name, cmd)| {
            let detail = cmd
                .as_str()
                .map(str::to_owned)
                .or_else(|| cmd.get("command").and_then(|c| c.as_str()).map(str::to_owned));
            item(name, format!("deno task {name}"), detail.as_deref())
        })
        .collect()
}

/// `//` and `/* */` comments out of JSON (not inside strings).
fn strip_json_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_str = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

// ---- Makefile / justfile / Taskfile / Rakefile

fn is_target_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-')
}

fn makefile(text: &str) -> Vec<ScriptItem> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for line in text.lines() {
        // "target: deps" at column 0; not variables (:=, ::=), not special (.PHONY) or patterns.
        let Some(colon) = line.find(':') else {
            continue;
        };
        let (targets, rest) = (&line[..colon], &line[colon + 1..]);
        if rest.starts_with([':', '=']) || !line.starts_with(|c: char| c.is_ascii_alphanumeric()) {
            continue;
        }
        let targets = targets.trim_end();
        if targets.is_empty()
            || !targets
                .split_whitespace()
                .all(|t| t.chars().all(is_target_char) && t.starts_with(|c: char| c.is_ascii_alphanumeric()))
        {
            continue;
        }
        let deps = rest.split('#').next().unwrap_or_default();
        for t in targets.split_whitespace() {
            if t.contains('%') || !seen.insert(t.to_owned()) {
                continue;
            }
            out.push(item(t, format!("make {t}"), Some(deps)));
        }
    }
    out
}

fn justfile(text: &str) -> Vec<ScriptItem> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let first = line.split_whitespace().next().unwrap_or_default();
        if ["set", "alias", "export", "import", "mod"].contains(&first) {
            continue;
        }
        let body = line.strip_prefix('@').unwrap_or(line);
        let name: String = body
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            .collect();
        if name.is_empty() || name.starts_with('_') || name.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let rest = &body[name.len()..];
        // `name args: deps` (not `name := value`).
        let Some(colon) = rest.find(':') else {
            continue;
        };
        if rest[..colon].contains('=') || rest[colon + 1..].starts_with('=') {
            continue;
        }
        let doc = i
            .checked_sub(1)
            .and_then(|p| lines[p].trim_start().strip_prefix('#'))
            .map(str::trim);
        out.push(item(&name, format!("just {name}"), doc));
    }
    out
}

fn taskfile(text: &str) -> Vec<ScriptItem> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut in_tasks = false;
    let mut indent: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if line.trim_end() == "tasks:" {
            in_tasks = true;
            continue;
        }
        let t = line.trim();
        if !in_tasks || t.is_empty() || t.starts_with('#') {
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            break; // next top-level key
        }
        let lead = line.len() - line.trim_start().len();
        let Some(name) = t.strip_suffix(':').or_else(|| t.split_once(':').map(|(n, _)| n)) else {
            continue;
        };
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '-')) {
            continue;
        }
        let first = *indent.get_or_insert(lead);
        if lead != first {
            continue;
        }
        let desc = lines.get(i + 1).and_then(|n| {
            n.trim()
                .strip_prefix("desc:")
                .map(|d| d.trim().trim_matches(['"', '\'']).to_owned())
        });
        out.push(item(name, format!("task {name}"), desc.as_deref()));
    }
    out
}

fn rakefile(text: &str) -> Vec<ScriptItem> {
    let mut out: Vec<ScriptItem> = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("task") else {
            continue;
        };
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let rest = rest.trim_start();
        let word = |s: &str| -> String {
            s.chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect()
        };
        let name = if let Some(s) = rest.strip_prefix(':') {
            word(s)
        } else if let Some(s) = rest.strip_prefix(['"', '\'']) {
            word(s)
        } else {
            let w = word(rest);
            if rest[w.len()..].starts_with(':') { w } else { String::new() }
        };
        if !name.is_empty() && !out.iter().any(|s| s.name == name) {
            out.push(item(&name, format!("rake {name}"), None));
        }
    }
    out
}

// ---- pyproject.toml (poe / pdm / hatch task runners)

fn pyproject(text: &str) -> Vec<ScriptItem> {
    let mut out = Vec::new();
    let mut table = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(h) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            table = h.chars().filter(|c| !c.is_whitespace()).collect();
            // [tool.poe.tasks.name] declares a task by itself.
            if let Some(sub) = table.strip_prefix("tool.poe.tasks.") {
                let sub = sub.trim_matches('"');
                if !sub.is_empty() && !sub.contains('.') {
                    out.push(item(sub, format!("poe {sub}"), None));
                }
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let name = key.trim().trim_matches('"');
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')) {
            continue;
        }
        let detail = value.trim().trim_matches(['"', '\'']);
        if table == "tool.poe.tasks" {
            out.push(item(name, format!("poe {name}"), Some(detail)));
        } else if table == "tool.pdm.scripts" && !name.starts_with('_') {
            out.push(item(name, format!("pdm run {name}"), Some(detail)));
        } else if let Some(env) = table
            .strip_prefix("tool.hatch.envs.")
            .and_then(|t| t.strip_suffix(".scripts"))
        {
            let (label, run) = if env == "default" {
                (name.to_owned(), format!("hatch run {name}"))
            } else {
                (format!("{env}:{name}"), format!("hatch run {env}:{name}"))
            };
            out.push(item(&label, run, Some(detail)));
        }
    }
    out
}

// ---- build tools with well-known commands

fn gradle(ctx: &Ctx) -> Vec<ScriptItem> {
    let wrapper = ctx.present.contains(&if ctx.folder.is_empty() {
        "gradlew".to_owned()
    } else {
        format!("{}/gradlew", ctx.folder)
    });
    let g = match (wrapper, cfg!(windows)) {
        (true, true) => "gradlew.bat",
        (true, false) => "./gradlew",
        (false, _) => "gradle",
    };
    ["build", "test", "run", "clean", "tasks"]
        .iter()
        .map(|t| item(t, format!("{g} {t}"), None))
        .collect()
}

fn dotnet(text: &str, ctx: &Ctx) -> Vec<ScriptItem> {
    let p = ctx.name;
    if p.to_ascii_lowercase().ends_with(".sln") {
        return vec![
            item("build", format!("dotnet build \"{p}\""), None),
            item("test", format!("dotnet test \"{p}\""), None),
        ];
    }
    // Only what the project can do: run for apps / web / workers, test for test projects.
    let lower = text.to_ascii_lowercase();
    let squeezed: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
    let runnable = squeezed.contains("<outputtype>exe<")
        || squeezed.contains("<outputtype>winexe<")
        || lower.contains("sdk=\"microsoft.net.sdk.web")
        || lower.contains("sdk=\"microsoft.net.sdk.worker");
    let testable = lower.contains("microsoft.net.test.sdk") || squeezed.contains("<istestproject>true");
    let mut out = vec![item("build", format!("dotnet build \"{p}\""), None)];
    if testable {
        out.push(item("test", format!("dotnet test \"{p}\""), None));
    }
    if runnable {
        out.push(item("run", format!("dotnet run --project \"{p}\""), None));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(present: &'a HashSet<String>, folder: &'a str, name: &'a str) -> Ctx<'a> {
        Ctx {
            folder,
            name,
            present,
        }
    }

    fn names(v: &[ScriptItem]) -> Vec<&str> {
        v.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn package_json_uses_the_nearest_lock_file() {
        let mut present = HashSet::new();
        present.insert("yarn.lock".to_owned());
        let text = r#"{"scripts":{"dev":"vite","// note":"x","build":"vite build"}}"#;
        let s = package_json(text, &ctx(&present, "web", "package.json"));
        assert_eq!(names(&s), ["dev", "build"]);
        assert_eq!(s[0].run, "yarn dev");
        assert_eq!(s[0].detail.as_deref(), Some("vite"));
        present.insert("web/pnpm-lock.yaml".to_owned());
        assert_eq!(package_json(text, &ctx(&present, "web", "package.json"))[1].run, "pnpm run build");
    }

    #[test]
    fn makefile_targets() {
        let s = makefile("CC := gcc\n.PHONY: all\nall build: dep # x\n%.o: %.c\n\tcc\ntest:\n");
        assert_eq!(names(&s), ["all", "build", "test"]);
        assert_eq!(s[1].run, "make build");
        assert_eq!(s[0].detail.as_deref(), Some("dep"));
    }

    #[test]
    fn justfile_taskfile_rakefile_pyproject() {
        let j = justfile("set shell := [\"bash\"]\n# Build it\nbuild target:\n  cc\n_hidden:\nx := 1\n");
        assert_eq!(names(&j), ["build"]);
        assert_eq!(j[0].detail.as_deref(), Some("Build it"));
        let t = taskfile("version: '3'\ntasks:\n  build:\n    desc: Build\n    cmds: [x]\n  test:\n    cmds: [y]\nvars:\n");
        assert_eq!(names(&t), ["build", "test"]);
        assert_eq!(t[0].detail.as_deref(), Some("Build"));
        let r = rakefile("task :build do\nend\ntask \"lint\" do\nend\ntask test: :build\n");
        assert_eq!(names(&r), ["build", "lint", "test"]);
        let p = pyproject("[tool.poe.tasks]\nlint = \"ruff .\"\n[tool.pdm.scripts]\nstart = \"python -m app\"\n[tool.hatch.envs.dev.scripts]\ncheck = \"mypy\"\n");
        assert_eq!(names(&p), ["lint", "start", "dev:check"]);
        assert_eq!(p[2].run, "hatch run dev:check");
    }

    #[test]
    fn deno_comments_and_dotnet() {
        let d = deno_json("{\n // c\n \"tasks\": { \"dev\": \"deno run a.ts\" /* x */ }\n}");
        assert_eq!(d[0].run, "deno task dev");
        let present = HashSet::new();
        let x = dotnet("<Project Sdk=\"Microsoft.NET.Sdk.Web\"></Project>", &ctx(&present, "", "a.csproj"));
        assert_eq!(names(&x), ["build", "run"]);
    }
}
