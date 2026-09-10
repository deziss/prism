//! PATH shims — one interception mechanism for every client.
//!
//! A filter that never runs saves nothing, and the filters are prism's only lever whose
//! savings compound across turns: a tool result cut from 40k to 4k tokens is not saved
//! once, it is saved again on every later turn that re-sends the conversation.
//!
//! So the question is how to get *every* agent to call `prism cmd` instead of the raw
//! tool. Writing a bespoke integration per client — Claude Code, Cursor, Codex, Aider,
//! Cline, Windsurf, OpenCode, … — means N integrations that each break on their own
//! schedule. But all of them share one behaviour: they run shell commands. So put a
//! directory of tiny executables ahead of the real ones on `PATH` and every client is
//! covered by the same code, including clients that do not exist yet.
//!
//! Two hazards come with that, and both are handled here rather than left to the user:
//!
//! - **Recursion.** `prism cmd git` spawns `git`, which resolves to the shim again.
//!   [`strip_from_path`] removes the shim directory from the child's `PATH`, so the real
//!   binary is found; `PRISM_SHIM_DEPTH` is a second, independent guard. Note that the
//!   shim resolves the tool *dynamically* rather than baking in an absolute path — that
//!   is what keeps `nvm`, `rbenv`, `pyenv` and friends working.
//! - **Interactive commands.** `git rebase -i` must not have its output captured. The
//!   rule is the one piece of information that actually distinguishes the two callers:
//!   when stdout is a terminal a human is reading, so pass the tool through untouched;
//!   when it is a pipe an agent is reading, so filter. `PRISM_SHIM=always|auto|off`
//!   overrides.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Marker on the first line of every generated shim, used to recognise ours.
const MARKER: &str = "# PRISM shim";

pub fn shim_dir() -> PathBuf {
    crate::prism_data_dir().join("shims")
}

/// Tools to shim: everything with a real filter, plus whatever the user's YAML rules
/// add, so a rule file is enough to get a tool intercepted end to end.
pub fn shimmable() -> Vec<String> {
    let mut v: Vec<String> = crate::filter::FILTERED_TOOLS
        .iter()
        .map(|s| s.to_string())
        .collect();
    for t in crate::filter::loaded_rules().tools() {
        if !v.iter().any(|x| x == t) {
            v.push(t.to_string());
        }
    }
    v.sort();
    v
}

/// True when `p` lives inside a Cargo build directory.
///
/// Shims hard-code the path of the binary that installed them, so pointing them at
/// `target/debug` or `target/release` means a later `cargo clean` — or moving the repo —
/// breaks every shimmed command on the machine, which is every `git`, `ls` and `find`
/// the user runs. Worth refusing to do silently.
pub fn is_build_artifact(p: &Path) -> bool {
    // A profile directory anywhere *below* a `target/` component, not just immediately
    // inside it: a cross-compiled binary lives at `target/<triple>/release/prism`.
    let mut seen_target = false;
    for c in p.components() {
        let s = c.as_os_str().to_string_lossy();
        if seen_target && (s == "debug" || s == "release") {
            return true;
        }
        if s == "target" {
            seen_target = true;
        }
    }
    false
}

#[derive(Debug, Default)]
pub struct Report {
    pub written: Vec<String>,
    /// Set when the installing binary lives in a Cargo build directory.
    pub volatile_binary: Option<PathBuf>,
    /// Shimmable tools that are not installed on this machine. Skipped: a shim for a
    /// missing tool turns "command not found" into a confusing prism error.
    pub absent: Vec<String>,
    pub dir: PathBuf,
}

fn on_path(tool: &str, path: &str) -> bool {
    path.split(':').filter(|d| !d.is_empty()).any(|d| {
        let p = Path::new(d).join(tool);
        p.is_file() || p.is_symlink()
    })
}

/// Write a shim for every filtered tool that exists on this machine.
pub fn install() -> Result<Report> {
    let dir = shim_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let prism = std::env::current_exe().context("locating the prism binary")?;
    // Look for the real tools with our own directory excluded, or a reinstall would only
    // ever find the shims it wrote last time.
    let clean = strip_from_path(&std::env::var("PATH").unwrap_or_default());

    let mut report = Report {
        dir: dir.clone(),
        ..Default::default()
    };
    if is_build_artifact(&prism) {
        report.volatile_binary = Some(prism.clone());
    }
    for tool in shimmable() {
        // A name with a path separator is not resolved through PATH, so a shim for it
        // could never be reached.
        if tool.contains('/') {
            continue;
        }
        if !on_path(&tool, &clean) {
            report.absent.push(tool);
            continue;
        }
        let path = dir.join(&tool);
        std::fs::write(&path, shim_script(&prism, &tool))
            .with_context(|| format!("writing {}", path.display()))?;
        set_executable(&path)?;
        report.written.push(tool);
    }
    Ok(report)
}

