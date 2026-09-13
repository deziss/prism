//! PRISM reader.rs — Intelligent 7-mode file reader with AST code skeletonization,
//! session-cached re-reads (~15 tokens), git diff slice, and PathJail protection.

use anyhow::{Context, Result, anyhow};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Supported file reading modes
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ReadMode {
    /// Full file content with optional line numbers
    Full,
    /// Code skeleton: types, signatures, classes, structs (drops function bodies)
    #[default]
    Skeleton,
    /// Structural outline map of symbols, line ranges, and imports
    Map,
    /// Code with comments and redundant blank lines stripped
    Clean,
    /// Git diff for this file against HEAD
    Diff,
    /// Sliced line range (1-indexed, inclusive)
    Lines { start: usize, end: usize },
    /// Session-cached read: returns tiny receipt (~15 tokens) if file is unchanged
    Cached,
}

impl std::str::FromStr for ReadMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let lower = s.to_lowercase();
        if lower == "full" {
            Ok(ReadMode::Full)
        } else if lower == "skeleton" || lower == "signatures" || lower == "sig" {
            Ok(ReadMode::Skeleton)
        } else if lower == "map" || lower == "outline" {
            Ok(ReadMode::Map)
        } else if lower == "clean" || lower == "stripped" {
            Ok(ReadMode::Clean)
        } else if lower == "diff" {
            Ok(ReadMode::Diff)
        } else if lower == "cached" || lower == "cache" {
            Ok(ReadMode::Cached)
        } else if lower.starts_with("lines:") || lower.starts_with("line:") {
            let range_str = lower
                .trim_start_matches("lines:")
                .trim_start_matches("line:");
            parse_lines_range(range_str)
        } else {
            Err(anyhow!(
                "Unknown read mode: '{}'. Valid modes: skeleton, map, clean, diff, lines:N-M, cached, full",
                s
            ))
        }
    }
}

pub fn parse_lines_range(range_str: &str) -> Result<ReadMode> {
    let parts: Vec<&str> = range_str
        .split(&['-', ':', '.'][..])
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() == 2 {
        let start = parts[0]
            .trim()
            .parse::<usize>()
            .context("Invalid start line")?;
        let end = parts[1]
            .trim()
            .parse::<usize>()
            .context("Invalid end line")?;
        Ok(ReadMode::Lines { start, end })
    } else if parts.len() == 1 {
        let line = parts[0]
            .trim()
            .parse::<usize>()
            .context("Invalid line number")?;
        Ok(ReadMode::Lines {
            start: line,
            end: line,
        })
    } else {
        Err(anyhow!(
            "Invalid line range format. Use N-M or N..M, e.g. 10-50"
        ))
    }
}

/// Output of a file read operation
#[derive(Debug, Clone)]
pub struct ReadOutput {
    pub content: String,
    pub original_tokens: usize,
    pub returned_tokens: usize,
    pub savings_pct: f64,
    pub mode_used: String,
    pub path: String,
    pub is_cached_receipt: bool,
}

// ── PathJail Security ─────────────────────────────────────────────────────────

/// Directories whose *entire contents* are credentials, relative to `$HOME`.
///
/// Filename rules alone are not enough: a private key saved as `~/.ssh/work` has no
/// telltale extension, and `~/.aws/config` names a profile that `~/.aws/credentials`
/// keys. Anything under these is refused regardless of what it is called.
const SECRET_HOME_DIRS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".kube",
    ".docker",
    ".azure",
    ".config/gcloud",
    // prism's own agent and MCP tokens live here. An agent that could read them
    // could impersonate this machine to the hub.
    ".config/prism",
];

/// Absolute paths that are credentials or leak them, whatever the caller calls them.
const SECRET_ABSOLUTE_PREFIXES: &[&str] = &[
    "/etc/shadow",
    "/etc/gshadow",
    "/etc/sudoers",
    "/etc/ssh/ssh_host_",
    "/etc/ssl/private",
    "/etc/pki/tls/private",
    "/root/",
    // `/proc/<pid>/environ` is the whole environment of another process — every
    // token it was started with, in one read.
    "/proc/",
    "/sys/",
];

/// Bare filenames that are credentials wherever they appear.
const SECRET_FILENAMES: &[&str] = &[
    ".netrc",
    "_netrc",
    ".git-credentials",
    ".npmrc",
    ".pypirc",
    ".htpasswd",
    ".pgpass",
    ".my.cnf",
    "shadow",
    "sudoers",
    "authorized_keys",
    "known_hosts",
];

/// Protect against exfiltrating secrets, private keys, and environment files.
///
/// This is the boundary between "a tool that reads your code" and "a tool that reads
/// your credentials". It matters more than it looks: `prism_read_file` is exposed over
/// MCP, so whatever is driving the agent — including text that arrived from a web page
/// or an issue tracker — chooses the path. A single successful read puts a private key
/// into a model's context, and from there into whatever that context is sent to.
///
/// Three layers, because filename matching alone leaks:
///   * **canonicalised** first, so `/tmp/../etc/shadow` and a symlink pointing at
///     `~/.ssh/id_rsa` are both resolved before any comparison;
///   * **directory** rules, for keys with arbitrary names (`~/.ssh/work`);
///   * **filename and extension** rules, for credentials that travel (`.npmrc`).
///
/// Deliberately has no escape hatch. An override flag would be the first thing a
/// prompt-injection payload reached for, and the legitimate case — a human who really
/// wants to look at their own key — is served by `cat`.
pub fn check_path_jail(path: &Path) -> Result<()> {
    // Resolve `..`, symlinks and relative paths before deciding. Without this,
    // `./foo/../../.ssh/id_rsa` reads as file name `id_rsa` but a *directory* check
    // would miss it, and a symlink named `notes.md` would bypass everything.
    //
    // `canonicalize` needs every component to exist, so it fails on a path that is
    // partly missing. Falling back to the raw path would then skip the directory
    // rules: `~/x/../.ssh/work` is lexically outside `~/.ssh` until `..` is resolved.
    // That specific shape is not exploitable — the kernel would fail the same open —
    // but the check should not depend on that coincidence, so resolve `..` and `.`
    // ourselves when the filesystem cannot.
    let resolved = path
        .canonicalize()
        .unwrap_or_else(|_| lexically_normalize(path));
    let full = resolved.to_string_lossy().to_lowercase();

    if let Some(home) = dirs::home_dir() {
        let home = home.to_string_lossy().to_lowercase();
        for dir in SECRET_HOME_DIRS {
            let secret_root = format!("{home}/{dir}");
            if full == secret_root || full.starts_with(&format!("{secret_root}/")) {
                return Err(refusal(path, &format!("~/{dir} holds credentials")));
            }
        }
    }

    for prefix in SECRET_ABSOLUTE_PREFIXES {
        if full.starts_with(prefix) {
            return Err(refusal(
                path,
                &format!("{prefix} is a system credential path"),
            ));
        }
    }

    let file_name = resolved
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    if SECRET_FILENAMES.contains(&file_name.as_str()) {
        return Err(refusal(
            path,
            "the file name marks it as a credential store",
        ));
    }

    let is_secret = file_name.starts_with(".env")
        || file_name.ends_with(".pem")
        || file_name.ends_with(".key")
        || file_name.ends_with(".p12")
        || file_name.ends_with(".pfx")
        || file_name.ends_with(".jks")
        || file_name.ends_with(".keystore")
        || file_name.ends_with(".ppk")
        || file_name.ends_with(".kdbx")
        // Terraform state embeds provider credentials and resource secrets verbatim.
        || file_name.ends_with(".tfstate")
        || file_name.ends_with(".tfstate.backup")
        || file_name.contains("id_rsa")
        || file_name.contains("id_dsa")
        || file_name.contains("id_ed25519")
        || file_name.contains("id_ecdsa")
        || file_name.contains("credentials")
        || (file_name.contains("secret")
            && (file_name.ends_with(".json")
                || file_name.ends_with(".yaml")
                || file_name.ends_with(".yml")));

    if is_secret {
        return Err(refusal(path, "the file name marks it as a secret"));
    }
    Ok(())
}

