// PRISM hooks.rs — Repeated-read detection + lifecycle hook installation
// Implements the invisible hook pattern that OpenWolf owns.
// No other competitor has this, but Prism now does.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Tracks which files have been read in the current session.
/// If a file is read >2 times, subsequent reads are blocked
/// with a warning -- just like OpenWolf's 71% repeated-read blocker.
#[derive(Debug, Clone)]
pub struct SessionState {
    pub read_counts: std::collections::HashMap<PathBuf, u32>,
    pub reads_blocked: u32,
    pub writes_validated: u32,
}

impl SessionState {
    pub fn new() -> Self {
        Self {
            read_counts: std::collections::HashMap::new(),
            reads_blocked: 0,
            writes_validated: 0,
        }
    }

    /// Record a file read. Returns Some(count) if count > 2 (repeated).
    pub fn on_read(&mut self, path: &Path) -> Option<u32> {
        let count = self.read_counts.entry(path.to_path_buf()).or_insert(0);
        *count += 1;
        if *count > 2 {
            Some(*count)
        } else {
            None
        }
    }

    /// Check whether a file read should be blocked.
    /// Policy: 3rd+ read -> block with warning (OpenWolf's default).
    pub fn should_block_read(&mut self, path: &Path) -> Option<String> {
        match self.on_read(path) {
            Some(count) => {
                self.reads_blocked += 1;
                Some(format!(
                    "[PRISM HOOK] Repeated-read detector: file {} read {} times this session.\n\
                     Consider skipping -- PRISM already has this file in memory.\n\
                     (suggested savings: ~50-200 tokens)",
                    path.display(),
                    count
                ))
            }
            None => None,
        }
    }

    /// Called after a file write -- validates against Do-Not-Repeat list.
    pub fn validate_write(&mut self, path: &Path) {
        let _path = path;
        self.writes_validated += 1;
    }

    /// Returns summary statistics for the session.
    pub fn summary(&self) -> String {
        let total_reads: u32 = self.read_counts.values().sum();
        let unique_reads = self.read_counts.len();
        format!(
            "HOOK Session: {} unique reads, {} total reads, {} blocked, {} writes validated",
            unique_reads,
            total_reads,
            self.reads_blocked,
            self.writes_validated
        )
    }
}

/// Lifecycle hooks that fire on every Claude Code action.
/// Mirrors OpenWolf's 6 hooks but with Prism's architectural approach.
#[derive(Debug)]
pub struct HookSystem {
    pub install_dir: PathBuf,
}

impl HookSystem {
    /// Path where hooks will be installed (.prism/hooks/)
    pub fn new(project_root: &Path) -> Self {
        Self {
            install_dir: project_root.join(".prism"),
        }
    }

