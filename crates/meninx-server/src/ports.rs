//! `uterm.ports`: TCP ports listening in terminals' process trees (Hermes Desk's 실행 중인 서버,
//! found the way VS Code's port detection does with `autoForwardPortsSource: process`): the
//! listening sockets of the OS with their processes, kept when the process descends from one of
//! the given terminal processes. A server that stops is gone from the next answer.

use std::collections::{BTreeMap, HashMap, HashSet};

use meninx_proto::PortInfo;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

/// Parent links further than this are not followed (a cycle guard).
const MAX_DEPTH: usize = 64;

/// The listening TCP ports of processes under `roots` (process ids), by port.
pub fn under(roots: &[u32]) -> Vec<PortInfo> {
    let mut by_port: BTreeMap<u16, PortInfo> = BTreeMap::new();
    for (_, p) in by_root(roots) {
        by_port.entry(p.port).or_insert(p);
    }
    by_port.into_values().collect()
}

/// The listening TCP ports of processes under `roots`, each with the root it runs under
/// (`meninx_terminal_list`: the ports of each terminal).
pub fn by_root(roots: &[u32]) -> Vec<(u32, PortInfo)> {
    if roots.is_empty() {
        return Vec::new();
    }
    let Ok(all) = listeners::get_all() else {
        return Vec::new();
    };
    let listening: Vec<listeners::Listener> = all
        .into_iter()
        .filter(|l| {
            l.protocol == listeners::Protocol::TCP && l.state == listeners::SocketState::Listen
        })
        .collect();
    if listening.is_empty() {
        return Vec::new();
    }
    let parents = parent_map();
    let roots: HashSet<u32> = roots.iter().copied().collect();
    let mut seen: HashSet<(u32, u16)> = HashSet::new();
    let mut out = Vec::new();
    for l in listening {
        let pid = l.process.pid;
        if let Some(root) = root_of(pid, &roots, &parents)
            && seen.insert((root, l.socket.port()))
        {
            out.push((
                root,
                PortInfo {
                    port: l.socket.port(),
                    pid,
                    process: l.process.name.clone(),
                },
            ));
        }
    }
    out.sort_by_key(|(r, p)| (*r, p.port));
    out
}

/// Every process' parent.
fn parent_map() -> HashMap<u32, u32> {
    let Ok(mut sys) = System::new() else {
        return HashMap::new();
    };
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    sys.processes()
        .iter()
        .filter_map(|(pid, p)| p.parent().map(|pp| (pid.as_u32(), pp.as_u32())))
        .collect()
}

/// The one of `roots` that `pid` is or runs below.
fn root_of(mut pid: u32, roots: &HashSet<u32>, parents: &HashMap<u32, u32>) -> Option<u32> {
    for _ in 0..MAX_DEPTH {
        if roots.contains(&pid) {
            return Some(pid);
        }
        match parents.get(&pid) {
            Some(&p) if p != pid && p != 0 => pid = p,
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descendants_follow_parent_links() {
        let parents: HashMap<u32, u32> = [(30, 20), (20, 10), (40, 1), (5, 5)].into_iter().collect();
        let roots: HashSet<u32> = [10, 40].into_iter().collect();
        assert_eq!(root_of(30, &roots, &parents), Some(10));
        assert_eq!(root_of(10, &roots, &parents), Some(10));
        assert_eq!(root_of(40, &roots, &parents), Some(40));
        assert_eq!(root_of(5, &roots, &parents), None);
        assert_eq!(root_of(1, &roots, &parents), None);
    }

    #[test]
    fn a_listening_child_of_this_process_is_found() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let me = std::process::id();
        let found = under(&[me]);
        assert!(found.iter().any(|p| p.port == port), "{found:?}");
        assert!(by_root(&[me]).iter().any(|(r, p)| *r == me && p.port == port));
        drop(l);
    }
}
