//! Every install step must have a matching uninstall step.
//!
//! # What this guards against
//!
//! PRISM's removal was broken for months, and not by a coding error. It was
//! broken because the project had three separate "off" switches — `remove-prism.sh`,
//! `scripts/prism-disable`, and cleanup inside `prism init` — and each one knew
//! about a different subset of what "on" had created. Somebody would add an
//! install step, update one of the three, and the other two would silently stop
//! being complete. Every removal still reported success.
//!
//! What survived that way: `prism-mcp.service` (while the remover chased a
//! `prism-bridge.service` nothing creates), NSS trust entries, an MCP registration
//! pointing at a deleted binary, an IDE left with TLS verification disabled, and a
//! self-signed interception root in the system trust store whose private key the
//! uninstaller had deleted.
//!
//! So this is a process guard, not a logic test. It scans the install side for
//! anything that writes outside PRISM's own source tree, and fails the build if
//! the target is not declared in `scripts/prism-manifest.sh` — the single list
//! that all three removal paths read. Adding an install step without an uninstall
//! step now breaks CI instead of shipping.
//!
//! Adding a legitimately new host write is a two-line change: declare it in the
//! manifest, and add it to `HOST_WRITES` below.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A thing PRISM can write outside its own tree.
///
/// `needle` is what appears at the install site; `manifest_token` is the
/// declaration that must exist in `scripts/prism-manifest.sh` for removal to know
/// about it.
struct HostWrite {
    needle: &'static str,
    manifest_token: &'static str,
    what: &'static str,
}

const HOST_WRITES: &[HostWrite] = &[
    HostWrite {
        needle: "/usr/local/share/ca-certificates",
        manifest_token: "PRISM_SYSTEM_CA_LINUX",
        what: "the Linux system-wide trust anchor",
    },
    HostWrite {
        needle: "System.keychain",
        manifest_token: "PRISM_SYSTEM_CA_MACOS_KEYCHAIN",
        what: "the macOS system keychain trust anchor",
    },
    HostWrite {
        needle: "certutil",
        manifest_token: "PRISM_NSS_NICKNAME",
        what: "NSS trust for Electron apps and Firefox",
    },
    HostWrite {
        needle: ".bashrc",
        manifest_token: "PRISM_RC_FILES",
        what: "shell startup files",
    },
    HostWrite {
        needle: "config.fish",
        manifest_token: "PRISM_FISH_CONFIG",
        what: "the fish shell config",
    },
    HostWrite {
        needle: ".claude.json",
        manifest_token: "PRISM_CLAUDE_CONFIG",
        what: "the Claude Code MCP registration",
    },
    HostWrite {
        needle: ".claude/settings.json",
        manifest_token: "PRISM_CLAUDE_SETTINGS",
        what: "Claude Code settings",
    },
    HostWrite {
        needle: "systemd/user",
        manifest_token: "PRISM_SYSTEMD_USER_DIR",
        what: "systemd user units",
    },
    HostWrite {
        needle: "environment.d",
        manifest_token: "PRISM_LEGACY_ENV_FILE",
        what: "the login-session environment file",
    },
    HostWrite {
        needle: "shims",
        manifest_token: "PRISM_SHIM_DIR",
        what: "the PATH shim directory",
    },
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Source files that represent the *install* side. The removal scripts and the
/// manifest are excluded: they mention every needle by definition, so including
/// them would make this test vacuous.
fn install_side_files() -> Vec<PathBuf> {
    let root = repo_root();
    let mut files = Vec::new();

    let mut stack = vec![root.join("src")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                files.push(path);
            }
        }
    }

    for script in ["scripts/prism-enable", "scripts/install.sh"] {
        let path = root.join(script);
        if path.is_file() {
            files.push(path);
        }
    }

    files.sort();
    files
}

fn manifest() -> String {
    read(&repo_root().join("scripts/prism-manifest.sh"))
}