fn shim_script(prism: &Path, tool: &str) -> String {
    format!(
        r#"#!/bin/sh
{MARKER} for `{tool}` — generated by `prism shim install`. Do not edit.
#
# prism removes this directory from PATH before spawning the real tool, so the
# lookup stays dynamic and version managers (nvm, rbenv, pyenv) keep working.
exec "{prism}" cmd {tool} "$@"
"#,
        prism = prism.display(),
    )
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Remove every shim prism generated. Only files carrying [`MARKER`] are touched, so a
/// name collision cannot make this delete something a user put there.
pub fn uninstall() -> Result<usize> {
    let dir = shim_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(0);
    };
    let mut n = 0;
    for e in entries.flatten() {
        let p = e.path();
        let ours = std::fs::read_to_string(&p)
            .map(|s| s.contains(MARKER))
            .unwrap_or(false);
        if ours && std::fs::remove_file(&p).is_ok() {
            n += 1;
        }
    }
    let _ = std::fs::remove_dir(&dir); // only succeeds when empty, which is what we want
    Ok(n)
}

#[derive(Debug)]
pub struct Status {
    pub dir: PathBuf,
    pub installed: usize,
    /// Whether the shim directory precedes the real tools on the current `PATH`.
    pub active: bool,
    pub mode: Mode,
}

