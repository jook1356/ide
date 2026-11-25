//! UI translations for the Meninx app.
//!
//! Translations are [Project Fluent](https://projectfluent.org) files embedded at compile time:
//! `locales/<code>/meninx.ftl`, English (`en`) being the reference and fallback. `build.rs` generates
//! one function per English message in [`tr`] (`new-session` -> [`tr::new_session`]), so a wrong
//! key or a missing argument is a compile error, and it fails the build when a locale does not
//! have exactly the English messages and variables.
//!
//! ```
//! use meninx_i18n::{Lang, set_language, tr};
//! set_language(Lang::Ko);
//! assert_eq!(tr::new_session(), "새 세션");
//! assert_eq!(tr::configuration_sessions(3, 1), "세션 3개, 실행 중 1개");
//! set_language(Lang::En);
//! assert_eq!(tr::configuration_sessions(1, 0), "1 session, 0 running");
//! ```
//!
//! Parameter types: a variable is `i64` when the English message uses it as a plural selector
//! (`{ $count -> [one] ... *[other] ... }`) or in `NUMBER($count)`; every other variable is
//! `&str`.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource};
use unic_langid::LanguageIdentifier;

mod generated {
    include!(concat!(env!("OUT_DIR"), "/lang.rs"));
}
pub use generated::Lang;

/// Generated translation functions, one per message of `locales/en/meninx.ftl`.
pub mod tr {
    include!(concat!(env!("OUT_DIR"), "/tr.rs"));
}

/// Type of a generated function's parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// `&str`
    Str,
    /// `i64`
    Num,
}

/// Description of one generated `tr::` function (see [`tr::MESSAGES`]).
#[derive(Debug, Clone, Copy)]
pub struct MessageInfo {
    /// Rust function name in [`tr`].
    pub function: &'static str,
    /// Fluent message id.
    pub id: &'static str,
    /// Fluent attribute name, if the function formats an attribute.
    pub attribute: Option<&'static str>,
    /// Parameters, in order.
    pub args: &'static [(&'static str, ArgKind)],
}

/// The user's language choice, as persisted in settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LanguagePref {
    /// Follow the OS UI language (English if it is not translated).
    #[default]
    System,
    /// Always use this language.
    Fixed(Lang),
}

impl LanguagePref {
    /// Every choice for a picker: `System`, then each language.
    pub fn all() -> Vec<LanguagePref> {
        std::iter::once(LanguagePref::System)
            .chain(Lang::ALL.iter().map(|l| LanguagePref::Fixed(*l)))
            .collect()
    }

    /// The language this choice means right now.
    pub fn resolve(self) -> Lang {
        match self {
            LanguagePref::System => system_language(),
            LanguagePref::Fixed(l) => l,
        }
    }

    /// Persisted form: `"system"` or a language code.
    pub fn code(self) -> &'static str {
        match self {
            LanguagePref::System => "system",
            LanguagePref::Fixed(l) => l.code(),
        }
    }

    /// Parse the persisted form; anything unknown is `System`.
    pub fn from_code(s: &str) -> LanguagePref {
        Lang::from_code(s).map_or(LanguagePref::System, LanguagePref::Fixed)
    }
}

impl serde::Serialize for LanguagePref {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.code())
    }
}

impl<'de> serde::Deserialize<'de> for LanguagePref {
    /// Lenient: a string is parsed with [`LanguagePref::from_code`]; null, numbers, booleans and
    /// unknown codes (e.g. a language removed later) mean `System`.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = LanguagePref;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("\"system\" or a language code")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LanguagePref, E> {
                Ok(LanguagePref::from_code(v))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<LanguagePref, E> {
                Ok(LanguagePref::System)
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<LanguagePref, E> {
                Ok(LanguagePref::System)
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<LanguagePref, E> {
                Ok(LanguagePref::System)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<LanguagePref, E> {
                Ok(LanguagePref::System)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<LanguagePref, E> {
                Ok(LanguagePref::System)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<LanguagePref, E> {
                Ok(LanguagePref::System)
            }
        }
        d.deserialize_any(V)
    }
}

impl Lang {
    /// The language with this code (`"en"`, `"ko"`, case-insensitive).
    pub fn from_code(code: &str) -> Option<Lang> {
        Lang::ALL
            .iter()
            .copied()
            .find(|l| l.code().eq_ignore_ascii_case(code))
    }
}

impl std::fmt::Display for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.native_name())
    }
}

static CURRENT: AtomicUsize = AtomicUsize::new(0);