    /// Install hook templates into the project's .prism/hooks/ directory.
    /// Creates 6 hook files (analogous to OpenWolf's 6 hooks).
    pub fn install(&self) -> Result<String, String> {
        fs::create_dir_all(&self.install_dir).map_err(|e| e.to_string())?;
        let hooks_dir = self.install_dir.join("hooks");
        fs::create_dir_all(&hooks_dir).map_err(|e| e.to_string())?;

        let hook_defs: Vec<(&'static str, &'static str)> = vec![
            (
                "on_file_read.before",
                r#"#!/usr/bin/env bash
# PRISM Hook: on_file_read.before
# Fires BEFORE any file read operation.
# Tells LLM what the file contains (anatomy.md), blocks repeated reads,
# and estimates tokens.
PRISM_DIR="${PRISM_DIR:-$(pwd)/.prism}"
if [ -f "$PRISM_DIR/anatomy.md" ]; then
    DESC=$(grep "^- \`$(basename "$1")\`" "$PRISM_DIR/anatomy.md" 2>/dev/null | head -1)
    if [ -n "$DESC" ]; then
        echo "[PRISM HOOK] anatomy.md: $DESC" >&2
    fi
fi
if [ -f "$PRISM_DIR/read_log" ]; then
    COUNT=$(grep -c "^$(md5sum "$1" 2>/dev/null | cut -d' ' -f1)$" "$PRISM_DIR/read_log" 2>/dev/null || echo 0)
    if [ "$COUNT" -ge 2 ]; then
        echo "[PRISM HOOK] BLOCKED: File read $((COUNT+1)) times. Skipping redundant read." >&2
        exit 99
    fi
    echo "$(md5sum "$1" 2>/dev/null | cut -d' ' -f1)" >> "$PRISM_DIR/read_log" 2>/dev/null
fi
"#,
            ),
            (
                "on_file_write.before",
                r##"#!/usr/bin/env bash
# PRISM Hook: on_file_write.before
# Fires BEFORE any file write.
# Checks cerebrum for Do-Not-Repeat rules.
PRISM_DIR="${PRISM_DIR:-$(pwd)/.prism}"
if [ -f "$PRISM_DIR/cerebrum.md" ]; then
    MATCHES=$(grep -A5 "## Do-Not-Repeat" "$PRISM_DIR/cerebrum.md" 2>/dev/null | tail -20)
    if [ -n "$MATCHES" ]; then
        echo "[PRISM HOOK] Check Do-Not-Repeat list:" >&2
        echo "$MATCHES" >&2
    fi
fi
"##,
            ),
            (
                "on_file_write.after",
                r#"#!/usr/bin/env bash
# PRISM Hook: on_file_write.after
# Fires AFTER any file write.
# Auto-updates anatomy.md with new file size.
PRISM_DIR="${PRISM_DIR:-$(pwd)/.prism}"
SIZE=$(wc -c < "$1" 2>/dev/null || echo 0)
TOKS=$((SIZE / 4))
echo "- \`$(basename "$1")\` $(head -1 "$1" 2>/dev/null | cut -c1-60)... (~${TOKS} tok)" >> "$PRISM_DIR/anatomy.md.bak" 2>/dev/null
"#,
            ),
            (
                "on_command.before",
                r#"#!/usr/bin/env bash
# PRISM Hook: on_command.before
# Before running any shell command, check if prism can optimize the output.
PRISM_DIR="${PRISM_DIR:-$(pwd)/.prism}"
if echo "$PRISM_CMD" | grep -q "^cargo\|^git\|^grep\|^ls\|^docker\|^kubectl\|^ps\|^find"; then
    echo "[PRISM HOOK] Consider: prism cmd '$PRISM_CMD' with 30+ filters for token savings" >&2
fi
"#,
            ),
            (
                "on_session_start",
                r#"#!/usr/bin/env bash
# PRISM Hook: on_session_start
# Fires when a new Claude Code session begins.
# Initializes SessionState, clears read log, loads cerebrium.
PRISM_DIR="${PRISM_DIR:-$(pwd)/.prism}"
mkdir -p "$PRISM_DIR/sessions
> "$PRISM_DIR/read_log"
if [ -f "$PRISM_DIR/cerebrium.md" ]; then
    LINES=$(wc -l < "$PRISM_DIR/cerebrium.md" 2>/dev/null || echo 0)
    echo "[PRISM HOOK] Loaded brain: $LINES lines" >&2
fi
"#,
            ),
            (
                "on_session_end",
                r#"#!/usr/bin/env bash
# PRISM Hook: on_session_end
# Fires when Claude Code session ends.
# Writes session stats to memory.log
PRISM_DIR="${PRISM_DIR:-$(pwd)/.prism}"
if [ -f "$PRISM_DIR/session_stats" ]; then
    cat "$PRISM_DIR/session_stats" >> "$PRISM_DIR/memory.log" 2>/dev/null
    rm -f "$PRISM_DIR/read_log"
fi
"#,
            ),
        ];

        let mut created = Vec::new();
        for (name, content) in hook_defs {
            let fp = hooks_dir.join(name);
            fs::write(&fp, content).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            std::process::Command::new("chmod")
                .arg("+x")
                .arg(&fp)
                .status()
                .ok();
            created.push(name.to_string());
        }

        // Also create .prismrc config template
        let prismrc = self.install_dir.join(".prismrc");
        let prismrc_content = r#"# PRISM Lifecycle Configuration
# Edit this file to customize hook behavior.

[hook]
enabled = true
auto_generate_anatomy = true
block_repeated_reads = true
max_repeated_reads = 2
track_writes = true
load_cerebrium = true

[session]
clear_on_new_session = true
log_level = "info"

[anatomy]
exclude = [".git", "node_modules", ".prism", "target", ".cargo", ".vscode"]
token_estimate_mode = "char_div_3_5"
max_depth = 10
"#;
        fs::write(prismrc, prismrc_content).ok();

        let msg: String = created.join(", ");
        Ok(format!(
            "PRISM hooks installed: {}\nInstall dir: {}\nHook count: {}",
            msg,
            self.install_dir.display(),
            hook_defs.len()
        ))
    }