pub fn status() -> Status {
    let dir = shim_dir();
    let installed = std::fs::read_dir(&dir)
        .map(|es| {
            es.flatten()
                .filter(|e| {
                    std::fs::read_to_string(e.path())
                        .map(|s| s.contains(MARKER))
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);
    let path = std::env::var("PATH").unwrap_or_default();
    Status {
        active: path_leads_with(&path, &dir),
        dir,
        installed,
        mode: mode(),
    }
}

/// True when `dir` appears on `path` ahead of any other entry — the only arrangement in
/// which the shims actually win the lookup.
fn path_leads_with(path: &str, dir: &Path) -> bool {
    let target = dir.to_string_lossy();
    path.split(':')
        .find(|d| !d.is_empty())
        .map(|d| d == target)
        .unwrap_or(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Filter only when stdout is not a terminal — an agent is reading, not a person.
    Auto,
    /// Always filter, terminal or not.
    Always,
    /// Never filter; shims become transparent passthroughs.
    Off,
}

pub fn parse_mode(v: Option<&str>) -> Mode {
    match v.map(str::trim) {
        Some("off") | Some("0") | Some("false") | Some("no") => Mode::Off,
        Some("always") | Some("1") | Some("true") => Mode::Always,
        _ => Mode::Auto,
    }
}

pub fn mode() -> Mode {
    parse_mode(std::env::var("PRISM_SHIM").ok().as_deref())
}

/// Whether `prism cmd` should filter this invocation, or hand the tool through untouched.
///
/// `stdout_is_tty` is passed in rather than probed so the decision is testable.
pub fn should_filter(mode: Mode, stdout_is_tty: bool, depth: u32) -> bool {
    // Already inside a prism-spawned tool: filtering again would mean prism ran itself,
    // which the PATH sanitising should already prevent. Belt and braces.
    if depth > 0 {
        return false;
    }
    match mode {
        Mode::Off => false,
        Mode::Always => true,
        Mode::Auto => !stdout_is_tty,
    }
}

/// Remove the shim directory from a `PATH` value, preserving order and separators.
pub fn strip_from_path(path: &str) -> String {
    let dir = shim_dir();
    let target = dir.to_string_lossy().to_string();
    path.split(':')
        .filter(|d| *d != target.as_str())
        .collect::<Vec<_>>()
        .join(":")
}

/// The line to add to a shell rc, or to a client's `env` configuration.
pub fn path_line() -> String {
    format!("export PATH=\"{}:$PATH\"", shim_dir().display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_directory_binary_is_recognised_as_volatile() {
        for p in [
            "/home/u/proj/target/debug/prism",
            "/home/u/proj/target/release/prism",
            "/home/u/proj/target/x86_64-unknown-linux-gnu/release/prism",
        ] {
            assert!(is_build_artifact(Path::new(p)), "{p}");
        }
        for p in [
            "/usr/local/bin/prism",
            "/home/u/.local/bin/prism",
            "/opt/prism/bin/prism",
        ] {
            assert!(!is_build_artifact(Path::new(p)), "{p}");
        }
        // a directory merely named "release" outside a target/ tree is fine
        assert!(!is_build_artifact(Path::new("/home/u/release/prism")));
    }

    #[test]
    fn stripping_removes_only_the_shim_dir_and_keeps_order() {
        let d = shim_dir().to_string_lossy().to_string();
        let path = format!("{d}:/usr/local/bin:/usr/bin:/bin");
        assert_eq!(strip_from_path(&path), "/usr/local/bin:/usr/bin:/bin");
        // in the middle, and appearing twice
        let path = format!("/usr/local/bin:{d}:/usr/bin:{d}:/bin");
        assert_eq!(strip_from_path(&path), "/usr/local/bin:/usr/bin:/bin");
        // absent: unchanged
        assert_eq!(strip_from_path("/usr/bin:/bin"), "/usr/bin:/bin");
        // a directory that merely starts with the same text must survive
        let path = format!("{d}-backup:/usr/bin");
        assert_eq!(strip_from_path(&path), format!("{d}-backup:/usr/bin"));
    }

    #[test]
    fn the_tty_decides_who_is_reading() {
        // A person at a terminal gets the real tool; an agent reading a pipe gets the
        // filtered form. This is the whole interactive-command story.
        assert!(
            !should_filter(Mode::Auto, true, 0),
            "a human at a tty must see raw output"
        );
        assert!(
            should_filter(Mode::Auto, false, 0),
            "a pipe means an agent is reading"
        );
        assert!(should_filter(Mode::Always, true, 0));
        assert!(!should_filter(Mode::Off, false, 0));
        // recursion guard beats every mode
        for m in [Mode::Auto, Mode::Always, Mode::Off] {
            assert!(!should_filter(m, false, 1), "depth>0 must never filter");
        }
    }

    #[test]
    fn mode_parsing_defaults_to_auto_and_ignores_noise() {
        assert_eq!(parse_mode(None), Mode::Auto);
        assert_eq!(parse_mode(Some("")), Mode::Auto);
        assert_eq!(parse_mode(Some("banana")), Mode::Auto);
        assert_eq!(parse_mode(Some("off")), Mode::Off);
        assert_eq!(parse_mode(Some(" always ")), Mode::Always);
    }

    #[test]
    fn path_activation_requires_leading_position() {
        let d = shim_dir();
        let s = d.to_string_lossy().to_string();
        assert!(path_leads_with(&format!("{s}:/usr/bin"), &d));
        // present but losing the lookup is not active
        assert!(!path_leads_with(&format!("/usr/bin:{s}"), &d));
        assert!(!path_leads_with("/usr/bin:/bin", &d));
    }

    #[test]
    fn every_shimmable_tool_is_a_bare_command_name() {
        // A shim is reached through PATH, so a name containing a separator could never
        // be intercepted — those belong in `UNSHIMMABLE`.
        for t in crate::filter::FILTERED_TOOLS {
            assert!(!t.contains('/'), "{t} is a path, not a PATH lookup");
            assert!(!t.is_empty());
        }
    }

    #[test]
    fn a_generated_shim_is_executable_sh_that_calls_prism_cmd() {
        let s = shim_script(Path::new("/usr/local/bin/prism"), "cargo");
        assert!(s.starts_with("#!/bin/sh\n"));
        assert!(s.contains(MARKER));
        assert!(
            s.contains(r#"exec "/usr/local/bin/prism" cmd cargo "$@""#),
            "{s}"
        );
        // no baked-in tool path: resolution must stay dynamic for nvm/rbenv/pyenv
        assert!(!s.contains("/usr/bin/cargo"), "{s}");
    }

    #[test]
    fn uninstall_ignores_files_it_did_not_write() {
        let dir = std::env::temp_dir().join(format!("prism-shim-t-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mine = dir.join("git");
        std::fs::write(&mine, format!("#!/bin/sh\n{MARKER} for `git`\n")).unwrap();
        let theirs = dir.join("please-keep");
        std::fs::write(&theirs, "#!/bin/sh\necho hand written\n").unwrap();
        // uninstall() works on shim_dir(); exercise the same predicate directly
        let ours = |p: &Path| {
            std::fs::read_to_string(p)
                .map(|s| s.contains(MARKER))
                .unwrap_or(false)
        };
        assert!(ours(&mine));
        assert!(!ours(&theirs));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
