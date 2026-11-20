//! `def.lookup` cases ported from Hermes Desk `tests/definitions.test.cjs`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::*;
use crate::jail::testutil::{git, tmp};

const FILES: &[(&str, &str)] = &[
    // JS: require, destructured require, namespace-style module object.
    (
        "js/util.js",
        "function helper() {}\nfunction other() {}\nmodule.exports = { helper, other }\n",
    ),
    (
        "js/main.js",
        "const u = require('./util')\nconst { helper } = require('./util')\nhelper()\nu.other()\n",
    ),
    // TS: relative import, tsconfig paths alias, namespace import, re-export chain.
    (
        "tsconfig.json",
        "{\n  // aliases\n  \"compilerOptions\": { \"baseUrl\": \".\", \"paths\": { \"@/*\": [\"src/*\"], }, },\n}\n",
    ),
    (
        "src/lib/math.ts",
        "export const PI = 3\n\nexport function add(a: number, b: number) {\n  return a + b\n}\n",
    ),
    (
        "src/lib/more.ts",
        "export interface More {\n  x: number\n}\n",
    ),
    (
        "src/lib/index.ts",
        "export { add as sum } from './math'\nexport * from './more'\n",
    ),
    (
        "src/a.ts",
        "import { add } from './lib/math'\nimport * as m from '@/lib/math'\nimport { sum, More } from '@/lib'\n\nfunction local() {}\nlocal()\nadd(1, 2)\nm.PI\nsum(1, 2)\nlet v: More\n",
    ),
    // Vue SFCs.
    (
        "src/components/MyButton.vue",
        "<template>\n  <button @click=\"onClick\"><slot /></button>\n</template>\n\n<script setup lang=\"ts\">\nimport { add } from '@/lib/math'\nconst emit = defineEmits<{ click: [] }>()\nfunction onClick(n: number) {\n  emit('click')\n  return add(n, 1)\n}\n</script>\n",
    ),
    (
        "src/components/Card.vue",
        "<script>\nexport default {\n  name: 'Card'\n}\n</script>\n",
    ),
    (
        "src/App.vue",
        "<template>\n  <my-button />\n  <MyButton>{{ title }}</MyButton>\n  <Card />\n</template>\n\n<script setup>\nimport MyButton from './components/MyButton.vue'\nimport Card from '@/components/Card'\nconst title = 'hi'\n</script>\n",
    ),
    (
        "src/use.ts",
        "import MyButton from './components/MyButton.vue'\nMyButton\n",
    ),
    // Python.
    ("pkg/__init__.py", "from .mod import x\n"),
    (
        "pkg/mod.py",
        "import os\n\n\ndef x():\n    return 1\n\nLIMIT: int = 3\n",
    ),
    (
        "pkg/use.py",
        "from .mod import x\nimport pkg.mod as pm\n\nx()\npm.LIMIT\n",
    ),
    ("tools/run.py", "from pkg import x\nx()\n"),
    // Repo-wide: several declarations of the same unimported name.
    ("dup/one.js", "function dup() {}\n"),
    ("dup/two.js", "export function dup() {}\n"),
    ("dup/use.js", "dup()\n"),
    ("aaa/dup.js", "const dup = 1\n"),
    // External module: the require binding is the definition.
    (
        "ext/use.js",
        "const path = require('node:path')\nimport {\n  join\n} from 'node:path'\nconst {\n  sep\n} = require('node:path')\npath.join()\njoin()\nsep\n",
    ),
    ("ext/other.js", "function join() {}\n"),
];

fn file(rel: &str) -> &'static str {
    FILES.iter().find(|(p, _)| *p == rel).unwrap().1
}