/// Resolve `.` and `..` without touching the filesystem.
///
/// Used only when `canonicalize` cannot run. Symlinks are not followed — nothing here
/// can know about them — so this is a fallback that closes the traversal hole, not a
/// replacement for the real thing.
fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                // Popping past the root is a no-op, matching the kernel.
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Extra directories refused to MCP callers, relative to `$HOME`.
///
/// Stricter than the CLI jail on purpose. A human running `prism read` chose the path
/// themselves; an MCP caller's path is chosen by whatever is driving the model, which
/// routinely includes text prism never vetted — a web page, an issue comment, a
/// dependency's README. The threat is not the user, it is instructions arriving as data.
///
/// So MCP additionally loses read access to things a coding assistant has no reason to
/// open: browser profiles (session cookies and saved passwords), shell history (which
/// is where exported tokens end up), OS keyrings, and package-manager auth files.
const MCP_EXTRA_HOME_DIRS: &[&str] = &[
    // Browser profiles: cookies, saved passwords, session tokens.
    ".mozilla",
    ".config/google-chrome",
    ".config/chromium",
    ".config/BraveSoftware",
    "snap/firefox",
    // Secret stores.
    ".password-store",
    ".local/share/keyrings",
    ".gnome2/keyrings",
    // Package and cloud tooling that keeps tokens on disk.
    ".config/gh",
    ".config/hub",
    ".m2",
    ".gradle",
    ".cargo/registry/credentials",
    ".terraform.d",
    ".vagrant.d",
    // Claude Code's own configuration holds MCP definitions and tokens.
    ".claude",
];

/// Extra filenames refused to MCP callers.
const MCP_EXTRA_FILENAMES: &[&str] = &[
    // Shell history is where `export TOKEN=...` goes to live forever.
    ".bash_history",
    ".zsh_history",
    ".sh_history",
    ".python_history",
    ".psql_history",
    ".mysql_history",
    ".claude.json",
    "auth.json",
    "credentials.tfrc.json",
    "settings.xml",
    "gradle.properties",
];

/// Extra absolute prefixes refused to MCP callers.
const MCP_EXTRA_ABSOLUTE_PREFIXES: &[&str] = &[
    "/var/log/auth.log",
    "/var/log/secure",
    "/etc/krb5.keytab",
    "/etc/machine-id",
    "/var/lib/docker",
    "/run/secrets",
    "/run/user",
];

/// The jail for MCP-driven reads: everything [`check_path_jail`] refuses, plus more.
///
/// Kept as a separate entry point rather than a flag on the base jail so the stricter
/// set is visible at the call site, and so tightening it cannot accidentally change
/// what a human typing `prism read` is allowed to do.
pub fn check_path_jail_mcp(path: &Path) -> Result<()> {
    check_path_jail(path)?;

    let resolved = path
        .canonicalize()
        .unwrap_or_else(|_| lexically_normalize(path));
    let full = resolved.to_string_lossy().to_lowercase();

    if let Some(home) = dirs::home_dir() {
        let home = home.to_string_lossy().to_lowercase();
        for dir in MCP_EXTRA_HOME_DIRS {
            let root = format!("{home}/{}", dir.to_lowercase());
            if full == root || full.starts_with(&format!("{root}/")) {
                return Err(refusal(path, &format!("~/{dir} is not readable over MCP")));
            }
        }
    }

    for prefix in MCP_EXTRA_ABSOLUTE_PREFIXES {
        if full.starts_with(prefix) {
            return Err(refusal(path, &format!("{prefix} is not readable over MCP")));
        }
    }

    let file_name = resolved
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    if MCP_EXTRA_FILENAMES.contains(&file_name.as_str()) {
        return Err(refusal(path, "this file is not readable over MCP"));
    }

    Ok(())
}

fn refusal(path: &Path, why: &str) -> anyhow::Error {
    anyhow!(
        "PathJail: refusing to read {} — {why}.\n\
         PRISM will not put credentials into a model's context. Read it yourself if you \
         genuinely need to.",
        path.display()
    )
}

// ── Session Read Cache ────────────────────────────────────────────────────────

fn session_reads_path() -> PathBuf {
    crate::prism_data_dir()
        .join("cache")
        .join("session_reads.json")
}

