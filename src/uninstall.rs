//! `prism uninstall` — audit or remove every trace of PRISM from this machine.
//!
//! # Why this is a thin wrapper
//!
//! The actual removal logic lives in `remove-prism.sh`, driven by
//! `scripts/prism-manifest.sh`, and this module embeds both at compile time and
//! runs them. That looks indirect, so the reasoning is worth recording.
//!
//! Uninstalling has to keep working *after* the binary is gone — a half-finished
//! removal that deleted `~/.local/bin/prism` first would otherwise have no way to
//! finish. So the shell script has to be complete on its own, and it has to be
//! the authority. Reimplementing the same removal in Rust would give PRISM a
//! fourth "off" switch with its own idea of what "on" created, which is precisely
//! the bug this work exists to fix: the project already had three of those, each
//! missing something the others handled, and every one of them reported success.
//!
//! Embedding the scripts with `include_str!` means the copy `prism uninstall`
//! runs is byte-identical to the one in the repository. They cannot drift, and
//! the command works from any directory without a checkout present.

use anyhow::{Context, Result};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

/// Compiled-in copies of the removal script and its manifest. `tests/uninstall_symmetry.rs`
/// asserts that every host-mutating path in the source tree is declared in the manifest,
/// so these staying in sync with the code is enforced, not assumed.
const REMOVE_SCRIPT: &str = include_str!("../remove-prism.sh");
const MANIFEST: &str = include_str!("../scripts/prism-manifest.sh");

/// Stage the embedded scripts into a private temporary directory.
///
/// `remove-prism.sh` resolves its manifest relative to its own location, so the
/// two must land in the layout it expects: the script at the root and the
/// manifest under `scripts/`.
fn stage() -> Result<PathBuf> {
    let root = std::env::temp_dir().join(format!("prism-uninstall-{}", std::process::id()));
    let scripts = root.join("scripts");
    std::fs::create_dir_all(&scripts)
        .with_context(|| format!("could not create {}", scripts.display()))?;

    let script_path = root.join("remove-prism.sh");
    let mut f = std::fs::File::create(&script_path)
        .with_context(|| format!("could not write {}", script_path.display()))?;
    f.write_all(REMOVE_SCRIPT.as_bytes())?;
    f.flush()?;
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?;

    std::fs::write(scripts.join("prism-manifest.sh"), MANIFEST)?;

    Ok(script_path)
}

/// Run the audit (default) or the removal.
///
/// `remove` maps to the script's `--remove`; without it nothing on the machine is
/// changed and the exit status reports whether anything was found.
pub fn run(remove: bool, yes: bool, verbose: bool) -> Result<()> {
    // The root check lives in remove-prism.sh, not here. Enforcing it in two
    // places is how the rest of this subsystem drifted apart in the first place,
    // and the script must refuse root on its own regardless — it is the copy that
    // still works once this binary has been deleted.
    let script = stage()?;

    let mut cmd = std::process::Command::new("bash");
    cmd.arg(&script);
    if remove {
        cmd.arg("--remove");
    }
    if yes {
        cmd.arg("--yes");
    }
    if verbose {
        cmd.arg("--verbose");
    }

    let status = cmd
        .status()
        .with_context(|| format!("could not run {}", script.display()))?;

    // Best-effort: the removal may have deleted the temp dir's parent on some
    // configurations, and a stale staging directory is harmless either way.
    let _ = std::fs::remove_dir_all(script.parent().unwrap_or(&script));

    // The script exits 1 when traces remain, which is information, not a crash.
    // Propagating it lets `prism uninstall && echo clean` work as expected.
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded script must be the real one, not an empty or truncated file.
    /// A silently empty `include_str!` would make `prism uninstall` exit 0 having
    /// done nothing — the exact "reported success, changed nothing" failure this
    /// command exists to end.
    #[test]
    fn embedded_scripts_are_present_and_complete() {
        assert!(
            REMOVE_SCRIPT.contains("PRISM REMOVAL"),
            "remove-prism.sh was not embedded"
        );
        assert!(
            REMOVE_SCRIPT.contains("detect_processes"),
            "embedded remover is missing the live-process detector"
        );
        assert!(
            MANIFEST.contains("PRISM_MANIFEST_LOADED"),
            "prism-manifest.sh was not embedded"
        );
    }

    /// Staging must produce the layout `remove-prism.sh` expects: it resolves the
    /// manifest as `$SCRIPT_DIR/scripts/prism-manifest.sh`.
    #[test]
    fn staging_writes_the_layout_the_script_expects() {
        let script = stage().expect("stage");
        assert!(script.is_file());
        let manifest = script
            .parent()
            .unwrap()
            .join("scripts")
            .join("prism-manifest.sh");
        assert!(
            manifest.is_file(),
            "manifest must sit at scripts/prism-manifest.sh next to the script"
        );
        let mode = std::fs::metadata(&script).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "staged script must be executable");
        let _ = std::fs::remove_dir_all(script.parent().unwrap());
    }
}