/// The fixture repo (pkg/, tools/, dup/, ext/ stay untracked: `--untracked` must see them).
fn repo() -> &'static Path {
    static R: OnceLock<PathBuf> = OnceLock::new();
    R.get_or_init(|| {
        let d = tmp("defs").join("repo");
        std::fs::create_dir_all(&d).unwrap();
        for (rel, text) in FILES {
            let p = d.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        git(&d, &["init", "-q"]);
        git(&d, &["config", "user.email", "t@t"]);
        git(&d, &["config", "user.name", "t"]);
        git(&d, &["config", "commit.gpgsign", "false"]);
        git(&d, &["add", "js", "src", "tsconfig.json"]);
        git(&d, &["commit", "-qm", "base"]);
        d
    })
}

fn find(rel: &str, text: &str, offset: usize) -> Outcome {
    let jail = RootJail::new(repo()).unwrap();
    let ctx = DiskCtx::new(&jail);
    find_definition(&ctx, Some(jail.root()), rel, text, offset)
}

/// Definition at the `nth` occurrence of `needle` (+ `delta` bytes) in a fixture file.
fn at(rel: &str, needle: &str, nth: usize, delta: usize) -> Outcome {
    let text = file(rel);
    let mut i = None;
    for _ in 0..=nth {
        let from = i.map_or(0, |x| x + 1);
        i = Some(from + text[from..].find(needle).expect("needle"));
    }
    find(rel, text, i.unwrap() + delta)
}

fn where_(o: &Outcome) -> String {
    match o {
        Outcome::One(t) => format!("{}:{}:{}", t.path, t.line, t.col),
        other => format!("{other:?}"),
    }
}

fn w(rel: &str, needle: &str, nth: usize, delta: usize) -> String {
    where_(&at(rel, needle, nth, delta))
}

fn reason(o: &Outcome) -> (String, Option<String>) {
    match o {
        Outcome::None(e) => (e.id.clone(), e.get("spec").map(str::to_owned)),
        other => panic!("expected none, got {other:?}"),
    }
}

#[test]
fn js_require_and_destructured_require() {
    assert_eq!(w("js/main.js", "'./util'", 0, 3), "js/util.js:1:1");
    assert_eq!(w("js/main.js", "helper()", 0, 1), "js/util.js:1:10");
    assert_eq!(w("js/main.js", "other", 0, 2), "js/util.js:2:10");
}

#[test]
fn ts_relative_alias_namespace_reexport() {
    assert_eq!(w("src/a.ts", "'./lib/math'", 0, 4), "src/lib/math.ts:1:1");
    assert_eq!(w("src/a.ts", "'@/lib/math'", 0, 4), "src/lib/math.ts:1:1");
    assert_eq!(w("src/a.ts", "'@/lib'", 0, 3), "src/lib/index.ts:1:1");
    assert_eq!(w("src/a.ts", "add(1", 0, 1), "src/lib/math.ts:3:17");
    assert_eq!(w("src/a.ts", "m.PI", 0, 3), "src/lib/math.ts:1:14");
    assert_eq!(w("src/a.ts", "sum(1", 0, 1), "src/lib/math.ts:3:17");
    assert_eq!(w("src/a.ts", "More", 1, 1), "src/lib/more.ts:1:18");
}

#[test]
fn vue_imports_templates_tags_params() {
    let btn = "src/components/MyButton.vue:1:1";
    let app = "src/App.vue";
    assert_eq!(w(app, "'./components/MyButton.vue'", 0, 3), btn);
    assert_eq!(w(app, "my-button", 0, 4), btn);
    assert_eq!(w(app, "MyButton>{{", 0, 2), btn);
    assert_eq!(w(app, "</MyButton", 0, 3), btn);
    assert_eq!(w(app, "title }}", 0, 1), "src/App.vue:10:7");
    assert_eq!(
        w(app, "'@/components/Card'", 0, 3),
        "src/components/Card.vue:1:1"
    );
    assert_eq!(w(app, "<Card", 0, 2), "src/components/Card.vue:2:8");
    let b = "src/components/MyButton.vue";
    assert_eq!(w(b, "onClick\"", 0, 1), format!("{b}:8:10"));
    assert_eq!(w(b, "add(n", 0, 1), "src/lib/math.ts:3:17");
    assert_eq!(w(b, "n, 1", 0, 1), format!("{b}:8:18"));
    assert_eq!(w("src/use.ts", "MyButton", 1, 1), btn);
}