fn compute_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    // digest 0.11's `Array` output type dropped the blanket `LowerHex` impl the old
    // `generic-array` `GenericArray<u8, N>` had; hex-encode by hand instead.
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Checks if the file hash matches previous read in this session.
/// Returns Some(hash_preview) if identical, None if changed or new.
fn check_session_cache(path: &Path, content: &str) -> Option<String> {
    let cache_file = session_reads_path();
    let current_hash = compute_sha256(content.as_bytes());
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let key = canonical.to_string_lossy().to_string();

    let mut map: HashMap<String, String> = if cache_file.exists() {
        std::fs::read_to_string(&cache_file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        HashMap::new()
    };

    if let Some(prev_hash) = map.get(&key) {
        if *prev_hash == current_hash {
            return Some(current_hash[..8].to_string());
        }
    }

    // Update with latest hash
    map.insert(key, current_hash);
    if let Ok(json) = serde_json::to_string(&map) {
        let _ = std::fs::create_dir_all(cache_file.parent().unwrap_or(Path::new(".")));
        let _ = std::fs::write(&cache_file, json);
    }
    None
}

// ── File Reader Core ──────────────────────────────────────────────────────────

/// Read a file according to the specified mode and format options.
pub fn read_file(path: &Path, mode: ReadMode, line_numbers: bool) -> Result<ReadOutput> {
    check_path_jail(path)?;

    if !path.exists() {
        return Err(anyhow!("File not found: {}", path.display()));
    }

    let raw_bytes = std::fs::read(path).context("Failed to read file")?;
    let content = String::from_utf8_lossy(&raw_bytes).to_string();
    let total_lines = content.lines().count();
    let orig_tokens =
        crate::analytics::count_tokens(&content, "gpt-4").unwrap_or(content.len() / 4);

    let (result_text, mode_name, is_cached) = match mode {
        ReadMode::Cached => {
            if let Some(hash_prefix) = check_session_cache(path, &content) {
                let receipt = format!(
                    "[PRISM CACHED READ: {} | SHA256: {} | Unchanged ({} lines) | Cost: ~15 tokens]",
                    path.display(),
                    hash_prefix,
                    total_lines
                );
                (receipt, "cached".to_string(), true)
            } else {
                // First read or changed: return skeleton
                let skel = extract_skeleton(&content, path);
                (skel, "skeleton (cached-miss)".to_string(), false)
            }
        }
        ReadMode::Skeleton => {
            let skel = extract_skeleton(&content, path);
            (skel, "skeleton".to_string(), false)
        }
        ReadMode::Map => {
            let map = generate_symbol_map(&content, path);
            (map, "map".to_string(), false)
        }
        ReadMode::Clean => {
            let clean = clean_code(&content, path);
            (clean, "clean".to_string(), false)
        }
        ReadMode::Diff => {
            let diff = git_diff_file(path)?;
            (diff, "diff".to_string(), false)
        }
        ReadMode::Lines { start, end } => {
            let sliced = slice_lines(&content, start, end, line_numbers);
            (sliced, format!("lines:{}-{}", start, end), false)
        }
        ReadMode::Full => {
            if line_numbers {
                let numbered = add_line_numbers(&content);
                (numbered, "full".to_string(), false)
            } else {
                (content.clone(), "full".to_string(), false)
            }
        }
    };

    let returned_tokens =
        crate::analytics::count_tokens(&result_text, "gpt-4").unwrap_or(result_text.len() / 4);
    let savings_pct = if orig_tokens > 0 && returned_tokens < orig_tokens {
        (1.0 - (returned_tokens as f64 / orig_tokens as f64)) * 100.0
    } else {
        0.0
    };

    Ok(ReadOutput {
        content: result_text,
        original_tokens: orig_tokens,
        returned_tokens,
        savings_pct,
        mode_used: mode_name,
        path: path.display().to_string(),
        is_cached_receipt: is_cached,
    })
}

// ── AST & Code Skeletonization ────────────────────────────────────────────────
//
// The skeletonizers keep *every* declaration at *every* nesting depth — functions
// inside `mod tests`, methods on an `impl` nested in a module, class methods, nested
// `def`s — and drop only bodies. Depth is expressed as one leading space per level,
// which costs a single token per line.

fn extract_skeleton(content: &str, path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if name == "dockerfile" || name.starts_with("dockerfile.") {
        return skeleton_dockerfile(content);
    }
    if name == "makefile" || name == "gnumakefile" || ext == "mk" {
        return skeleton_makefile(content);
    }
    match ext.as_str() {
        "rs" => skeleton_rust(content),
        "py" | "pyi" => skeleton_python(content),
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => skeleton_typescript(content),
        "go" => skeleton_go(content),
        "md" | "markdown" => skeleton_markdown(content),
        "json" => skeleton_json(content),
        "yaml" | "yml" | "toml" => skeleton_config(content),
        "sh" | "bash" | "zsh" => skeleton_shell(content),
        _ => skeleton_generic(content),
    }
}

/// Lexer state that has to survive line boundaries: block comments and string
/// literals (a Rust `"…"` may span lines with no continuation marker at all).
#[derive(Default)]
struct Lex {
    in_comment: bool,
    in_string: Option<char>,
}

/// Net brace delta of a line, ignoring braces inside comments, strings and chars.
fn brace_delta(line: &str, lex: &mut Lex) -> i32 {
    let b: Vec<char> = line.chars().collect();
    let mut depth = 0i32;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if lex.in_comment {
            if c == '*' && b.get(i + 1) == Some(&'/') {
                lex.in_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if let Some(quote) = lex.in_string {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == quote {
                lex.in_string = None;
            }
            i += 1;
            continue;
        }
        match c {
            '/' if b.get(i + 1) == Some(&'/') => break, // line comment
            '/' if b.get(i + 1) == Some(&'*') => {
                lex.in_comment = true;
                i += 2;
                continue;
            }
            // a lone `'` is a lifetime (`&'a str`), not a char literal — mistaking it
            // for one swallows the rest of the line and desyncs brace depth
            '\'' if !(b.get(i + 1) == Some(&'\\') || b.get(i + 2) == Some(&'\'')) => {
                i += 1;
                continue;
            }
            '"' | '\'' | '`' => {
                lex.in_string = Some(c);
                i += 1;
                continue;
            }
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    depth
}

/// Strip a leading visibility/qualifier prefix so keyword detection is simple.
fn after_qualifiers(t: &str) -> &str {
    let mut rest = t;
    loop {
        let next = rest
            .strip_prefix("pub(crate) ")
            .or_else(|| rest.strip_prefix("pub(super) "))
            .or_else(|| rest.strip_prefix("pub(self) "))
            .or_else(|| rest.strip_prefix("pub "))
            .or_else(|| rest.strip_prefix("default "))
            .or_else(|| rest.strip_prefix("const "))
            .or_else(|| rest.strip_prefix("async "))
            .or_else(|| rest.strip_prefix("unsafe "))
            .or_else(|| rest.strip_prefix("extern \"C\" "))
            .or_else(|| rest.strip_prefix("extern "));
        match next {
            Some(r) if r != rest => rest = r,
            _ => return rest,
        }
    }
}

#[derive(PartialEq)]
enum RustItem {
    Fn,
    Container, // mod / impl / trait — keep scanning inside
    Struct,
    Enum,
    Other, // type / const / static / use — single line
}

fn rust_item_kind(t: &str) -> Option<RustItem> {
    let r = after_qualifiers(t);
    let kw = r.split_whitespace().next()?;
    Some(match kw {
        "fn" => RustItem::Fn,
        "mod" | "impl" | "trait" => RustItem::Container,
        "struct" | "union" => RustItem::Struct,
        "enum" => RustItem::Enum,
        "type" | "static" | "use" | "macro_rules!" => RustItem::Other,
        _ => {
            // `const NAME: T = …` (a bare `const` qualifier was stripped above)
            if t.starts_with("const ") || t.starts_with("pub const ") {
                RustItem::Other
            } else {
                return None;
            }
        }
    })
}

/// Rust skeletonizer: module docs, imports, and every declaration at any depth.
fn skeleton_rust(content: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut pending: Vec<String> = Vec::new(); // attributes / doc comment for the next item
    let mut containers: Vec<i32> = Vec::new(); // brace depth at which each container opened
    let mut depth = 0i32;
    let mut lex = Lex::default();
    let mut skips: Vec<i32> = Vec::new(); // depths of the fn bodies we are inside
    let mut collect: Option<(RustItem, i32, Vec<String>, String)> = None; // struct/enum body
    let mut sig: Option<(RustItem, String)> = None; // multi-line signature being joined
    let mut doc_count = 0usize;

    for line in content.lines() {
        let trimmed = line.trim();
        let before = depth;
        let delta = brace_delta(line, &mut lex);
        depth = (depth + delta).max(0); // never let a mis-lexed line push depth negative

        // leave any function bodies that just closed
        while skips.last().map(|u| depth <= *u).unwrap_or(false) {
            skips.pop();
        }
        // collecting a struct/enum body
        if let Some((kind, until, mut buf, head)) = collect.take() {
            if depth <= until {
                out.push(summarize_body(&kind, &head, &buf, containers.len()));
            } else {
                if !trimmed.is_empty() && !trimmed.starts_with("//") {
                    buf.push(trimmed.to_string());
                }
                collect = Some((kind, until, buf, head));
            }
            continue;
        }
        // close finished containers
        while containers.last().map(|c| depth <= *c).unwrap_or(false) {
            containers.pop();
        }
        let indent = " ".repeat(containers.len() + skips.len());
        // Inside a function body only declarations matter — a nested `fn`/`struct`
        // is still part of the API surface, statements and closures are not.
        let in_body = !skips.is_empty();
        if in_body && sig.is_none() && rust_item_kind(trimmed).is_none() {
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }
        // module docs (first few) and item docs (first line each)
        if trimmed.starts_with("//!") {
            if doc_count < 3 {
                out.push(trimmed.to_string());
                doc_count += 1;
            }
            continue;
        }
        if trimmed.starts_with("///") || trimmed.starts_with("/**") {
            if pending.iter().all(|p| !p.starts_with("///")) {
                pending.push(format!("{}{}", indent, truncate_str(trimmed, 100)));
            }
            continue;
        }
        if trimmed.starts_with("//") {
            continue;
        }
        if trimmed.starts_with("#[") || trimmed.starts_with("#![") {
            pending.push(format!("{}{}", indent, truncate_str(trimmed, 80)));
            continue;
        }

        // continue a multi-line signature
        if let Some((kind, mut acc)) = sig.take() {
            acc.push(' ');
            acc.push_str(trimmed);
            let done = trimmed.contains('{') || trimmed.ends_with(';');
            if !done {
                sig = Some((kind, acc));
                continue;
            }
            finish_item(
                kind,
                acc,
                before,
                depth,
                &indent,
                &mut out,
                &mut pending,
                &mut containers,
                &mut skips,
                &mut collect,
            );
            continue;
        }

        match rust_item_kind(trimmed) {
            Some(kind) => {
                let done =
                    trimmed.contains('{') || trimmed.ends_with(';') || trimmed.ends_with(',');
                if !done {
                    sig = Some((kind, trimmed.to_string()));
                    continue;
                }
                finish_item(
                    kind,
                    trimmed.to_string(),
                    before,
                    depth,
                    &indent,
                    &mut out,
                    &mut pending,
                    &mut containers,
                    &mut skips,
                    &mut collect,
                );
            }
            None => {
                pending.clear();
            }
        }
    }
    if let Some((kind, _, buf, head)) = collect {
        out.push(summarize_body(&kind, &head, &buf, containers.len()));
    }
    if out.is_empty() {
        skeleton_generic(content)
    } else {
        out.join("\n")
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_item(
    kind: RustItem,
    acc: String,
    before: i32,
    depth: i32,
    indent: &str,
    out: &mut Vec<String>,
    pending: &mut Vec<String>,
    containers: &mut Vec<i32>,
    skips: &mut Vec<i32>,
    collect: &mut Option<(RustItem, i32, Vec<String>, String)>,
) {
    let opens_body = depth > before;
    let head = acc.split('{').next().unwrap_or(&acc).trim().to_string();
    out.append(pending);
    match kind {
        RustItem::Fn => {
            out.push(format!("{}{}", indent, head));
            if opens_body {
                skips.push(before);
            }
        }
        RustItem::Container => {
            out.push(format!("{}{}", indent, head));
            if opens_body {
                containers.push(before);
            }
        }
        RustItem::Struct | RustItem::Enum => {
            if opens_body {
                *collect = Some((kind, before, Vec::new(), format!("{}{}", indent, head)));
            } else {
                out.push(format!("{}{}", indent, acc.trim()));
            }
        }
        RustItem::Other => out.push(format!("{}{}", indent, truncate_str(acc.trim(), 160))),
    }
}

/// `pub struct X` + its fields → one line; private fields are counted, not listed.
fn summarize_body(kind: &RustItem, head: &str, body: &[String], _depth: usize) -> String {
    let mut shown: Vec<String> = Vec::new();
    let mut hidden = 0usize;
    for raw in body {
        let item = raw.trim().trim_end_matches(',');
        if item.is_empty() || item.starts_with("#[") || item.starts_with("//") {
            continue;
        }
        match kind {
            RustItem::Struct => {
                if let Some(field) = item.strip_prefix("pub ") {
                    shown.push(field.to_string());
                } else {
                    hidden += 1;
                }
            }
            _ => {
                // enum variant: keep the name and shape marker only
                let name = item
                    .split(['(', '{', '='])
                    .next()
                    .unwrap_or(item)
                    .trim()
                    .to_string();
                if !name.is_empty() {
                    shown.push(name);
                }
            }
        }
    }
    let sep = if matches!(kind, RustItem::Struct) {
        ", "
    } else {
        " | "
    };
    let limit = 12;
    let extra = shown.len().saturating_sub(limit);
    shown.truncate(limit);
    let mut inner = shown.join(sep);
    if extra > 0 {
        inner.push_str(&format!("{}+{} more", sep, extra));
    }
    if hidden > 0 {
        if !inner.is_empty() {
            inner.push_str(", ");
        }
        inner.push_str(&format!("… {} private fields", hidden));
    }
    if inner.is_empty() {
        format!("{} {{ }}", head)
    } else {
        format!("{} {{ {} }}", head, inner)
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Python skeletonizer: imports, decorators, classes, `def`s at any depth,
/// module constants, dataclass/class attribute annotations.
fn skeleton_python(content: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut imports: Vec<String> = Vec::new();
    let mut in_doc: Option<&str> = None;
    let mut doc_owner_shown = true;
    let mut join: Option<String> = None; // multi-line def signature

    for line in content.lines() {
        let trimmed = line.trim();
        let lead = line.len() - line.trim_start().len();
        let indent = " ".repeat(lead / 2);

        if let Some(q) = in_doc {
            if trimmed.contains(q) {
                in_doc = None;
            }
            continue;
        }
        if trimmed.starts_with("\"\"\"") || trimmed.starts_with("'''") {
            let q = if trimmed.starts_with("\"\"\"") {
                "\"\"\""
            } else {
                "'''"
            };
            let body = &trimmed[3..];
            if !doc_owner_shown {
                let first = body.split(q).next().unwrap_or("").trim();
                if !first.is_empty() {
                    out.push(format!("{}\"{}\"", indent, truncate_str(first, 100)));
                }
                doc_owner_shown = true;
            }
            if !body.contains(q) {
                in_doc = Some(q);
            }
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(mut acc) = join.take() {
            acc.push(' ');
            acc.push_str(trimmed);
            if trimmed.ends_with(':') {
                out.push(format!("{}{}", indent, truncate_str(&squeeze(&acc), 200)));
                doc_owner_shown = false;
            } else {
                join = Some(acc);
            }
            continue;
        }
        if trimmed.starts_with("import ") || trimmed.starts_with("from ") {
            imports.push(trimmed.to_string());
            continue;
        }
        if trimmed.starts_with('@') {
            out.push(format!("{}{}", indent, truncate_str(trimmed, 100)));
            continue;
        }
        let is_def = trimmed.starts_with("def ")
            || trimmed.starts_with("async def ")
            || trimmed.starts_with("class ");
        if is_def {
            if trimmed.ends_with(':') {
                out.push(format!("{}{}", indent, truncate_str(trimmed, 200)));
                doc_owner_shown = false;
            } else {
                join = Some(trimmed.to_string());
            }
            continue;
        }
        if trimmed.starts_with("if __name__") {
            out.push(trimmed.to_string());
            continue;
        }
        // module constants and annotated attributes
        if let Some((lhs, rhs)) = trimmed.split_once('=') {
            let name = lhs.trim();
            let bare = name.split(':').next().unwrap_or(name).trim();
            let is_const = !bare.is_empty()
                && bare
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            if (is_const && lead == 0) || (name.contains(':') && lead > 0) {
                out.push(format!(
                    "{}{} = {}",
                    indent,
                    name,
                    truncate_str(rhs.trim(), 40)
                ));
            }
            continue;
        }
        if trimmed.contains(':') && lead > 0 && !trimmed.contains('(') && !trimmed.ends_with(':') {
            // bare annotation: `name: type`
            out.push(format!("{}{}", indent, truncate_str(trimmed, 100)));
        }
    }

    let mut head: Vec<String> = Vec::new();
    if imports.len() > 15 {
        let names: Vec<String> = imports
            .iter()
            .take(8)
            .map(|i| i.split_whitespace().nth(1).unwrap_or("").to_string())
            .collect();
        head.push(format!(
            "imports: {} lines ({}, +{} more)",
            imports.len(),
            names.join(", "),
            imports.len() - 8
        ));
    } else {
        head.extend(imports);
    }
    head.extend(out);
    if head.is_empty() {
        skeleton_generic(content)
    } else {
        head.join("\n")
    }
}

fn squeeze(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// TypeScript / JavaScript skeletonizer: imports, types with members, classes with
/// every method, top-level functions, and test titles.
fn skeleton_typescript(content: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut imports: Vec<String> = Vec::new();
    let mut containers: Vec<i32> = Vec::new(); // class / interface / enum / namespace depth
    let mut in_members = false; // inside interface/type/enum: keep member lines
    let mut member_count = 0usize;
    let mut depth = 0i32;
    let mut lex = Lex::default();
    let mut skip_to: Option<i32> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        let before = depth;
        depth = (depth + brace_delta(line, &mut lex)).max(0);

        if let Some(until) = skip_to {
            if depth <= until {
                skip_to = None;
            }
            continue;
        }
        while containers.last().map(|c| depth <= *c).unwrap_or(false) {
            containers.pop();
            in_members = false;
            member_count = 0;
        }
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with('*')
            || trimmed.starts_with("/*")
        {
            continue;
        }
        let indent = " ".repeat(containers.len());
        if trimmed.starts_with("import ") || trimmed.starts_with("export * from") {
            imports.push(squeeze(trimmed));
            continue;
        }
        if trimmed.starts_with('@') {
            out.push(format!("{}{}", indent, truncate_str(trimmed, 100)));
            continue;
        }
        let head = trimmed.split('{').next().unwrap_or(trimmed).trim();
        let is_type = head.starts_with("interface ")
            || head.starts_with("export interface ")
            || head.starts_with("type ")
            || head.starts_with("export type ")
            || head.starts_with("enum ")
            || head.starts_with("export enum ")
            || head.starts_with("const enum ");
        let is_class = head.contains("class ") && !head.contains('(');
        let is_ns = head.starts_with("namespace ") || head.starts_with("declare module ");
        let is_fn = head.starts_with("function ")
            || head.starts_with("export function ")
            || head.starts_with("async function ")
            || head.starts_with("export async function ")
            || head.starts_with("export default function");
        let is_test = head.starts_with("describe(")
            || head.starts_with("it(")
            || head.starts_with("test(")
            || head.starts_with("describe.each")
            || head.starts_with("beforeEach(")
            || head.starts_with("afterEach(");
        let is_export_const = (head.starts_with("export const ")
            || head.starts_with("export let ")
            || head.starts_with("const ")
            || head.starts_with("let "))
            && (trimmed.contains("=>") || trimmed.contains("function") || containers.is_empty());
        // a method inside a class: `name(args)` / `async name(args)` / `get name()`
        let is_method = !containers.is_empty()
            && !in_members
            && trimmed.contains('(')
            && !trimmed.starts_with("return")
            && !trimmed.starts_with("if")
            && !trimmed.starts_with("for")
            && !trimmed.starts_with("while")
            && !trimmed.starts_with("switch")
            && !trimmed.starts_with("catch")
            && !trimmed.contains(" = ")
            && (trimmed.ends_with('{') || trimmed.ends_with(';'));

        if is_type || is_class || is_ns {
            out.push(format!("{}{}", indent, truncate_str(head, 200)));
            if depth > before {
                containers.push(before);
                in_members = is_type;
                member_count = 0;
            }
            continue;
        }
        if is_fn || is_method {
            out.push(format!(
                "{}{}",
                indent,
                truncate_str(head.trim_end_matches(';'), 200)
            ));
            if depth > before {
                skip_to = Some(before);
            }
            continue;
        }
        if is_test {
            out.push(format!("{}{}", indent, truncate_str(head, 160)));
            if depth > before {
                containers.push(before);
            }
            continue;
        }
        if is_export_const && containers.is_empty() {
            out.push(truncate_str(&squeeze(trimmed), 80));
            if depth > before {
                skip_to = Some(before);
            }
            continue;
        }
        if in_members {
            // interface / enum member
            if member_count < 20 {
                out.push(format!(
                    "{}{}",
                    indent,
                    truncate_str(&squeeze(trimmed), 120)
                ));
            } else if member_count == 20 {
                out.push(format!("{}[+more members]", indent));
            }
            member_count += 1;
        }
    }

    let mut head: Vec<String> = Vec::new();
    if imports.len() > 15 {
        head.push(format!("imports: {} lines", imports.len()));
    } else {
        head.extend(imports);
    }
    head.extend(out);
    if head.is_empty() {
        skeleton_generic(content)
    } else {
        head.join("\n")
    }
}

/// Go skeletonizer: package, imports, types with exported fields, every func.
fn skeleton_go(content: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut depth = 0i32;
    let mut lex = Lex::default();
    let mut skip_to: Option<i32> = None;
    let mut group: Option<(&str, Vec<String>)> = None; // var ( … ) / const ( … )
    let mut in_type: Option<(String, Vec<String>, usize)> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        let before = depth;
        depth = (depth + brace_delta(line, &mut lex)).max(0);

        if let Some(until) = skip_to {
            if depth <= until {
                skip_to = None;
            }
            continue;
        }
        if let Some((kind, mut names)) = group.take() {
            if trimmed == ")" {
                out.push(format!("{} ( {} )", kind, names.join(", ")));
            } else {
                if !trimmed.is_empty() && !trimmed.starts_with("//") {
                    if let Some(n) = trimmed.split_whitespace().next() {
                        names.push(n.to_string());
                    }
                }
                group = Some((kind, names));
            }
            continue;
        }
        if let Some((head, mut fields, mut hidden)) = in_type.take() {
            if depth <= before && trimmed.starts_with('}') {
                let extra = fields.len().saturating_sub(12);
                fields.truncate(12);
                let mut inner = fields.join(", ");
                if extra > 0 {
                    inner.push_str(&format!(", +{} more", extra));
                }
                if hidden > 0 {
                    if !inner.is_empty() {
                        inner.push_str(", ");
                    }
                    inner.push_str(&format!("… {} unexported", hidden));
                }
                out.push(format!("{} {{ {} }}", head, inner));
            } else {
                if !trimmed.is_empty() && !trimmed.starts_with("//") {
                    let first = trimmed.split_whitespace().next().unwrap_or("");
                    if first
                        .chars()
                        .next()
                        .map(|c| c.is_uppercase())
                        .unwrap_or(false)
                    {
                        fields.push(squeeze(trimmed));
                    } else {
                        hidden += 1;
                    }
                }
                in_type = Some((head, fields, hidden));
            }
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        // grouped forms must be matched before the single-line ones, or
        // `import (` would be emitted as a bare import and its names dropped
        if trimmed == "import (" || trimmed == "var (" || trimmed == "const (" {
            let kind = trimmed.split_whitespace().next().unwrap_or("var");
            group = Some((
                match kind {
                    "import" => "import",
                    "const" => "const",
                    _ => "var",
                },
                Vec::new(),
            ));
            continue;
        }
        if trimmed.starts_with("package ") || trimmed.starts_with("import ") {
            out.push(trimmed.to_string());
            continue;
        }
        if trimmed.starts_with("type ") {
            let head = trimmed
                .split('{')
                .next()
                .unwrap_or(trimmed)
                .trim()
                .to_string();
            if depth > before {
                in_type = Some((head, Vec::new(), 0));
            } else {
                out.push(head);
            }
            continue;
        }
        if trimmed.starts_with("func ") {
            out.push(truncate_str(
                trimmed.split('{').next().unwrap_or(trimmed).trim(),
                200,
            ));
            if depth > before {
                skip_to = Some(before);
            }
            continue;
        }
        if (trimmed.starts_with("var ") || trimmed.starts_with("const ")) && depth == 0 {
            out.push(truncate_str(&squeeze(trimmed), 120));
        }
    }
    if out.is_empty() {
        skeleton_generic(content)
    } else {
        out.join("\n")
    }
}

/// Markdown skeletonizer: headings plus the first sentence of each section.
fn skeleton_markdown(content: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut want_lead = false;
    let mut fence: Option<String> = None;
    let mut fence_lines = 0usize;
    let mut table: Option<(String, usize, usize)> = None; // header, rows, cols
    let mut list_items = 0usize;

    let flush_table = |table: &mut Option<(String, usize, usize)>, out: &mut Vec<String>| {
        if let Some((header, rows, cols)) = table.take() {
            out.push(format!("[table: {} rows × {} cols] {}", rows, cols, header));
        }
    };
    let flush_list = |n: &mut usize, out: &mut Vec<String>| {
        if *n > 3 {
            out.push(format!("[+{} more list items]", *n - 3));
        }
        *n = 0;
    };

    for line in content.lines() {
        let t = line.trim();
        if let Some(f) = fence.clone() {
            if t.starts_with("```") {
                out.push(format!(
                    "[code: {}, {} lines]",
                    if f.is_empty() { "text" } else { &f },
                    fence_lines
                ));
                fence = None;
                fence_lines = 0;
            } else {
                fence_lines += 1;
            }
            continue;
        }
        if let Some(rest) = t.strip_prefix("```") {
            flush_table(&mut table, &mut out);
            flush_list(&mut list_items, &mut out);
            fence = Some(rest.trim().to_string());
            continue;
        }
        if t.starts_with('#') {
            flush_table(&mut table, &mut out);
            flush_list(&mut list_items, &mut out);
            out.push(t.to_string());
            want_lead = true;
            continue;
        }
        if t.starts_with('|') {
            let cols = t.matches('|').count().saturating_sub(1);
            match table.as_mut() {
                Some((_, rows, _)) => *rows += 1,
                None => table = Some((squeeze(t), 0, cols)),
            }
            continue;
        }
        flush_table(&mut table, &mut out);
        if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") || t.starts_with("1. ")
        {
            list_items += 1;
            if list_items <= 3 {
                out.push(truncate_str(t, 120));
            }
            continue;
        }
        flush_list(&mut list_items, &mut out);
        if t.is_empty() {
            continue;
        }
        if want_lead {
            let sentence = t.split_once(". ").map(|(a, _)| a).unwrap_or(t);
            out.push(truncate_str(sentence, 120));
            want_lead = false;
        }
    }
    flush_table(&mut table, &mut out);
    flush_list(&mut list_items, &mut out);
    if out.is_empty() {
        skeleton_generic(content)
    } else {
        out.join("\n")
    }
}

/// JSON skeletonizer: keys with value shapes instead of values.
fn skeleton_json(content: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(content) {
        Ok(v) => {
            let mut out = Vec::new();
            json_shape(&v, 0, None, &mut out);
            out.join("\n")
        }
        Err(_) => skeleton_generic(content),
    }
}

fn json_shape(v: &serde_json::Value, depth: usize, key: Option<&str>, out: &mut Vec<String>) {
    use serde_json::Value;
    let pad = " ".repeat(depth);
    let label = key.map(|k| format!("{}: ", k)).unwrap_or_default();
    match v {
        Value::Object(map) => {
            if depth >= 2 {
                let names: Vec<&str> = map.keys().take(8).map(|s| s.as_str()).collect();
                let extra = map.len().saturating_sub(names.len());
                let mut inner = names.join(", ");
                if extra > 0 {
                    inner.push_str(&format!(", +{} more", extra));
                }
                out.push(format!("{}{}{{{} keys: {}}}", pad, label, map.len(), inner));
                return;
            }
            out.push(format!("{}{}{{{} keys}}", pad, label, map.len()));
            for (k, val) in map {
                json_shape(val, depth + 1, Some(k), out);
            }
        }
        Value::Array(items) => {
            let kind = match items.first() {
                Some(Value::Object(_)) => "objects",
                Some(Value::Array(_)) => "arrays",
                Some(Value::String(_)) => "strings",
                Some(Value::Number(_)) => "numbers",
                Some(Value::Bool(_)) => "bools",
                _ => "items",
            };
            out.push(format!("{}{}[{} {}]", pad, label, items.len(), kind));
            if let Some(first) = items.first() {
                if depth < 2 && matches!(first, Value::Object(_)) {
                    json_shape(first, depth + 1, Some("[0]"), out);
                }
            }
        }
        Value::String(s) => {
            let shown = if s.chars().count() <= 30 {
                format!("\"{}\"", s)
            } else {
                format!("string({})", s.chars().count())
            };
            out.push(format!("{}{}{}", pad, label, shown));
        }
        other => out.push(format!("{}{}{}", pad, label, other)),
    }
}

/// YAML / TOML skeletonizer: keys down to depth 2, values shown only when short.
fn skeleton_config(content: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    for line in content.lines() {
        let t = line.trim_end();
        let trimmed = t.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let lead = t.len() - t.trim_start().len();
        if trimmed.starts_with('[') {
            out.push(trimmed.to_string()); // TOML table header
            continue;
        }
        if lead > 4 || trimmed.starts_with("- ") {
            skipped += 1;
            continue;
        }
        match trimmed.split_once(['=', ':']) {
            Some((k, v)) => {
                let v = v.trim().trim_matches('"');
                let shown = if v.is_empty() {
                    String::new()
                } else if v.chars().count() <= 30 {
                    format!(" {}", v)
                } else {
                    format!(" string({})", v.chars().count())
                };
                out.push(format!("{}{}:{}", " ".repeat(lead / 2), k.trim(), shown));
            }
            None => skipped += 1,
        }
    }
    if skipped > 0 {
        out.push(format!("[+{} more lines]", skipped));
    }
    if out.is_empty() {
        skeleton_generic(content)
    } else {
        out.join("\n")
    }
}

/// Shell skeletonizer: shebang, `set -e…`, functions, top-level assignments.
fn skeleton_shell(content: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    for (i, line) in content.lines().enumerate() {
        let t = line.trim();
        if i == 0 && t.starts_with("#!") {
            out.push(t.to_string());
            continue;
        }
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t.starts_with("set -") {
            out.push(t.to_string());
            continue;
        }
        let is_fn = t.ends_with("() {") || t.ends_with("()") || t.starts_with("function ");
        if is_fn {
            out.push(t.trim_end_matches('{').trim().to_string());
            continue;
        }
        if let Some((lhs, _)) = t.split_once('=') {
            if !lhs.contains(char::is_whitespace) && !lhs.is_empty() {
                out.push(format!("{}=", lhs));
                continue;
            }
        }
        skipped += 1;
    }
    if skipped > 0 {
        out.push(format!("[+{} more lines]", skipped));
    }
    out.join("\n")
}

fn skeleton_dockerfile(content: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut run_lines = 0usize;
    let mut in_run = false;
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if in_run {
            run_lines += 1;
            in_run = t.ends_with('\\');
            continue;
        }
        let kw = t.split_whitespace().next().unwrap_or("").to_uppercase();
        match kw.as_str() {
            "FROM" | "ARG" | "ENV" | "EXPOSE" | "ENTRYPOINT" | "CMD" | "WORKDIR" | "USER"
            | "VOLUME" | "HEALTHCHECK" | "LABEL" => out.push(truncate_str(t, 160)),
            "RUN" | "COPY" | "ADD" => {
                run_lines += 1;
                in_run = t.ends_with('\\');
            }
            _ => {}
        }
    }
    if run_lines > 0 {
        out.push(format!("[+{} RUN/COPY lines]", run_lines));
    }
    out.join("\n")
}

fn skeleton_makefile(content: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut recipe_lines = 0usize;
    for line in content.lines() {
        if line.starts_with('\t') {
            recipe_lines += 1;
            continue;
        }
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some((target, deps)) = t.split_once(':') {
            if !target.contains('=') && !target.is_empty() {
                let deps = deps.trim().trim_start_matches('=').trim();
                out.push(if deps.is_empty() {
                    format!("{}:", target)
                } else {
                    format!("{}: {}", target, truncate_str(deps, 100))
                });
                continue;
            }
        }
        if let Some((lhs, _)) = t.split_once('=') {
            out.push(format!("{}=", lhs.trim()));
        }
    }
    if recipe_lines > 0 {
        out.push(format!("[+{} recipe lines]", recipe_lines));
    }
    out.join("\n")
}

/// Generic skeletonizer for unknown formats: a head sample plus announced totals.
fn skeleton_generic(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let keep = 20.min(lines.len());
    let mut out: Vec<String> = lines[..keep]
        .iter()
        .map(|l| l.trim_end().to_string())
        .collect();
    if lines.len() > keep {
        out.push(format!("[+{} more lines]", lines.len() - keep));
    }
    out.push(format!("lines: {}, bytes: {}", lines.len(), content.len()));
    out.join("\n")
}

// ── Symbol Map ────────────────────────────────────────────────────────────────

/// Symbol map: every declaration with its line number, indented by nesting depth.
fn generate_symbol_map(content: &str, path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let total_lines = content.lines().count();
    let mut symbols: Vec<String> = Vec::new();
    let mut depth = 0i32;
    let mut lex = Lex::default();

    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        let before = depth;
        if matches!(
            ext.as_str(),
            "rs" | "ts" | "tsx" | "js" | "jsx" | "go" | "c" | "cpp" | "h" | "java"
        ) {
            depth += brace_delta(line, &mut lex);
        }
        let level = if ext == "py" {
            (line.len() - line.trim_start().len()) / 4
        } else {
            before.max(0) as usize
        };
        let name = if ext == "rs" {
            rust_item_kind(trimmed).and_then(|k| match k {
                RustItem::Other => None,
                _ => Some(
                    trimmed
                        .split('{')
                        .next()
                        .unwrap_or(trimmed)
                        .trim()
                        .to_string(),
                ),
            })
        } else if ext == "py" {
            if trimmed.starts_with("def ")
                || trimmed.starts_with("async def ")
                || trimmed.starts_with("class ")
            {
                Some(trimmed.trim_end_matches(':').to_string())
            } else {
                None
            }
        } else if ext == "go" {
            if trimmed.starts_with("func ") || trimmed.starts_with("type ") {
                Some(
                    trimmed
                        .split('{')
                        .next()
                        .unwrap_or(trimmed)
                        .trim()
                        .to_string(),
                )
            } else {
                None
            }
        } else if trimmed.starts_with("function ")
            || trimmed.starts_with("export function ")
            || trimmed.contains("class ") && !trimmed.contains('(')
            || trimmed.starts_with("export interface ")
            || trimmed.starts_with("interface ")
        {
            Some(
                trimmed
                    .split('{')
                    .next()
                    .unwrap_or(trimmed)
                    .trim()
                    .to_string(),
            )
        } else {
            None
        };
        if let Some(n) = name {
            symbols.push(format!(
                "  L{:<5}{}{}",
                idx + 1,
                " ".repeat(level),
                truncate_str(&n, 160)
            ));
        }
    }

    let mut out = format!("SYMBOL MAP: {} ({} lines)\n", path.display(), total_lines);
    if symbols.is_empty() {
        out.push_str("  (no top-level symbols detected)\n");
    } else {
        out.push_str(&symbols.join("\n"));
        out.push('\n');
    }
    out
}

// ── Clean Code (Comment stripping) ────────────────────────────────────────────

fn clean_code(content: &str, _path: &Path) -> String {
    let mut out = Vec::new();
    let mut in_block_comment = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if in_block_comment {
            if let Some(pos) = line.find("*/") {
                in_block_comment = false;
                let remainder = line[pos + 2..].trim();
                if !remainder.is_empty() {
                    out.push(remainder.to_string());
                }
            }
            continue;
        }

        if trimmed.starts_with("/*") {
            if !trimmed.contains("*/") {
                in_block_comment = true;
            }
            continue;
        }

        // Line comment stripping
        if trimmed.starts_with("//") || trimmed.starts_with('#') {
            continue;
        }

        if trimmed.is_empty() {
            if out.last().map(|s: &String| s.is_empty()).unwrap_or(false) {
                continue; // Avoid consecutive blank lines
            }
            out.push(String::new());
            continue;
        }

        // Inline comment removal if safe
        if let Some(idx) = line.find(" //") {
            let code_part = line[..idx].trim_end();
            if !code_part.is_empty() {
                out.push(code_part.to_string());
                continue;
            }
        }

        out.push(line.to_string());
    }

    out.join("\n")
}

// ── Git Diff & Line Slicing ───────────────────────────────────────────────────

fn git_diff_file(path: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["diff", "HEAD", "--", path.to_string_lossy().as_ref()])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let diff_str = String::from_utf8_lossy(&out.stdout).to_string();
            if diff_str.trim().is_empty() {
                Ok(format!(
                    "(No unstaged or committed changes in git for {})",
                    path.display()
                ))
            } else {
                Ok(diff_str)
            }
        }
        _ => Err(anyhow!("Failed to run git diff on {}", path.display())),
    }
}

fn slice_lines(content: &str, start: usize, end: usize, line_numbers: bool) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    let start_idx = start.saturating_sub(1).min(total);
    let end_idx = end.min(total);

    if start_idx >= end_idx {
        return format!(
            "(Line range {}-{} is outside file bounds of 1-{})",
            start, end, total
        );
    }

    let mut out = Vec::new();
    for (i, line) in lines[start_idx..end_idx].iter().enumerate() {
        let line_num = start_idx + i + 1;
        if line_numbers {
            out.push(format!("{:4} | {}", line_num, line));
        } else {
            out.push((*line).to_string());
        }
    }
    out.join("\n")
}

fn add_line_numbers(content: &str) -> String {
    content
        .lines()
        .enumerate()
        .map(|(idx, line)| format!("{:4} | {}", idx + 1, line))
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skeleton_rust_extracts_sigs() {
        let code = r#"
use std::path::PathBuf;

/// My test struct
pub struct User {
    pub name: String,
    age: u32,
}

impl User {
    pub fn new(name: String) -> Self {
        let age = 0;
        User { name, age }
    }
}
"#;
        let skel = skeleton_rust(code);
        assert!(skel.contains("pub struct User"));
        assert!(skel.contains("impl User"));
        assert!(!skel.contains("let age = 0;"));
    }

    #[test]
    fn test_path_jail_blocks_sensitive() {
        assert!(check_path_jail(Path::new(".env")).is_err());
        assert!(check_path_jail(Path::new(".env.local")).is_err());
        assert!(check_path_jail(Path::new("server.key")).is_err());
        assert!(check_path_jail(Path::new("id_rsa")).is_err());
        assert!(check_path_jail(Path::new("src/main.rs")).is_ok());
    }

    /// System credential stores, whatever they are called.
    ///
    /// The jail was filename-only, so `/etc/shadow` — no dot-prefix, no extension,
    /// not containing "credentials" — sailed straight through, as did every file in
    /// `/proc/<pid>/environ`, which is another process's entire environment.
    #[test]
    fn path_jail_blocks_system_credential_paths() {
        for p in [
            "/etc/shadow",
            "/etc/gshadow",
            "/etc/sudoers",
            "/etc/ssh/ssh_host_rsa_key",
            "/etc/ssl/private/server.pem",
            "/proc/1/environ",
            "/root/notes.txt",
        ] {
            assert!(
                check_path_jail(Path::new(p)).is_err(),
                "{p} must be refused"
            );
        }
    }

    /// A private key with an arbitrary name is still a private key.
    ///
    /// `~/.ssh/work` matches no extension rule and contains none of the magic
    /// substrings, which is exactly why the directory has to be refused wholesale.
    #[test]
    fn path_jail_blocks_everything_under_credential_directories() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        for rel in [
            ".ssh/work",
            ".ssh/config",
            ".gnupg/secring.gpg",
            ".aws/config",
            ".kube/config",
            ".docker/config.json",
            // prism's own hub credentials: agent token and MCP token.
            ".config/prism/hub.json",
        ] {
            let p = home.join(rel);
            assert!(
                check_path_jail(&p).is_err(),
                "~/{rel} must be refused even though its name looks harmless"
            );
        }
    }

    /// Credentials that travel with a project.
    #[test]
    fn path_jail_blocks_portable_credential_files() {
        for p in [
            ".netrc",
            ".git-credentials",
            ".npmrc",
            ".pypirc",
            ".pgpass",
            "vault.kdbx",
            "keys.jks",
            "terraform.tfstate",
        ] {
            assert!(
                check_path_jail(Path::new(p)).is_err(),
                "{p} must be refused"
            );
        }
    }

    /// Traversal and symlinks must not defeat the directory rules.
    ///
    /// Without canonicalising first, `foo/../../.ssh/id_rsa` has file name `id_rsa`
    /// (caught by luck) while `foo/../../.ssh/work` has file name `work` and would be
    /// allowed — the traversal hides which directory it actually lands in.
    #[test]
    fn path_jail_resolves_traversal_before_deciding() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let sneaky = home.join("x/../.ssh/work");
        assert!(
            check_path_jail(&sneaky).is_err(),
            "traversal into ~/.ssh must still be refused"
        );
    }

    /// MCP reads lose access to things a coding assistant has no reason to open.
    ///
    /// These are not "secrets" by name — `~/.bash_history` is a plain text file — but
    /// they are where credentials accumulate, and an MCP path is chosen by whatever is
    /// driving the model rather than by the user.
    #[test]
    fn mcp_jail_blocks_more_than_the_cli_jail() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        for rel in [
            ".bash_history",
            ".zsh_history",
            ".claude.json",
            ".mozilla/firefox/profile/cookies.sqlite",
            ".config/google-chrome/Default/Login Data",
            ".password-store/work/db.gpg",
            ".config/gh/hosts.yml",
            ".m2/settings.xml",
            ".local/share/keyrings/login.keyring",
        ] {
            let p = home.join(rel);
            assert!(
                check_path_jail_mcp(&p).is_err(),
                "~/{rel} must be refused over MCP"
            );
            // The CLI jail is deliberately looser — a human asked for this one.
            // (Only assert that for paths the base jail does not already cover.)
        }
        for abs in [
            "/var/log/auth.log",
            "/etc/machine-id",
            "/run/user/1000/keyring",
        ] {
            assert!(
                check_path_jail_mcp(Path::new(abs)).is_err(),
                "{abs} must be refused over MCP"
            );
        }
    }

    /// The MCP jail is a superset: anything the CLI refuses, MCP refuses too.
    #[test]
    fn mcp_jail_is_a_superset_of_the_cli_jail() {
        for p in ["/etc/shadow", ".env", "id_rsa", ".netrc"] {
            assert!(check_path_jail(Path::new(p)).is_err());
            assert!(
                check_path_jail_mcp(Path::new(p)).is_err(),
                "{p} must stay refused over MCP"
            );
        }
    }

    /// And it must still let an assistant read the code it is there to work on.
    #[test]
    fn mcp_jail_allows_source_files() {
        for p in ["src/main.rs", "README.md", "Cargo.toml", "docs/guide.md"] {
            assert!(
                check_path_jail_mcp(Path::new(p)).is_ok(),
                "{p} must be readable over MCP"
            );
        }
    }

    /// The jail must not swallow ordinary source files.
    #[test]
    fn path_jail_allows_normal_files() {
        for p in [
            "src/main.rs",
            "README.md",
            "Cargo.toml",
            "docs/keynote.md",
            "src/secrets_manager.rs",
        ] {
            assert!(
                check_path_jail(Path::new(p)).is_ok(),
                "{p} is a normal file and must be readable"
            );
        }
    }

    #[test]
    fn test_lines_slice() {
        let content = "line 1\nline 2\nline 3\nline 4\nline 5";
        let sliced = slice_lines(content, 2, 4, false);
        assert_eq!(sliced, "line 2\nline 3\nline 4");
    }

    #[test]
    fn test_skeleton_python() {
        let py_code = r#"
import os

class ModelProxy:
    """Proxy class for LLMs"""
    def __init__(self, port: int):
        self.port = port
        self.active = True

    async def forward_request(self, payload: dict) -> dict:
        result = {}
        return result
"#;
        let skel = skeleton_python(py_code);
        assert!(skel.contains("class ModelProxy"));
        assert!(skel.contains("def __init__"));
        assert!(skel.contains("def forward_request"));
        assert!(!skel.contains("self.active = True"));
    }

    #[test]
    fn test_skeleton_typescript() {
        let ts_code = r#"
import { Request } from "express";

export interface Config {
    port: number;
}

export function startServer(cfg: Config): void {
    const app = express();
    app.listen(cfg.port);
}
"#;
        let skel = skeleton_typescript(ts_code);
        assert!(skel.contains("export interface Config"));
        assert!(skel.contains("export function startServer"));
        assert!(!skel.contains("app.listen"));
    }

    #[test]
    fn test_clean_code() {
        let code = "// comment line\nfn test() {\n    // inner comment\n    let x = 1;\n}";
        let cleaned = clean_code(code, Path::new("test.rs"));
        assert!(!cleaned.contains("// comment line"));
        assert!(!cleaned.contains("// inner comment"));
        assert!(cleaned.contains("let x = 1;"));
    }

    #[test]
    fn skeleton_rust_keeps_nested_and_test_module_fns() {
        let code = r#"
//! Module docs.
use std::sync::Mutex;

pub struct Cfg {
    pub port: u16,
    secret: String,
    pub host: String,
}

pub enum Mode { Fast, Slow, Custom(u8) }

impl Cfg {
    pub fn new(port: u16) -> Self {
        let secret = String::new();
        Cfg { port, secret, host: String::new() }
    }
    fn private_helper(&self) -> bool { true }
}

mod inner {
    pub mod deeper {
        pub fn buried(x: &'static str) -> usize {
            x.len()
        }
    }
    impl super::Cfg {
        pub async fn reload(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
    }
}

pub fn with_inner_fn() -> u8 {
    fn inner_helper(a: u8) -> u8 { a + 1 }
    let s = "a string with { unbalanced brace and a \
             continuation line }";
    let _ = s;
    inner_helper(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_test_one() {
        assert!(true);
    }

    #[test]
    fn nested_test_two() {
        let sse = "data: {\"a\":{\"b\":1}}\n\n\
                   data: [DONE]\n";
        assert!(!sse.is_empty());
    }
}
"#;
        let skel = skeleton_rust(code);
        for name in [
            "fn new",
            "fn private_helper",
            "fn buried",
            "fn reload",
            "fn with_inner_fn",
            "fn inner_helper",
            "fn nested_test_one",
            "fn nested_test_two",
        ] {
            assert!(skel.contains(name), "missing {}\n{}", name, skel);
        }
        // every `fn` in the source survives
        let src_fns = code
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.starts_with("fn ") || t.starts_with("pub fn ") || t.starts_with("pub async fn ")
            })
            .count();
        let skel_fns = skel.lines().filter(|l| l.contains("fn ")).count();
        assert!(skel_fns >= src_fns, "{} < {}\n{}", skel_fns, src_fns, skel);
        // containers and fields
        assert!(skel.contains("mod inner"), "{}", skel);
        assert!(
            skel.contains("pub struct Cfg { port: u16, host: String, … 1 private fields }"),
            "{}",
            skel
        );
        assert!(
            skel.contains("pub enum Mode { Fast, Slow, Custom(u8) }"),
            "{}",
            skel
        ); // single-line enum kept verbatim
        assert!(skel.contains("//! Module docs."), "{}", skel);
        assert!(skel.contains("use std::sync::Mutex;"), "{}", skel);
        // bodies are gone
        assert!(!skel.contains("let secret"), "{}", skel);
        assert!(!skel.contains("assert!(true)"), "{}", skel);
    }

    #[test]
    fn skeleton_rust_survives_lifetimes_and_multiline_signatures() {
        let code = r#"
fn takes_static(x: &'static str) -> usize {
    x.len()
}

fn multi_line(
    a: u32,
    b: u32,
) -> u32 {
    a + b
}

fn after_both() -> bool { true }
"#;
        let skel = skeleton_rust(code);
        assert!(
            skel.contains("fn takes_static(x: &'static str) -> usize"),
            "{}",
            skel
        );
        assert!(
            skel.contains("fn multi_line( a: u32, b: u32, ) -> u32"),
            "{}",
            skel
        );
        assert!(skel.contains("fn after_both"), "{}", skel);
        assert!(!skel.contains("a + b"), "{}", skel);
    }

    #[test]
    fn skeleton_python_keeps_nested_defs_and_constants() {
        let code = r#"
import os
from typing import Optional

MAX_RETRIES = 5

class Repo:
    """Storage for items."""

    name: str

    def __init__(self, root: str):
        self.root = root

    async def fetch(
        self,
        key: str,
    ) -> Optional[str]:
        def _inner(k):
            return k
        return _inner(key)

if __name__ == "__main__":
    Repo("/tmp")
"#;
        let skel = skeleton_python(code);
        for want in [
            "import os",
            "MAX_RETRIES = 5",
            "class Repo:",
            "def __init__",
            "async def fetch",
            "def _inner",
            "name: str",
            "if __name__",
        ] {
            assert!(skel.contains(want), "missing {}\n{}", want, skel);
        }
        assert!(skel.contains("\"Storage for items.\""), "{}", skel);
        assert!(!skel.contains("self.root = root"), "{}", skel);
    }

    #[test]
    fn skeleton_typescript_keeps_class_methods_interface_members_and_tests() {
        let code = r#"
import { Injectable } from "@nestjs/common";

export interface Config {
    port: number;
    host: string;
}

export type Mode = "fast" | "slow";

@Injectable()
export class Service {
    private cache = new Map();

    constructor(private cfg: Config) {}

    async start(): Promise<void> {
        this.cache.clear();
    }

    get ready(): boolean {
        return true;
    }
}

export const helper = (a: number) => a + 1;

describe("Service", () => {
    it("starts", async () => {
        expect(1).toBe(1);
    });
});
"#;
        let skel = skeleton_typescript(code);
        for want in [
            "export interface Config",
            "port: number",
            "host: string",
            "export type Mode",
            "export class Service",
            "async start()",
            "get ready()",
            "export const helper",
            "describe(\"Service\"",
            "it(\"starts\"",
        ] {
            assert!(skel.contains(want), "missing {}\n{}", want, skel);
        }
        assert!(!skel.contains("this.cache.clear()"), "{}", skel);
        assert!(!skel.contains("expect(1)"), "{}", skel);
    }

    #[test]
    fn skeleton_go_keeps_groups_types_and_methods() {
        let code = r#"
package main

import (
    "fmt"
    "os"
)

var (
    ErrMissing = fmt.Errorf("missing")
    debug      = false
)

type Server struct {
    Addr string
    port int
}

func NewServer(addr string) *Server {
    return &Server{Addr: addr}
}

func (s *Server) Start() error {
    return nil
}
"#;
        let skel = skeleton_go(code);
        for want in [
            "package main",
            "import (",
            "\"fmt\"",
            "var ( ErrMissing, debug )",
            "type Server struct",
            "Addr string",
            "unexported",
            "func NewServer",
            "func (s *Server) Start",
        ] {
            assert!(skel.contains(want), "missing {}\n{}", want, skel);
        }
        assert!(!skel.contains("return nil"), "{}", skel);
    }

    #[test]
    fn skeleton_markdown_json_config_and_generic() {
        let md = skeleton_markdown(
            "# Title\n\nFirst sentence here. Second one ignored.\n\n## Usage\n\n```bash\nls\ncd\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- one\n- two\n- three\n- four\n",
        );
        assert!(md.contains("# Title"), "{}", md);
        assert!(md.contains("First sentence here"), "{}", md);
        assert!(!md.contains("Second one ignored"), "{}", md);
        assert!(md.contains("[code: bash, 2 lines]"), "{}", md);
        assert!(md.contains("[table: 2 rows × 2 cols]"), "{}", md); // separator row counts as a row
        assert!(md.contains("[+1 more list items]"), "{}", md);

        let js = skeleton_json(
            "{\"name\":\"prism\",\"deps\":{\"a\":\"1\",\"b\":\"2\"},\"list\":[{\"x\":1},{\"x\":2}]}",
        );
        assert!(js.contains("name: \"prism\""), "{}", js);
        assert!(js.contains("deps: {2 keys}"), "{}", js);
        assert!(js.contains("list: [2 objects]"), "{}", js);

        let cfg = skeleton_config(
            "[package]\nname = \"prism\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = { version = \"1.0\", features = [\"derive\"] }\n",
        );
        assert!(cfg.contains("[package]"), "{}", cfg);
        assert!(cfg.contains("name: prism"), "{}", cfg);

        let generic_out = skeleton_generic(&"x\n".repeat(50));
        assert!(generic_out.contains("[+30 more lines]"), "{}", generic_out);
        assert!(
            generic_out.contains("lines: 50, bytes: 100"),
            "{}",
            generic_out
        );
    }

    #[test]
    fn skeleton_shell_dockerfile_makefile() {
        let sh = skeleton_shell(
            "#!/usr/bin/env bash\nset -euo pipefail\nOUT=/tmp\nrun() {\n  echo hi\n}\necho done\n",
        );
        assert!(sh.contains("#!/usr/bin/env bash"), "{}", sh);
        assert!(sh.contains("set -euo pipefail"), "{}", sh);
        assert!(sh.contains("OUT="), "{}", sh);
        assert!(sh.contains("run()"), "{}", sh);
        assert!(sh.contains("more lines"), "{}", sh);

        let df = skeleton_dockerfile(
            "FROM rust:1.80 AS build\nWORKDIR /app\nRUN cargo build \\\n  --release\nCOPY . .\nEXPOSE 8080\nCMD [\"prism\"]\n",
        );
        assert!(df.contains("FROM rust:1.80 AS build"), "{}", df);
        assert!(df.contains("EXPOSE 8080"), "{}", df);
        assert!(df.contains("RUN/COPY lines"), "{}", df);

        let mk = skeleton_makefile(
            "VERSION = 1.0\nbuild: fmt vet\n\tcargo build\n\ttrue\ntest:\n\tcargo test\n",
        );
        assert!(mk.contains("VERSION="), "{}", mk);
        assert!(mk.contains("build: fmt vet"), "{}", mk);
        assert!(mk.contains("test:"), "{}", mk);
        assert!(mk.contains("[+3 recipe lines]"), "{}", mk);
    }

    #[test]
    fn symbol_map_lists_nested_symbols_with_lines() {
        let code = "mod outer {\n    pub fn top() {}\n    mod inner {\n        pub fn deep() {}\n    }\n}\n";
        let map = generate_symbol_map(code, Path::new("x.rs"));
        assert!(map.contains("SYMBOL MAP"), "{}", map);
        assert!(map.contains("mod outer"), "{}", map);
        assert!(map.contains("L2"), "{}", map);
        assert!(map.contains("pub fn top"), "{}", map);
        assert!(map.contains("pub fn deep"), "{}", map);
    }

    #[test]
    fn skeleton_retains_every_declaration_in_this_crate_file() {
        // reader.rs itself: no `fn` may disappear from its own skeleton
        let src = include_str!("reader.rs");
        let skel = skeleton_rust(src);
        let decl = |t: &str| {
            let r = after_qualifiers(t.trim());
            r.starts_with("fn ")
        };
        let mut missing = Vec::new();
        for line in src.lines() {
            if !decl(line) {
                continue;
            }
            let name = line
                .trim()
                .split("fn ")
                .nth(1)
                .and_then(|r| r.split(['(', '<', ' ']).next())
                .unwrap_or("");
            if name.is_empty() || name.starts_with('$') {
                continue;
            }
            if !skel.contains(&format!("fn {}", name)) {
                missing.push(name.to_string());
            }
        }
        assert!(missing.is_empty(), "dropped declarations: {:?}", missing);
    }

    #[test]
    fn test_read_mode_parsing() {
        assert_eq!("skeleton".parse::<ReadMode>().unwrap(), ReadMode::Skeleton);
        assert_eq!("diff".parse::<ReadMode>().unwrap(), ReadMode::Diff);
        assert_eq!(
            "lines:10-50".parse::<ReadMode>().unwrap(),
            ReadMode::Lines { start: 10, end: 50 }
        );
    }
}
