//! Identity of the server's OS user and the per-user local socket name.

use ide_core::Paths;
use ide_proto::UserInfo;

/// The OS user running this process.
pub fn current_user() -> UserInfo {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let name = var("USERNAME")
        .or_else(|| var("USER"))
        .or_else(|| var("LOGNAME"))
        .unwrap_or_else(|| "user".into());
    let id = if cfg!(windows) {
        match var("USERDOMAIN") {
            Some(d) => format!("{d}\\{name}"),
            None => name.clone(),
        }
    } else {
        name.clone()
    };
    UserInfo { name, id }
}

/// Environment variable overriding the local socket (pipe name or socket path).
pub const SOCKET_ENV: &str = "IDE_SERVER_SOCKET";

/// The daemon's local socket: `\\.\pipe\ide-server-<user>` on Windows, `<data>/ide-server.sock`
/// elsewhere. With `IDE_DATA_DIR` set, the pipe name gets a hash of the data dir so separate
/// data dirs (tests, custom setups) get separate daemons. `IDE_SERVER_SOCKET` overrides it,
/// except inside an agent session (`IDE_SESSION_ID` set): there it is the bridge's socket,
/// given to every process of the session, and an app or daemon started from the session (a
/// test with its own `IDE_DATA_DIR`) would reach the user's daemon and could stop it (seen
/// 2026-11-24: an app script's server stop ended the user's sessions). The bridge's own
/// subcommands read it directly.
pub fn socket_name(paths: &Paths, user: &UserInfo) -> String {
    let in_session = std::env::var_os(crate::bridge::SESSION_ENV).is_some_and(|v| !v.is_empty());
    socket_name_in(paths, user, std::env::var(SOCKET_ENV).ok().filter(|_| !in_session))
}

fn socket_name_in(paths: &Paths, user: &UserInfo, explicit: Option<String>) -> String {
    if let Some(s) = explicit.filter(|s| !s.is_empty()) {
        return s;
    }
    if cfg!(windows) {
        let mut name = format!(r"\\.\pipe\ide-server-{}", sanitize(&user.name));
        if std::env::var_os(ide_core::DATA_DIR_ENV).is_some_and(|v| !v.is_empty()) {
            name.push_str(&format!(
                "-{:08x}",
                fnv(paths.root().to_string_lossy().as_bytes())
            ));
        }
        name
    } else {
        paths.root().join("ide-server.sock").display().to_string()
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn fnv(data: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in data {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(sanitize(r"dom\a b"), "dom_a_b");
        assert_eq!(sanitize("사용자"), "사용자");
        assert_ne!(fnv(b"a"), fnv(b"b"));
        let u = current_user();
        assert!(!u.name.is_empty());
    }

    #[test]
    fn explicit_socket_wins_unless_dropped() {
        let paths = Paths::new(std::env::temp_dir().join("ide-user-socket-test"));
        let u = UserInfo { name: "me".into(), id: "1".into() };
        assert_eq!(socket_name_in(&paths, &u, Some("x".into())), "x");
        let own = socket_name_in(&paths, &u, None);
        assert_ne!(own, "x");
        assert_eq!(socket_name_in(&paths, &u, Some(String::new())), own, "empty = unset");
    }
}