#[test]
fn same_file_local_declaration() {
    let r = at("src/a.ts", "local()", 0, 1);
    assert_eq!(where_(&r), "src/a.ts:5:10");
    let Outcome::One(t) = r else { unreachable!() };
    assert_eq!(t.preview, "function local() {}");
}

#[test]
fn python_relative_package_alias() {
    assert_eq!(w("pkg/use.py", "x()", 0, 1), "pkg/mod.py:4:5");
    assert_eq!(w("pkg/use.py", ".mod", 0, 2), "pkg/mod.py:1:1");
    assert_eq!(w("tools/run.py", "x()", 0, 1), "pkg/mod.py:4:5");
    assert_eq!(w("pkg/use.py", "LIMIT", 0, 2), "pkg/mod.py:7:1");
    let (id, spec) = reason(&at("pkg/mod.py", "os", 0, 1));
    assert_eq!(
        (id.as_str(), spec.as_deref()),
        ("def-external-module", Some("os"))
    );
}

#[test]
fn external_module_binding() {
    assert_eq!(w("ext/use.js", "path.join", 0, 1), "ext/use.js:1:7");
    assert_eq!(w("ext/use.js", "join()", 1, 1), "ext/use.js:3:3");
    assert_eq!(w("ext/use.js", "sep", 1, 1), "ext/use.js:6:3");
    let (id, spec) = reason(&at("ext/use.js", "path.join", 0, 6));
    assert_eq!(
        (id.as_str(), spec.as_deref()),
        ("def-external-name", Some("node:path"))
    );
}

#[test]
fn repo_wide_many_same_dir_first() {
    let r = at("dup/use.js", "dup", 0, 1);
    let Outcome::Many(name, targets) = r else {
        panic!("expected many: {r:?}")
    };
    assert_eq!(name, "dup");
    let got: Vec<String> = targets
        .iter()
        .map(|t| format!("{}:{}:{}", t.path, t.line, t.col))
        .collect();
    assert_eq!(
        got,
        ["dup/one.js:1:10", "dup/two.js:1:17", "aaa/dup.js:1:7"]
    );
    let none = find("src/x.ts", "nothingDeclaresThis()", 2);
    assert_eq!(reason(&none).0, "def-not-found");
}

#[test]
fn unsupported_and_outside_paths() {
    assert_eq!(reason(&find("README.md", "x", 0)).0, "def-unsupported-file");
    assert_eq!(safe_rel("../x.js"), None);
    assert_eq!(safe_rel("a/../../x.js"), None);
    assert_eq!(safe_rel("C:/x.js"), None);
    assert_eq!(safe_rel("/etc/x.js"), None);
    assert_eq!(safe_rel("a\\b.js").as_deref(), Some("a/b.js"));
    assert_eq!(reason(&find("../x.js", "a", 0)).0, "def-invalid-position");
}

fn at_text(rel: &str, text: &str, needle: &str, nth: usize, delta: usize) -> String {
    let mut i = None;
    for _ in 0..=nth {
        let from = i.map_or(0, |x| x + 1);
        i = Some(from + text[from..].find(needle).expect("needle"));
    }
    match find(rel, text, i.unwrap() + delta) {
        Outcome::One(t) => format!("{}:{}", t.line, t.col),
        other => format!("{other:?}"),
    }
}

