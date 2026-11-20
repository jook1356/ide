//! Slug rules for profile names and directory names.

use crate::error::{Error, Result};

/// Maximum slug length.
pub const MAX_SLUG_LEN: usize = 64;

/// Validate a slug: 1..=64 chars of `[a-z0-9_-]`, starting with `[a-z0-9]`.
///
/// Slugs are used verbatim as directory names and inside git branch names, so they are
/// restricted to characters that are safe everywhere (no case-folding surprises on Windows).
pub fn validate_slug(name: &str) -> Result<()> {
    let invalid = |reason| {
        Err(Error::InvalidName {
            name: name.to_owned(),
            reason,
        })
    };
    if name.is_empty() {
        return invalid("must not be empty");
    }
    if name.len() > MAX_SLUG_LEN {
        return invalid("must be at most 64 characters");
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return invalid("only lowercase letters, digits, '-' and '_' are allowed");
    }
    if !name.as_bytes()[0].is_ascii_alphanumeric() {
        return invalid("must start with a letter or digit");
    }
    Ok(())
}

/// Turn arbitrary text into a valid slug: lowercase ASCII alphanumerics kept, runs of other
/// characters collapsed to a single `-`, trimmed, truncated. Returns `fallback` if nothing is left.
pub fn slugify(text: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c.to_ascii_lowercase());
        } else {
            dash = true;
        }
        if out.len() >= MAX_SLUG_LEN {
            break;
        }
    }
    out.truncate(MAX_SLUG_LEN);
    let out = out.trim_matches(['-', '_']).to_owned();
    if out.is_empty() {
        fallback.to_owned()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_slugs() {
        for s in ["default", "work", "a", "0", "my-profile_2", &"a".repeat(64)] {
            validate_slug(s).unwrap_or_else(|e| panic!("{s}: {e}"));
        }
    }

    #[test]
    fn invalid_slugs() {
        for s in [
            "",
            "Work",
            "-a",
            "_a",
            "a b",
            "a/b",
            "a.b",
            "프로필",
            "..",
            &"a".repeat(65),
        ] {
            assert!(validate_slug(s).is_err(), "{s:?} should be invalid");
        }
    }

    #[test]
    fn slugify_rules() {
        assert_eq!(slugify("My Repo", "x"), "my-repo");
        assert_eq!(slugify("  --foo..bar--  ", "x"), "foo-bar");
        assert_eq!(slugify("한글", "repo"), "repo");
        assert_eq!(slugify("a한b", "repo"), "a-b");
        assert_eq!(slugify(&"x".repeat(100), "r").len(), MAX_SLUG_LEN);
        for s in ["My Repo", "___", "a__b", "-x-"] {
            validate_slug(&slugify(s, "r")).unwrap();
        }
    }
}
