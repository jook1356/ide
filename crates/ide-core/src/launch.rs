//! A concrete process to spawn in a PTY, and program resolution helpers. Building the spec of
//! a session is the job of the agent providers (`ide-agents`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Everything needed to spawn an agent. Nothing is launched by `ide-core`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    /// Resolved absolute path of the program.
    pub program: PathBuf,
    /// Arguments (without the program).
    pub args: Vec<String>,
    /// Working directory (the session's worktree).
    pub cwd: PathBuf,
    /// Variables to set on top of the inherited environment.
    pub env: Vec<(String, String)>,
}

/// The parts of the process environment that launch resolution depends on. Injectable so
/// resolution can be tested without touching the real environment.
#[derive(Debug, Clone, Default)]
pub struct LaunchEnv {
    /// `PATH`.
    pub path: Option<OsString>,
    /// `PATHEXT` (Windows only).
    pub pathext: Option<OsString>,
    /// `COMSPEC` (Windows shell).
    pub comspec: Option<OsString>,
    /// `SHELL` (non-Windows shell).
    pub shell: Option<OsString>,
}

impl LaunchEnv {
    /// Read the current process environment.
    pub fn from_process() -> Self {
        Self {
            path: std::env::var_os("PATH"),
            pathext: std::env::var_os("PATHEXT"),
            comspec: std::env::var_os("COMSPEC"),
            shell: std::env::var_os("SHELL"),
        }
    }
}

/// Whether `program` is a Windows batch file (`.cmd` / `.bat`, case-insensitive), which
/// `CreateProcess` cannot start directly.
pub fn is_batch_file(program: &Path) -> bool {
    program
        .extension()
        .map(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
        .unwrap_or(false)
}

/// On Windows, run batch files (npm shims such as `claude.cmd`) through the command
/// interpreter: `cmd.exe /d /s /c <program> <args>`. Other programs, and every program on
/// other platforms, are returned unchanged.
pub fn wrap_batch_file(
    program: PathBuf,
    args: Vec<String>,
    env: &LaunchEnv,
) -> Result<(PathBuf, Vec<String>)> {
    if !cfg!(windows) || !is_batch_file(&program) {
        return Ok((program, args));
    }
    let cmd = cmd_exe(env)?;
    let mut wrapped: Vec<String> = ["/d", "/s", "/c"].map(String::from).into();
    wrapped.push(program.display().to_string());
    wrapped.extend(args);
    Ok((cmd, wrapped))
}

/// Find `program` on `env`'s `PATH` or fail with [`Error::ProgramNotFound`].
pub fn resolve_program(program: &str, env: &LaunchEnv) -> Result<PathBuf> {
    which_in(program, env.path.as_deref(), env.pathext.as_deref()).ok_or_else(|| {
        Error::ProgramNotFound {
            program: program.to_owned(),
        }
    })
}

/// The user's shell: `COMSPEC` (else `cmd.exe`) on Windows, `$SHELL` (else `/bin/sh`) elsewhere.
pub fn shell_program(env: &LaunchEnv) -> Result<PathBuf> {
    let configured = if cfg!(windows) {
        env.comspec.as_ref()
    } else {
        env.shell.as_ref()
    };
    if let Some(p) = configured.filter(|p| !p.is_empty())
        && let Some(found) = which_in(
            &p.to_string_lossy(),
            env.path.as_deref(),
            env.pathext.as_deref(),
        )
    {
        return Ok(found);
    }
    resolve_program(if cfg!(windows) { "cmd.exe" } else { "/bin/sh" }, env)
}

/// `cmd.exe` for running batch files: `COMSPEC` when it names `cmd.exe`, else `cmd.exe` on `PATH`.
fn cmd_exe(env: &LaunchEnv) -> Result<PathBuf> {
    if let Some(p) = env.comspec.as_ref().filter(|p| !p.is_empty())
        && Path::new(p)
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("cmd.exe"))
        && let Some(found) = which_in(
            &p.to_string_lossy(),
            env.path.as_deref(),
            env.pathext.as_deref(),
        )
    {
        return Ok(found);
    }
    resolve_program("cmd.exe", env)
}

/// Find `program` on the current `PATH` (honouring `PATHEXT` on Windows).
pub fn which(program: &str) -> Option<PathBuf> {
    let env = LaunchEnv::from_process();
    which_in(program, env.path.as_deref(), env.pathext.as_deref())
}