/// True when the manifest actually *declares* `token`, not merely mentions it.
///
/// A plain substring search is not enough: it matches a longer name that happens
/// to start with the token (`PRISM_FISH_CONFIG` inside `PRISM_FISH_CONFIG_OLD`),
/// and it matches prose in the manifest's own comments — either of which would
/// let a removed declaration pass unnoticed, making this whole test decorative.
fn manifest_declares(manifest: &str, token: &str) -> bool {
    manifest.lines().any(|line| {
        let line = line.trim_start();
        line.strip_prefix(token)
            .is_some_and(|rest| rest.starts_with('=') || rest.starts_with("=("))
    })
}

/// The core guard: anything the install side touches must be declared for removal.
#[test]
fn every_host_write_is_declared_in_the_manifest() {
    let manifest = manifest();
    let files = install_side_files();
    assert!(
        files.len() > 5,
        "install-side file discovery found only {} files — the scan is broken, \
         and a broken scan passes everything",
        files.len()
    );

    let mut failures = Vec::new();

    for write in HOST_WRITES {
        let mut sites = BTreeSet::new();
        for path in &files {
            let body = read(path);
            for (lineno, line) in body.lines().enumerate() {
                // Skip comments: this file and the sources deliberately *discuss*
                // these paths in prose, and a mention in a comment is not a write.
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") || trimmed.starts_with('#') {
                    continue;
                }
                if line.contains(write.needle) {
                    sites.insert(format!(
                        "{}:{}",
                        path.strip_prefix(repo_root()).unwrap_or(path).display(),
                        lineno + 1
                    ));
                }
            }
        }

        if sites.is_empty() {
            continue;
        }

        if !manifest_declares(&manifest, write.manifest_token) {
            failures.push(format!(
                "\n  {} ({})\n    written at: {}\n    but scripts/prism-manifest.sh declares no `{}`,\n    so no removal path knows to clean it up.",
                write.needle,
                write.what,
                sites.iter().cloned().collect::<Vec<_>>().join(", "),
                write.manifest_token
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "install steps with no matching uninstall step:{}\n\n\
         Declare each one in scripts/prism-manifest.sh. All three removal paths \
         read that file, so a declaration there is what makes the write removable.\n",
        failures.join("")
    );
}

/// Every removal path must read the manifest rather than carrying its own list.
///
/// Private copies are how the three off-switches drifted apart. A hardcoded
/// `$HOME/...` path in a remover is the shape of that bug.
#[test]
fn removal_scripts_source_the_manifest() {
    for script in [
        "remove-prism.sh",
        "scripts/prism-disable",
        "scripts/prism-enable",
    ] {
        let body = read(&repo_root().join(script));
        assert!(
            body.contains("prism-manifest.sh"),
            "{script} does not source scripts/prism-manifest.sh, so it carries its \
             own idea of what PRISM installed — exactly how prism-mcp.service and \
             the NSS trust entries survived every removal"
        );
    }
}

/// The remover must not hardcode a single IDE profile path.
///
/// It used to target the literal `~/.config/Antigravity IDE/User/settings.json`.
/// The machine had three Antigravity profile directories; the script cleaned one,
/// left two fully configured to proxy through a daemon that no longer existed,
/// and printed "OK: Antigravity settings are clean".
#[test]
fn remover_discovers_ide_profiles_rather_than_naming_one() {
    let body = read(&repo_root().join("remove-prism.sh"));
    assert!(
        !body.contains("Antigravity IDE/User/settings.json"),
        "remove-prism.sh hardcodes one Antigravity profile path; profiles must be \
         discovered via prism_ide_settings_files() so sibling profile directories \
         are not silently skipped"
    );
    assert!(
        body.contains("prism_ide_settings_files"),
        "remove-prism.sh must enumerate IDE settings through the manifest helper"
    );
}

/// Removal must be opt-in, and the audit must be able to report a non-clean machine.
#[test]
fn audit_is_the_default_and_reports_failure() {
    let body = read(&repo_root().join("remove-prism.sh"));
    assert!(
        body.contains(r#"MODE="audit""#),
        "remove-prism.sh must default to a read-only audit: it is run repeatedly to \
         answer 'is it actually gone?', and that question must be safe to ask"
    );
    assert!(
        body.contains("exit 1"),
        "the audit must exit non-zero while traces remain, or CI and scripts cannot \
         tell a clean machine from a dirty one"
    );
}

/// The detector for already-running processes must exist.
///
/// This is the finding no uninstaller can act on and the reason removal appeared
/// to fail for months: environment is copied into a process at exec time, so a
/// poisoned login session keeps handing dead variables to everything it spawns.
/// Naming those processes is the only remedy available, so it must not be dropped.
#[test]
fn remover_detects_processes_holding_stale_environment() {
    let body = read(&repo_root().join("remove-prism.sh"));
    assert!(
        body.contains("/proc/$pid/environ") || body.contains("/proc/\\$pid/environ"),
        "remove-prism.sh must scan /proc for processes still carrying PRISM \
         environment variables"
    );
    assert!(
        body.contains("LOGIN SESSION"),
        "the detector must call out a poisoned login session specifically — \
         restarting individual apps cannot fix that case, and users need to be told"
    );
}

/// `prism serve` must not default to a wildcard bind.
#[test]
fn proxy_binds_loopback_by_default() {
    let main = read(&repo_root().join("src/main.rs"));
    assert!(
        main.contains(r#"#[arg(long, default_value = "127.0.0.1")]"#),
        "`prism serve` must expose --bind defaulting to 127.0.0.1"
    );
    let proxy = read(&repo_root().join("src/proxy.rs"));
    assert!(
        !proxy.contains("SocketAddr::from(([0, 0, 0, 0], port))"),
        "the proxy holds a CA private key and mints certificates on demand; \
         binding 0.0.0.0 unconditionally offers that to the whole network"
    );
}

/// `prism init` must not install a system-wide trust anchor.
///
/// Nothing removed it, while uninstallation deleted the CA private key — leaving
/// the machine trusting an interception root that could not be audited.
#[test]
fn init_does_not_install_a_system_trust_anchor() {
    let cli = read(&repo_root().join("src/cli.rs"));
    assert!(
        !cli.contains("proxy::install_ca_system("),
        "`prism init` must not write to the OS trust store: nothing removes it, \
         and it outlives the private key that uninstallation deletes"
    );
}

/// `prism shim` must not advise setting env.PATH in Claude Code's settings.
///
/// Claude Code does not expand `${PATH}` there. Following that advice leaves the
/// agent with the shims directory plus a literal string on PATH, so once the
/// shims are deleted every command becomes "command not found" — and no
/// uninstaller looked at that file.
#[test]
fn shim_help_does_not_recommend_claude_settings_path() {
    // Match the shape of the recommendation -- a JSON literal assigning env.PATH --
    // rather than any mention of the file. The help text now warns against doing
    // this, and a warning must not trip the guard that exists to prevent the advice.
    let cli = read(&repo_root().join("src/cli.rs"));
    for (lineno, line) in cli.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        let recommends_json = line.contains(".claude/settings.json")
            && line.contains("\\\"env\\\"")
            && line.contains("PATH");
        assert!(
            !recommends_json,
            "src/cli.rs:{} still prints an env.PATH snippet for ~/.claude/settings.json; \
             ${{PATH}} is not expanded there, so following it leaves the agent with no \
             working PATH once the shims directory is removed",
            lineno + 1
        );
    }

    // And the correction must actually be present, so this does not silently pass
    // by the advice simply having been deleted along with the explanation.
    assert!(
        cli.contains("is not expanded there"),
        "src/cli.rs should explain why env.PATH in ~/.claude/settings.json is wrong, \
         not just omit the suggestion"
    );
}
