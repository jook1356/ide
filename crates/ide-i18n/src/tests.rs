use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::*;

/// Tests that switch the global language hold this lock.
static LANG_LOCK: Mutex<()> = Mutex::new(());

/// Generated functions that may stay unused (none today). Add a function name here only with
/// a comment saying where it will be used.
const ALLOW_UNUSED: &[&str] = &[];

#[test]
fn every_message_formats_in_every_locale() {
    let mut failures = Vec::new();
    for lang in Lang::ALL {
        for info in tr::MESSAGES {
            match check_message(*lang, info) {
                Ok(s) => {
                    if s.contains(['\u{2068}', '\u{2069}']) {
                        failures.push(format!(
                            "{} {}: isolation marks",
                            lang.code(),
                            info.function
                        ));
                    }
                    if s.trim().is_empty() {
                        failures.push(format!("{} {}: empty", lang.code(), info.function));
                    }
                    for (name, _) in info.args {
                        if !s.contains(&format!("<{name}>")) && !s.contains('2') {
                            failures.push(format!(
                                "{} {}: argument ${name} is not shown",
                                lang.code(),
                                info.function
                            ));
                        }
                    }
                }
                Err(e) => failures.push(format!("{} {}: {e}", lang.code(), info.function)),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "format failures:\n{}",
        failures.join("\n")
    );
}

#[test]
fn switching_language() {
    let _g = LANG_LOCK.lock().unwrap();
    set_language(Lang::Ko);
    assert_eq!(current(), Lang::Ko);
    assert_eq!(tr::new_session(), "새 세션");
    assert_eq!(tr::profile_sessions(3, 1), "세션 3개, 실행 중 1개");
    assert_eq!(
        tr::delete_session_title("shell A"),
        "“shell A” 세션을 삭제할까요?"
    );
    set_language(Lang::En);
    assert_eq!(tr::new_session(), "New session");
    assert_eq!(tr::profile_sessions(1, 0), "1 session, 0 running");
    assert_eq!(tr::profile_sessions(2, 1), "2 sessions, 1 running");
    assert_eq!(
        tr::err_profile_running(1, "default"),
        "1 session of profile “default” is running; stop them before renaming."
    );
    assert_eq!(
        tr::err_not_a_git_repo("C:/x", "fatal"),
        "C:/x is not a git repository: fatal"
    );
}

#[test]
fn language_names_and_codes() {
    assert_eq!(Lang::ALL[0], Lang::FALLBACK);
    assert_eq!(Lang::En.native_name(), "English");
    assert_eq!(Lang::Ko.native_name(), "한국어");
    for l in Lang::ALL {
        assert_eq!(Lang::from_code(l.code()), Some(*l));
    }
    assert_eq!(Lang::from_code("KO"), Some(Lang::Ko));
    assert_eq!(Lang::from_code("fr"), None);
}

#[test]
fn negotiation_from_os_locales() {
    assert_eq!(negotiate(&["ko-KR"]), Lang::Ko);
    assert_eq!(negotiate(&["ko"]), Lang::Ko);
    assert_eq!(negotiate(&["en-US", "ko-KR"]), Lang::En);
    assert_eq!(negotiate(&["fr-FR", "ko-KR"]), Lang::Ko);
    assert_eq!(negotiate(&["fr-FR"]), Lang::En);
    assert_eq!(negotiate(&["ko_KR"]), Lang::Ko);
    assert_eq!(negotiate::<&str>(&[]), Lang::En);
    assert_eq!(negotiate(&["not a locale!"]), Lang::En);
    // Whatever the OS says, it resolves to a supported language.
    assert!(Lang::ALL.contains(&system_language()));
}

#[test]
fn pref_serde_is_lenient() {
    let s = |p: LanguagePref| serde_json::to_string(&p).unwrap();
    let d = |j: &str| serde_json::from_str::<LanguagePref>(j).unwrap();
    assert_eq!(s(LanguagePref::System), "\"system\"");
    assert_eq!(s(LanguagePref::Fixed(Lang::Ko)), "\"ko\"");
    for p in LanguagePref::all() {
        assert_eq!(d(&s(p)), p);
    }
    assert_eq!(d("\"xx\""), LanguagePref::System);
    assert_eq!(d("null"), LanguagePref::System);
    assert_eq!(d("3"), LanguagePref::System);
    assert_eq!(LanguagePref::all().len(), 1 + Lang::ALL.len());
    assert_eq!(LanguagePref::Fixed(Lang::Ko).resolve(), Lang::Ko);
}

/// Korean text must not use broken particle pairs like `은(는)` / `을(를)`.
#[test]
fn korean_has_no_particle_pairs() {
    let src = std::fs::read_to_string(manifest_dir().join("locales/ko/ide.ftl")).unwrap();
    let bad = [
        "(은)", "(는)", "(을)", "(를)", "(이)", "(가)", "(과)", "(와)", "(으)",
    ];
    let hits: Vec<_> = src
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter(|l| bad.iter().any(|b| l.contains(b)))
        .collect();
    assert!(
        hits.is_empty(),
        "particle pairs in ko/ide.ftl:\n{}",
        hits.join("\n")
    );
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Every generated function is called somewhere in `crates/*/src` (outside this crate).
#[test]
fn no_unused_messages() {
    let crates = manifest_dir().parent().unwrap().to_owned();
    let mut files = Vec::new();
    for c in std::fs::read_dir(&crates).unwrap().filter_map(|e| e.ok()) {
        if c.file_name() != "ide-i18n" {
            rust_files(&c.path().join("src"), &mut files);
        }
    }
    assert!(
        !files.is_empty(),
        "no sources found under {}",
        crates.display()
    );
    let sources: String = files
        .iter()
        .map(|f| std::fs::read_to_string(f).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let used = |name: &str| {
        let needle = format!("tr::{name}");
        sources.match_indices(&needle).any(|(i, _)| {
            !sources[i + needle.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    };
    let unused: Vec<_> = tr::MESSAGES
        .iter()
        .map(|m| m.function)
        .filter(|f| !ALLOW_UNUSED.contains(f) && !used(f))
        .collect();
    assert!(
        unused.is_empty(),
        "{} unused message(s) (remove them from every locale, or add to ALLOW_UNUSED): {}",
        unused.len(),
        unused.join(", ")
    );
    let stale: Vec<_> = ALLOW_UNUSED
        .iter()
        .filter(|f| !tr::MESSAGES.iter().any(|m| m.function == **f))
        .collect();
    assert!(
        stale.is_empty(),
        "ALLOW_UNUSED names unknown functions: {stale:?}"
    );
}
