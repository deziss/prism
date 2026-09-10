//! Shell registration for PRISM's PATH shims.
//!
//! The previous version of this module defined a `PRISM_HOOK()` shell function that
//! nothing ever called, dispatched to `prism <tool>` (a subcommand that does not exist —
//! it is `prism cmd <tool>`), and exported `PRISM_DATA_DIR` pointing at the hooks
//! directory, which relocates prism's entire store. It is replaced by a single `PATH`
//! entry, because `PATH` is the one interception point every client already respects.
//!
//! See [`crate::shim`] for the mechanism.

use anyhow::{Context, Result};
use std::path::PathBuf;

const BEGIN: &str = "# >>> PRISM shims >>>";
const END: &str = "# <<< PRISM shims <<<";

/// Shells we can write a `PATH` line for, and the rc file each reads.
fn rc_files(global: bool) -> Result<Vec<PathBuf>> {
    if !global {
        return Ok(vec![std::env::current_dir()?.join(".prismrc")]);
    }
    let home = dirs::home_dir().context("no home dir")?;
    // Write to every rc that already exists rather than guessing from $SHELL: a user who
    // runs bash in a terminal and zsh in an IDE needs both, and $SHELL only names one.
    let mut found: Vec<PathBuf> = [".bashrc", ".zshrc"]
        .iter()
        .map(|f| home.join(f))
        .filter(|p| p.exists())
        .collect();
    let fish = home.join(".config/fish/config.fish");
    if fish.exists() {
        found.push(fish);
    }
    if found.is_empty() {
        found.push(home.join(".bashrc"));
    }
    Ok(found)
}

fn is_fish(p: &std::path::Path) -> bool {
    p.extension().and_then(|e| e.to_str()) == Some("fish")
}

fn block_for(p: &std::path::Path) -> String {
    let dir = crate::shim::shim_dir();
    let line = if is_fish(p) {
        format!("fish_add_path -p {}", shell_quote(&dir.to_string_lossy()))
    } else {
        format!("export PATH=\"{}:$PATH\"", dir.display())
    };
    format!("{BEGIN}\n{line}\n{END}\n")
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Install the shims and put their directory on `PATH`.
pub async fn install(global: bool) -> Result<()> {
    let report = crate::shim::install()?;
    if let Some(vol) = &report.volatile_binary {
        eprintln!();
        eprintln!("  WARNING: these shims point at a build directory:");
        eprintln!("    {}", vol.display());
        eprintln!("  `cargo clean`, or moving this repo, will break every shimmed command");
        eprintln!("  (git, ls, find, cargo, …) for this user. Install the binary to a stable");
        eprintln!("  location first, then re-run from there:");
        eprintln!(
            "    cp {} ~/.local/bin/prism && ~/.local/bin/prism shim install --path",
            vol.display()
        );
        eprintln!();
    }
    for rc in rc_files(global)? {
        let existing = std::fs::read_to_string(&rc).unwrap_or_default();
        let stripped = strip_block(&existing);
        let updated = format!("{}{}", ensure_trailing_newline(&stripped), block_for(&rc));
        std::fs::write(&rc, updated).with_context(|| format!("writing {}", rc.display()))?;
        println!("  PATH:         {}", rc.display());
    }
    println!(
        "  Shims:        {} in {}",
        report.written.len(),
        report.dir.display()
    );
    println!("  (open a new shell, or `source` the rc, for it to take effect)");
    Ok(())
}

/// Remove the `PATH` block and every generated shim.
pub async fn uninstall(global: bool) -> Result<()> {
    for rc in rc_files(global)? {
        if let Ok(existing) = std::fs::read_to_string(&rc) {
            let stripped = strip_block(&existing);
            if stripped != existing {
                std::fs::write(&rc, stripped)?;
                println!("  cleaned {}", rc.display());
            }
        }
    }
    let n = crate::shim::uninstall()?;
    println!("  removed {n} shims");
    Ok(())
}

/// Drop the marked block, and also any line from the pre-marker versions of this module,
/// so upgrading does not leave a dead `source .../prism_hook.sh` behind.
fn strip_block(content: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut inside = false;
    for line in content.lines() {
        let t = line.trim();
        if t == BEGIN {
            inside = true;
            continue;
        }
        if t == END {
            inside = false;
            continue;
        }
        if inside {
            continue;
        }
        // legacy: `# PRISM hook (auto-generated)` followed by a `. ".../prism_hook.sh"`
        if t.starts_with("# PRISM hook") || t.starts_with("# PRISM local hook") {
            continue;
        }
        if t.contains("prism_hook.sh") {
            continue;
        }
        out.push(line);
    }
    let mut s = out.join("\n");
    if content.ends_with('\n') && !s.is_empty() {
        s.push('\n');
    }
    s
}

fn ensure_trailing_newline(s: &str) -> String {
    if s.is_empty() || s.ends_with('\n') {
        s.to_string()
    } else {
        format!("{s}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_round_trips_without_accumulating() {
        let rc = std::path::Path::new("/home/u/.bashrc");
        let base = "export EDITOR=vim\nalias ll='ls -la'\n";
        let once = format!("{}{}", base, block_for(rc));
        // installing twice must not leave two blocks
        let twice = format!(
            "{}{}",
            ensure_trailing_newline(&strip_block(&once)),
            block_for(rc)
        );
        assert_eq!(once, twice);
        // and removing gets us exactly back
        assert_eq!(strip_block(&once), base);
    }

    #[test]
    fn the_broken_legacy_hook_is_cleaned_up_on_upgrade() {
        // What the previous version wrote: a comment plus a source line, where
        // `uninstall` removed only the comment and left the source line behind.
        let legacy = "export EDITOR=vim\n\
                      # PRISM hook (auto-generated)\n\
                      . \"/home/u/.local/share/prism/hooks/prism_hook.sh\"\n\
                      alias ll='ls -la'\n";
        let cleaned = strip_block(legacy);
        assert!(!cleaned.contains("prism_hook.sh"), "{cleaned}");
        assert!(!cleaned.contains("PRISM hook"), "{cleaned}");
        assert!(cleaned.contains("export EDITOR=vim"));
        assert!(cleaned.contains("alias ll"));
    }

    #[test]
    fn fish_gets_fish_syntax() {
        let f = block_for(std::path::Path::new("/home/u/.config/fish/config.fish"));
        assert!(f.contains("fish_add_path -p "), "{f}");
        assert!(!f.contains("export PATH="), "{f}");
        let b = block_for(std::path::Path::new("/home/u/.bashrc"));
        assert!(b.contains("export PATH=\""), "{b}");
    }

    #[test]
    fn no_data_dir_is_exported() {
        // The old module exported PRISM_DATA_DIR pointing at the hooks directory, which
        // relocates the whole store now that the variable is honoured.
        let b = block_for(std::path::Path::new("/home/u/.bashrc"));
        assert!(!b.contains("PRISM_DATA_DIR"), "{b}");
    }

    #[test]
    fn stripping_an_untouched_rc_changes_nothing() {
        let rc = "export EDITOR=vim\n# my prism notes\n";
        assert_eq!(strip_block(rc), rc);
    }
}