static BUNDLES: LazyLock<Vec<FluentBundle<FluentResource>>> = LazyLock::new(|| {
    generated::SOURCES
        .iter()
        .map(|(code, src)| {
            let id: LanguageIdentifier = code.parse().expect("locale code (checked by build.rs)");
            let mut bundle = FluentBundle::new_concurrent(vec![id]);
            // No U+2068/U+2069 isolation marks around placeables: the UI would draw them.
            bundle.set_use_isolating(false);
            bundle.add_builtins().expect("NUMBER() registered once");
            // Syntax is checked by build.rs, so the error case cannot happen.
            let res = FluentResource::try_new((*src).to_owned()).unwrap_or_else(|(r, _)| r);
            bundle
                .add_resource(res)
                .expect("unique message ids (checked by build.rs)");
            bundle
        })
        .collect()
});

/// Switch the UI language (takes effect on the next `view`).
pub fn set_language(lang: Lang) {
    CURRENT.store(lang.index(), Ordering::Relaxed);
}

/// The current UI language.
pub fn current() -> Lang {
    Lang::ALL[CURRENT.load(Ordering::Relaxed)]
}

/// Resolve `pref` and make it the current language; returns the language now in use.
pub fn apply(pref: LanguagePref) -> Lang {
    let lang = pref.resolve();
    set_language(lang);
    lang
}

/// The best language for the OS UI locales, [`Lang::FALLBACK`] if none is translated.
pub fn system_language() -> Lang {
    let locales: Vec<String> = sys_locale::get_locales().collect();
    negotiate(&locales)
}

/// The best language for a list of requested BCP 47 locales (most preferred first), e.g.
/// `["ko-KR", "en-US"]`; [`Lang::FALLBACK`] if none matches.
pub fn negotiate<S: AsRef<str>>(requested: &[S]) -> Lang {
    let requested: Vec<LanguageIdentifier> = requested
        .iter()
        .filter_map(|s| s.as_ref().replace('_', "-").parse().ok())
        .collect();
    let available: Vec<LanguageIdentifier> = Lang::ALL
        .iter()
        .map(|l| l.code().parse().expect("locale code"))
        .collect();
    let default: LanguageIdentifier = Lang::FALLBACK.code().parse().expect("locale code");
    let chosen = fluent_langneg::negotiate_languages(
        &requested,
        &available,
        Some(&default),
        fluent_langneg::NegotiationStrategy::Lookup,
    );
    chosen
        .first()
        .and_then(|id| Lang::from_code(&id.to_string()))
        .unwrap_or(Lang::FALLBACK)
}

/// Format a message (or attribute) in the current language; used by the generated [`tr`]
/// functions. Falls back to English if the message is missing (cannot happen after the
/// build-time checks) and to the id itself as a last resort.
#[doc(hidden)]
pub fn format(id: &str, attr: Option<&str>, args: Option<&FluentArgs<'_>>) -> String {
    let lang = current();
    format_in(lang, id, attr, args)
        .or_else(|| format_in(Lang::FALLBACK, id, attr, args))
        .unwrap_or_else(|| id.to_owned())
}

fn format_in(
    lang: Lang,
    id: &str,
    attr: Option<&str>,
    args: Option<&FluentArgs<'_>>,
) -> Option<String> {
    let bundle = &BUNDLES[lang.index()];
    let msg = bundle.get_message(id)?;
    let pattern = match attr {
        Some(a) => msg.get_attribute(a)?.value(),
        None => msg.value()?,
    };
    let mut errors = Vec::new();
    let s = bundle.format_pattern(pattern, args, &mut errors);
    debug_assert!(
        errors.is_empty(),
        "formatting {id} ({}): {errors:?}",
        lang.code()
    );
    Some(s.into_owned())
}

/// Format a generated message in `lang` with sample arguments; `Err` lists Fluent's errors.
#[doc(hidden)]
pub fn check_message(lang: Lang, info: &MessageInfo) -> Result<String, String> {
    let bundle = &BUNDLES[lang.index()];
    let msg = bundle
        .get_message(info.id)
        .ok_or_else(|| format!("missing message {}", info.id))?;
    let pattern = match info.attribute {
        Some(a) => msg.get_attribute(a).map(|a| a.value()),
        None => msg.value(),
    }
    .ok_or_else(|| format!("missing pattern for {}", info.function))?;
    let mut args = FluentArgs::new();
    for (name, kind) in info.args {
        match kind {
            ArgKind::Num => args.set(*name, 2),
            ArgKind::Str => args.set(*name, format!("<{name}>")),
        }
    }
    let mut errors = Vec::new();
    let s = bundle.format_pattern(pattern, Some(&args), &mut errors);
    if errors.is_empty() {
        Ok(s.into_owned())
    } else {
        Err(format!("{errors:?}"))
    }
}

#[cfg(test)]
mod tests;