#[test]
fn js_parameters_win() {
    let text = [
        "const last = 0",
        "export function followChange(",
        "  last: { current: string | undefined },",
        "  path: string",
        "): string | undefined {",
        "  if (last.current === path) return undefined",
        "  const f = ({ a, b: path }) => path + a",
        "  xs.map((x) => last)",
        "  ys.forEach(item => item.n)",
        "  try {} catch (err) { log(err) }",
        "  for (const [k, v] of e) use(v)",
        "  return last.current",
        "}",
        "last",
    ]
    .join("\n");
    let f = |n: &str, nth: usize, d: usize| at_text("p/params.ts", &text, n, nth, d);
    assert_eq!(f("last.current", 0, 0), "3:3");
    assert_eq!(f("last.current", 1, 0), "3:3");
    assert_eq!(f("=== path", 0, 4), "4:3");
    assert_eq!(f("path + a", 0, 0), "7:22");
    assert_eq!(f("=> last", 0, 3), "3:3");
    assert_eq!(f("item.n", 0, 0), "9:14");
    assert_eq!(f("log(err)", 0, 4), "10:17");
    assert_eq!(f("use(v)", 0, 4), "11:18");
    assert_eq!(f("last", 5, 0), "1:7");
}

#[test]
fn python_parameters_win() {
    let text = [
        "x = 0",
        "def f(self, x, *args, y: int = 1, **kw) -> int:",
        "    \"\"\"x in a docstring\"\"\"",
        "    g = lambda x: x + y",
        "    return x + len(args) + kw[\"x\"]",
        "",
        "def h(a): return a",
        "print(x)",
    ]
    .join("\n");
    let f = |n: &str, nth: usize, d: usize| at_text("p/params.py", &text, n, nth, d);
    assert_eq!(f("return x", 0, 7), "2:13");
    assert_eq!(f("len(args)", 0, 4), "2:17");
    assert_eq!(f("kw[", 0, 0), "2:37");
    assert_eq!(f("x + y", 0, 0), "4:16");
    assert_eq!(f("x + y", 0, 4), "2:23");
    assert_eq!(f("return a", 0, 7), "7:7");
    assert_eq!(f("print(x)", 0, 6), "1:1");
}

#[test]
fn pack_for_path() {
    for (p, k) in [
        ("a/b.js", Pack::Js),
        ("a/b.MJS", Pack::Js),
        ("b.cjs", Pack::Js),
        ("b.jsx", Pack::Js),
        ("x/types.d.ts", Pack::Ts),
        ("b.tsx", Pack::Ts),
        ("b.mts", Pack::Ts),
        ("b.py", Pack::Py),
        ("b.pyi", Pack::Py),
    ] {
        assert_eq!(Pack::for_path(p), Some(k), "{p}");
    }
    assert_eq!(Pack::for_path("README.md"), None);
}

#[test]
fn lookup_by_line_col_with_unicode() {
    let jail = RootJail::new(repo()).unwrap();
    // `한글` before the call shifts bytes but not char columns.
    let text = "const 한글 = 1; helper2()\nfunction helper2() {}\n";
    let col = "const 한글 = 1; ".chars().count() as u32 + 2;
    let r = lookup(&jail, "js/x.js", 1, col, Some(text.into())).unwrap();
    assert_eq!(
        r,
        DefResult::One(DefTarget {
            path: "js/x.js".into(),
            line: 2,
            col: 10,
            preview: "function helper2() {}".into()
        })
    );
    let r = lookup(&jail, "js/main.js", 3, 2, None).unwrap();
    assert!(matches!(r, DefResult::One(t) if t.path == "js/util.js"));
    assert!(matches!(
        lookup(&jail, "js/main.js", 99, 1, None).unwrap(),
        DefResult::None { reason } if reason.id == "def-invalid-position"
    ));
    assert_eq!(
        lookup(&jail, "../x.js", 1, 1, Some("x".into()))
            .unwrap_err()
            .id,
        "path-denied"
    );
    assert_eq!(offset_of("ab\n가나다", 2, 3), Some(9));
    assert_eq!(offset_of("ab\n가나다", 2, 99), Some(12));
}
