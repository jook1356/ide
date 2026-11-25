//! Session list order (Hermes Desk's `lib/sessionOrder.ts`): an ordered list of criteria, each
//! ascending or descending; an empty list keeps the server's order. Earlier criteria win, equal
//! values keep the previous order (stable sort) and unknown values (no time yet) stay last in
//! either direction.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

/// What a criterion compares. Hermes Desk's `세션 상태` (git/PR status) has no counterpart yet:
/// the app does not track branch status per session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortKey {
    /// Session title.
    Name,
    /// Last activity (`last_active`).
    Updated,
    /// Creation time.
    Created,
    /// Process state: failed, running, starting, exited, stopped.
    Activity,
}

impl SortKey {
    /// Every key, in the order the "add criterion" list shows them.
    pub const ALL: [SortKey; 4] = [
        SortKey::Name,
        SortKey::Updated,
        SortKey::Created,
        SortKey::Activity,
    ];

    /// Direction a newly added criterion starts with (newest first for times, as Desk).
    pub fn default_dir(self) -> SortDir {
        match self {
            SortKey::Updated | SortKey::Created => SortDir::Desc,
            SortKey::Name | SortKey::Activity => SortDir::Asc,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortDir {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortCriterion {
    pub key: SortKey,
    pub dir: SortDir,
}

/// The values one session is compared by.
#[derive(Debug, Clone, Copy)]
pub struct SortItem<'a> {
    pub name: &'a str,
    /// Unix seconds; 0 = unknown.
    pub updated: u64,
    /// Unix seconds; 0 = unknown.
    pub created: u64,
    /// Rank of the process state (lower = needs attention first).
    pub activity: u8,
}

/// Drop repeated keys (the first one wins), e.g. from a hand-edited `ui.json`.
pub fn normalized(criteria: &[SortCriterion]) -> Vec<SortCriterion> {
    let mut out: Vec<SortCriterion> = Vec::new();
    for c in criteria {
        if !out.iter().any(|x| x.key == c.key) {
            out.push(*c);
        }
    }
    out
}

/// Case-insensitive, digit runs compared by value (`세션 2` before `세션 10`), like Desk's
/// `Intl.Collator('ko', { numeric: true })`.
fn compare_names(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = x.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    x.next();
                }
                let mut nb = String::new();
                while let Some(c) = y.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    y.next();
                }
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(p), Some(q)) => {
                let o = p.to_lowercase().cmp(q.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

fn compare_one(a: &SortItem<'_>, b: &SortItem<'_>, c: SortCriterion) -> Ordering {
    let flip = |o: Ordering| match c.dir {
        SortDir::Asc => o,
        SortDir::Desc => o.reverse(),
    };
    let time = |x: u64, y: u64| match (x, y) {
        // Unknown stays last in either direction.
        (0, 0) => Ordering::Equal,
        (0, _) => Ordering::Greater,
        (_, 0) => Ordering::Less,
        _ => flip(x.cmp(&y)),
    };
    match c.key {
        SortKey::Name => flip(compare_names(a.name, b.name)),
        SortKey::Updated => time(a.updated, b.updated),
        SortKey::Created => time(a.created, b.created),
        SortKey::Activity => flip(a.activity.cmp(&b.activity)),
    }
}

/// Sort `rows` in place by `criteria` (stable; no criteria = unchanged).
pub fn sort_by<T>(rows: &mut [T], criteria: &[SortCriterion], item: impl Fn(&T) -> SortItem<'_>) {
    if criteria.is_empty() {
        return;
    }
    rows.sort_by(|a, b| {
        let (a, b) = (item(a), item(b));
        criteria
            .iter()
            .map(|c| compare_one(&a, &b, *c))
            .find(|o| *o != Ordering::Equal)
            .unwrap_or(Ordering::Equal)
    });
}

/// Move `key` one place up (`delta < 0`) or down; out-of-range moves do nothing.
pub fn move_criterion(criteria: &mut [SortCriterion], key: SortKey, delta: i32) {
    let Some(from) = criteria.iter().position(|c| c.key == key) else {
        return;
    };
    let to = from as i64 + i64::from(delta.signum());
    if (0..criteria.len() as i64).contains(&to) {
        criteria.swap(from, to as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn it(name: &str, updated: u64, created: u64, activity: u8) -> SortItem<'_> {
        SortItem {
            name,
            updated,
            created,
            activity,
        }
    }

    fn names(rows: &[SortItem<'_>]) -> Vec<String> {
        rows.iter().map(|r| r.name.to_owned()).collect()
    }

    const fn c(key: SortKey, dir: SortDir) -> SortCriterion {
        SortCriterion { key, dir }
    }

    #[test]
    fn no_criteria_keeps_order() {
        let mut rows = vec![it("b", 1, 1, 0), it("a", 2, 2, 0)];
        sort_by(&mut rows, &[], |r| *r);
        assert_eq!(names(&rows), ["b", "a"]);
    }

    #[test]
    fn names_are_numeric_and_case_insensitive() {
        let mut rows = vec![
            it("세션 10", 0, 0, 0),
            it("Beta", 0, 0, 0),
            it("세션 2", 0, 0, 0),
            it("alpha", 0, 0, 0),
        ];
        sort_by(&mut rows, &[c(SortKey::Name, SortDir::Asc)], |r| *r);
        assert_eq!(names(&rows), ["alpha", "Beta", "세션 2", "세션 10"]);
        sort_by(&mut rows, &[c(SortKey::Name, SortDir::Desc)], |r| *r);
        assert_eq!(names(&rows), ["세션 10", "세션 2", "Beta", "alpha"]);
    }

    #[test]
    fn unknown_times_stay_last_both_ways() {
        let mut rows = vec![it("x", 0, 0, 0), it("old", 5, 0, 0), it("new", 9, 0, 0)];
        sort_by(&mut rows, &[c(SortKey::Updated, SortDir::Desc)], |r| *r);
        assert_eq!(names(&rows), ["new", "old", "x"]);
        sort_by(&mut rows, &[c(SortKey::Updated, SortDir::Asc)], |r| *r);
        assert_eq!(names(&rows), ["old", "new", "x"]);
    }

    #[test]
    fn later_criteria_break_ties_and_ties_keep_order() {
        let mut rows = vec![
            it("b", 0, 1, 1),
            it("a", 0, 2, 1),
            it("c", 0, 3, 0),
            it("d", 0, 3, 1),
        ];
        let crit = [
            c(SortKey::Activity, SortDir::Asc),
            c(SortKey::Created, SortDir::Desc),
        ];
        sort_by(&mut rows, &crit, |r| *r);
        assert_eq!(names(&rows), ["c", "d", "a", "b"]);
        // Equal on every criterion: previous order kept.
        let mut rows = vec![it("q", 0, 0, 2), it("p", 0, 0, 2)];
        sort_by(&mut rows, &[c(SortKey::Activity, SortDir::Asc)], |r| *r);
        assert_eq!(names(&rows), ["q", "p"]);
    }

    #[test]
    fn normalize_and_move() {
        let mut v = normalized(&[
            c(SortKey::Name, SortDir::Asc),
            c(SortKey::Updated, SortDir::Desc),
            c(SortKey::Name, SortDir::Desc),
        ]);
        assert_eq!(v.len(), 2);
        move_criterion(&mut v, SortKey::Updated, -1);
        assert_eq!(v[0].key, SortKey::Updated);
        move_criterion(&mut v, SortKey::Updated, -1);
        assert_eq!(v[0].key, SortKey::Updated);
        move_criterion(&mut v, SortKey::Name, 1);
        assert_eq!(v[1].key, SortKey::Name);
        assert_eq!(SortKey::Created.default_dir(), SortDir::Desc);
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(
            json,
            r#"[{"key":"updated","dir":"desc"},{"key":"name","dir":"asc"}]"#
        );
    }
}