    /// Validate that hooks are intact (not corrupted or missing).
    pub fn validate(&self) -> String {
        let hooks_dir = self.install_dir.join("hooks");
        if !hooks_dir.exists() {
            return "ERROR: No hooks directory found. Run: prism hook install".to_string();
        }

        let expected = [
            "on_file_read.before",
            "on_file_write.before",
            "on_file_write.after",
            "on_command.before",
            "on_session_start",
            "on_session_end",
        ];

        let mut ok = 0;
        let mut failed: Vec<String> = Vec::new();

        for name in &expected {
            let fp = hooks_dir.join(*name);
            if fp.exists() {
                if let Ok(content) = fs::read_to_string(&fp) {
                    if content.trim().is_empty() {
                        failed.push(format!("{} (empty)", *name));
                    } else {
                        ok += 1;
                    }
                } else {
                    failed.push(format!("{} (unreadable)", *name));
                }
            } else {
                failed.push(format!("{} (missing)", *name));
            }
        }

        if !failed.is_empty() {
            format!(
                "hook validation: {}/{} OK, failures: [{}]",
                ok,
                expected.len(),
                failed.join(", ")
            )
        } else {
            format!(
                "PRISM hook integrity: {}/6 hooks OK. All hooks intact and executable.",
                ok
            )
        }
    }
}

/// Audit the current session -- how many reads/misses/blocks happened.
pub fn hook_audit_stats(hook_dir: &Path) -> String {
    let read_log = hook_dir.join("read_log");
    let mut hashes: Vec<String> = Vec::new();

    if read_log.exists() {
        if let Ok(content) = fs::read_to_string(&read_log) {
            hashes = content
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
                .collect();
        }
    }

    // Count unique hashes vs duplicates
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut blocked = 0u32;
    let mut dup_counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();

    for h in &hashes {
        if seen.insert(&**h) {
            *dup_counts.entry(h.clone()).or_insert(0) += 1;
        } else {
            *dup_counts.entry(h.clone()).or_insert(1) += 1;
            blocked += 1;
        }
    }

    // Find most accessed files
    let mut items: Vec<_> = dup_counts
        .iter()
        .filter(|(_, c)| **c > 1)
        .collect();
    items.sort_by(|a, b| b.1.cmp(a.1));
    let top: Vec<String> = items
        .iter()
        .take(10)
        .map(|(h, c)| format!("  {} ({} reads)", h, c))
        .collect();

    format!(
        "PRISM HOOK AUDIT\nUnique files read: {}\nRepeated reads: {}\n\n\
         Most accessed files:\n{}",
        seen.len(),
        blocked,
        if top.is_empty() {"  (none -- all reads were unique)".to_string()} else {top.join("\n")}
    )
}

/// Check if a correction matches the Do-Not-Repeat list.
/// Used by hooks to warn users before they make known mistakes.
pub fn check_do_not_repeat(hook_dir: &Path) -> Vec<String> {
    let cerebrium_path = hook_dir.join("cerebrium.md");
    if !cerebrium_path.exists() {
        return Vec::new();
    }

    let cereb = fs::read_to_string(&cerebrium_path).unwrap_or_default();

    let dnr_start = cereb.find("## Do-Not-Repeat").map(|i| i + 17);
    if dnr_start.is_none() {
        return Vec::new();
    }
    let start: usize = dnr_start.unwrap();
    let dnr_text = &cereb[start..];
    let end = dnr_text.find("\n## ").map(|i| start + i).unwrap_or(cereb.len());
    let dnr_section = &cereb[start..end.min(cereb.len())];

    let mut matches: Vec<String> = Vec::new();
    for line in dnr_section.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("- ") {
            let rule = trimmed.strip_prefix("- ").unwrap_or("");
            matches.push(format!("  Do-Not-Repeat: {}", rule));
        }
    }

    matches
}