/// Find `program` in `path`. A program containing a path separator is checked directly.
/// On Windows, a name without one of the `pathext` extensions is tried with each of them
/// (default `.COM;.EXE;.BAT;.CMD`), in order, per directory.
pub fn which_in(
    program: &str,
    path: Option<&std::ffi::OsStr>,
    pathext: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    if program.is_empty() {
        return None;
    }
    let exts: Vec<String> = if cfg!(windows) {
        let raw = pathext
            .map(|p| p.to_string_lossy().into_owned())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
        raw.split(';')
            .filter(|e| !e.is_empty())
            .map(|e| e.to_ascii_lowercase())
            .collect()
    } else {
        Vec::new()
    };
    let lower = program.to_ascii_lowercase();
    let has_ext = exts.iter().any(|e| lower.ends_with(e.as_str()));
    let candidates = |base: PathBuf| -> Option<PathBuf> {
        if (!cfg!(windows) || has_ext) && is_executable(&base) {
            return Some(base);
        }
        if cfg!(windows) && !has_ext {
            for ext in &exts {
                let mut s = base.clone().into_os_string();
                s.push(ext);
                let p = PathBuf::from(s);
                if is_executable(&p) {
                    return Some(p);
                }
            }
        }
        None
    };

    if program.contains('/') || (cfg!(windows) && program.contains('\\')) {
        return candidates(PathBuf::from(program));
    }
    std::env::split_paths(path?)
        .filter(|d| !d.as_os_str().is_empty())
        .find_map(|dir| candidates(dir.join(program)))
}

fn is_executable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    meta.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::test_util::TempDir;

    fn fake_exe(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    fn exe(name: &str) -> String {
        if cfg!(windows) {
            format!("{name}.cmd")
        } else {
            name.to_owned()
        }
    }

    #[test]
    fn batch_files_wrapped_on_windows() {
        assert!(is_batch_file(Path::new("C:/x/claude.CMD")));
        assert!(is_batch_file(Path::new("run.bat")));
        assert!(!is_batch_file(Path::new("codex.exe")));
        assert!(!is_batch_file(Path::new("cmd")));

        let t = TempDir::new("launch-batch");
        let bin = t.0.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let cmd = fake_exe(&bin, "cmd.exe");
        let pwsh = fake_exe(&bin, "pwsh.exe");
        let env = LaunchEnv {
            path: Some(bin.clone().into_os_string()),
            pathext: Some(".EXE;.BAT;.CMD".into()),
            // A non-cmd COMSPEC is not used to run batch files.
            comspec: Some(pwsh.clone().into()),
            shell: None,
        };
        let bat = fake_exe(&bin, "codex.bat");
        let found = resolve_program("codex", &env).unwrap();
        assert_eq!(found, bat);
        let (p, args) = wrap_batch_file(found, vec!["--x".into()], &env).unwrap();
        if cfg!(windows) {
            assert_eq!(p, cmd);
            assert_eq!(
                args,
                ["/d", "/s", "/c", &bat.display().to_string(), "--x"].map(String::from)
            );
        } else {
            assert_eq!(args, ["--x"]);
        }
        // With an .exe next to it, the .exe wins (PATHEXT order) and runs directly.
        let exe_file = fake_exe(&bin, "codex.exe");
        if cfg!(windows) {
            let found = resolve_program("codex", &env).unwrap();
            assert_eq!(found, exe_file);
            let (p, args) = wrap_batch_file(found, Vec::new(), &env).unwrap();
            assert_eq!(p, exe_file);
            assert!(args.is_empty());
            // The shell is COMSPEC when it can be found.
            assert_eq!(shell_program(&env).unwrap(), pwsh);
        }
    }

    #[test]
    fn missing_program_is_clear_error() {
        let t = TempDir::new("launch-missing");
        let env = LaunchEnv {
            path: Some(t.0.clone().into_os_string()),
            ..Default::default()
        };
        let err = resolve_program("codex", &env).unwrap_err();
        assert!(matches!(&err, Error::ProgramNotFound { program } if program == "codex"));
        assert!(err.to_string().contains("not found on PATH"));
    }

    #[test]
    fn which_rules() {
        let t = TempDir::new("which");
        let a = t.0.join("a");
        let b = t.0.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let path = std::env::join_paths([a.clone(), b.clone()]).unwrap();
        let in_b = fake_exe(&b, &exe("tool"));
        assert_eq!(which_in("tool", Some(&path), None), Some(in_b.clone()));
        // Earlier PATH entries win.
        let in_a = fake_exe(&a, &exe("tool"));
        assert_eq!(which_in("tool", Some(&path), None), Some(in_a.clone()));
        // Directories are not programs.
        std::fs::create_dir_all(a.join("dir")).unwrap();
        assert_eq!(which_in("dir", Some(&path), None), None);
        // Explicit paths are checked directly.
        assert_eq!(
            which_in(&in_b.display().to_string(), None, None),
            Some(in_b)
        );
        assert_eq!(which_in("nope", Some(&path), None), None);
        assert_eq!(which_in("tool", None, None), None);
        assert_eq!(which_in("", Some(&path), None), None);
        if cfg!(windows) {
            // Explicit extension, case-insensitive; PATHEXT order respected.
            assert_eq!(
                which_in("tool.CMD", Some(&path), None),
                Some(a.join("tool.CMD"))
            );
            let exe_file = fake_exe(&a, "tool.exe");
            assert_eq!(
                which_in("tool", Some(&path), Some(".EXE;.CMD".as_ref())),
                Some(exe_file)
            );
            assert_eq!(which_in("tool", Some(&path), Some(".BAT".as_ref())), None);
            let _ = in_a;
        }
    }
}
