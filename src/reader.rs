//! PRISM reader.rs — Intelligent 7-mode file reader with AST code skeletonization,
//! session-cached re-reads (~15 tokens), git diff slice, and PathJail protection.

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Supported file reading modes
#[derive(Debug, Clone, PartialEq)]
pub enum ReadMode {
    /// Full file content with optional line numbers
    Full,
    /// Code skeleton: types, signatures, classes, structs (drops function bodies)
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

impl Default for ReadMode {
    fn default() -> Self {
        ReadMode::Skeleton
    }
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
            let range_str = lower.trim_start_matches("lines:").trim_start_matches("line:");
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
    let parts: Vec<&str> = range_str.split(&['-', ':', '.'][..]).filter(|s| !s.is_empty()).collect();
    if parts.len() == 2 {
        let start = parts[0].trim().parse::<usize>().context("Invalid start line")?;
        let end = parts[1].trim().parse::<usize>().context("Invalid end line")?;
        Ok(ReadMode::Lines { start, end })
    } else if parts.len() == 1 {
        let line = parts[0].trim().parse::<usize>().context("Invalid line number")?;
        Ok(ReadMode::Lines { start: line, end: line })
    } else {
        Err(anyhow!("Invalid line range format. Use N-M or N..M, e.g. 10-50"))
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

/// Protect against exfiltrating secrets, private keys, and environment files.
pub fn check_path_jail(path: &Path) -> Result<()> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    let is_secret = file_name.starts_with(".env")
        || file_name.ends_with(".pem")
        || file_name.ends_with(".key")
        || file_name.ends_with(".p12")
        || file_name.ends_with(".pfx")
        || file_name.contains("id_rsa")
        || file_name.contains("id_ed25519")
        || file_name.contains("id_ecdsa")
        || file_name.contains("credentials")
        || (file_name.contains("secret") && (file_name.ends_with(".json") || file_name.ends_with(".yaml") || file_name.ends_with(".yml")));

    if is_secret {
        return Err(anyhow!(
            "PathJail Security: Refusing to read protected sensitive file: {}",
            path.display()
        ));
    }
    Ok(())
}

// ── Session Read Cache ────────────────────────────────────────────────────────

fn session_reads_path() -> PathBuf {
    crate::prism_data_dir().join("cache").join("session_reads.json")
}

fn compute_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
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
    let orig_tokens = crate::analytics::count_tokens(&content, "gpt-4").unwrap_or(content.len() / 4);

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

    let returned_tokens = crate::analytics::count_tokens(&result_text, "gpt-4")
        .unwrap_or(result_text.len() / 4);
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

fn extract_skeleton(content: &str, path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "rs" => skeleton_rust(content),
        "py" => skeleton_python(content),
        "ts" | "tsx" | "js" | "jsx" | "mjs" => skeleton_typescript(content),
        "go" => skeleton_go(content),
        _ => skeleton_generic(content),
    }
}

/// Rust skeletonizer: extracts `pub struct`, `pub enum`, `trait`, `impl`, function signatures.
fn skeleton_rust(content: &str) -> String {
    let mut out = Vec::new();
    let mut brace_depth: i32 = 0;
    let mut in_item_sig = false;
    let mut current_item = String::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Preserve module docs and structural comments
        if trimmed.starts_with("//!") || (trimmed.starts_with("///") && brace_depth == 0) {
            out.push(line.to_string());
            continue;
        }

        // Preserve imports and module declarations
        if brace_depth == 0 && (trimmed.starts_with("use ") || trimmed.starts_with("pub use ") || trimmed.starts_with("mod ") || trimmed.starts_with("pub mod ")) {
            out.push(line.to_string());
            continue;
        }

        // Preserve derive / attribute macros for top-level items
        if brace_depth == 0 && trimmed.starts_with("#[") {
            out.push(line.to_string());
            continue;
        }

        // Check declaration triggers
        let is_decl_start = brace_depth <= 1
            && (trimmed.starts_with("pub fn ")
                || trimmed.starts_with("fn ")
                || trimmed.starts_with("pub async fn ")
                || trimmed.starts_with("async fn ")
                || trimmed.starts_with("pub struct ")
                || trimmed.starts_with("struct ")
                || trimmed.starts_with("pub enum ")
                || trimmed.starts_with("enum ")
                || trimmed.starts_with("pub trait ")
                || trimmed.starts_with("trait ")
                || trimmed.starts_with("impl ")
                || trimmed.starts_with("pub const ")
                || trimmed.starts_with("pub type "));

        if is_decl_start {
            in_item_sig = true;
            current_item = line.to_string();
        } else if in_item_sig {
            current_item.push(' ');
            current_item.push_str(trimmed);
        }

        // Count braces
        let open_count = line.chars().filter(|c| *c == '{').count() as i32;
        let close_count = line.chars().filter(|c| *c == '}').count() as i32;
        brace_depth += open_count - close_count;

        if in_item_sig {
            if trimmed.contains('{') || trimmed.ends_with(';') {
                in_item_sig = false;
                let sig = current_item.split('{').next().unwrap_or(&current_item).trim_end();
                if sig.contains("struct ") && !sig.contains(';') {
                    out.push(format!("{} {{ /* fields */ }}", sig));
                } else if sig.contains("enum ") {
                    out.push(format!("{} {{ /* variants */ }}", sig));
                } else if sig.contains("impl ") {
                    out.push(format!("{} {{ ... }}", sig));
                } else if sig.contains("fn ") {
                    out.push(format!("{} {{ ... }}", sig));
                } else {
                    out.push(sig.to_string());
                }
                current_item.clear();
            }
        }
    }

    if out.is_empty() {
        skeleton_generic(content)
    } else {
        out.join("\n")
    }
}

/// Python skeletonizer: extracts `class`, `def`, async `def`, decorators, docstrings.
fn skeleton_python(content: &str) -> String {
    let mut out = Vec::new();
    let mut in_docstring = false;
    let mut doc_quote = "";

    for line in content.lines() {
        let trimmed = line.trim();

        // Multi-line docstrings
        if trimmed.starts_with("\"\"\"") || trimmed.starts_with("'''") {
            let quote = if trimmed.starts_with("\"\"\"") { "\"\"\"" } else { "'''" };
            if !in_docstring {
                in_docstring = true;
                doc_quote = quote;
                out.push(line.to_string());
                if trimmed.len() > 3 && trimmed[3..].contains(quote) {
                    in_docstring = false;
                }
                continue;
            } else if quote == doc_quote {
                in_docstring = false;
                out.push(line.to_string());
                continue;
            }
        }

        if in_docstring {
            out.push(line.to_string());
            continue;
        }

        // Imports
        if trimmed.starts_with("import ") || trimmed.starts_with("from ") {
            out.push(line.to_string());
            continue;
        }

        // Decorators
        if trimmed.starts_with('@') {
            out.push(line.to_string());
            continue;
        }

        // Classes & Methods
        if trimmed.starts_with("class ") || trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            if trimmed.ends_with(':') {
                out.push(format!("{} ...", line));
            } else {
                out.push(line.to_string());
            }
            continue;
        }
    }

    if out.is_empty() {
        skeleton_generic(content)
    } else {
        out.join("\n")
    }
}

/// TypeScript/JavaScript skeletonizer: exports, interfaces, types, class headers, function headers.
fn skeleton_typescript(content: &str) -> String {
    let mut out = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Imports & exports
        if trimmed.starts_with("import ") || (trimmed.starts_with("export ") && (trimmed.contains("from ") || trimmed.starts_with("export *"))) {
            out.push(line.to_string());
            continue;
        }

        // Types and interfaces
        if trimmed.starts_with("export interface ") || trimmed.starts_with("interface ") || trimmed.starts_with("export type ") || trimmed.starts_with("type ") {
            out.push(line.to_string());
            continue;
        }

        // Functions and classes
        let is_func_or_class = trimmed.starts_with("export function ")
            || trimmed.starts_with("function ")
            || trimmed.starts_with("export async function ")
            || trimmed.starts_with("async function ")
            || trimmed.starts_with("export class ")
            || trimmed.starts_with("class ")
            || trimmed.starts_with("export const ") && (trimmed.contains("=>") || trimmed.contains("function"));

        if is_func_or_class {
            let sig = line.split('{').next().unwrap_or(line).trim_end();
            out.push(format!("{} {{ ... }}", sig));
        }
    }

    if out.is_empty() {
        skeleton_generic(content)
    } else {
        out.join("\n")
    }
}

/// Go skeletonizer: package, imports, types, structs, interfaces, and function signatures.
fn skeleton_go(content: &str) -> String {
    let mut out = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("package ") || trimmed.starts_with("import ") {
            out.push(line.to_string());
            continue;
        }

        if trimmed.starts_with("type ") && (trimmed.contains("struct") || trimmed.contains("interface")) {
            out.push(line.to_string());
            continue;
        }

        if trimmed.starts_with("func ") {
            let sig = line.split('{').next().unwrap_or(line).trim_end();
            out.push(format!("{} {{ ... }}", sig));
        }
    }

    if out.is_empty() {
        skeleton_generic(content)
    } else {
        out.join("\n")
    }
}

/// Generic skeletonizer for unknown extensions: keep structural lines, headers, declarations.
fn skeleton_generic(content: &str) -> String {
    let mut out = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Markdown headings
        if trimmed.starts_with('#') || trimmed.starts_with("==") || trimmed.starts_with("--") {
            out.push(line.to_string());
        } else if trimmed.starts_with("//") || trimmed.starts_with("/*") {
            // Keep top-level comments
            out.push(line.to_string());
        } else if out.len() < 50 {
            // Limit generic fallback
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

// ── Symbol Map ────────────────────────────────────────────────────────────────

fn generate_symbol_map(content: &str, path: &Path) -> String {
    let mut symbols = Vec::new();
    let total_lines = content.lines().count();

    for (idx, line) in content.lines().enumerate() {
        let line_num = idx + 1;
        let trimmed = line.trim();

        if trimmed.starts_with("pub fn ") || trimmed.starts_with("fn ") || trimmed.starts_with("async fn ") {
            let name = trimmed.split('(').next().unwrap_or(trimmed);
            symbols.push(format!("  L{:4}  {}", line_num, name));
        } else if trimmed.starts_with("pub struct ") || trimmed.starts_with("struct ") {
            let name = trimmed.split('{').next().unwrap_or(trimmed);
            symbols.push(format!("  L{:4}  {}", line_num, name));
        } else if trimmed.starts_with("pub enum ") || trimmed.starts_with("enum ") {
            let name = trimmed.split('{').next().unwrap_or(trimmed);
            symbols.push(format!("  L{:4}  {}", line_num, name));
        } else if trimmed.starts_with("pub trait ") || trimmed.starts_with("trait ") {
            let name = trimmed.split('{').next().unwrap_or(trimmed);
            symbols.push(format!("  L{:4}  {}", line_num, name));
        } else if trimmed.starts_with("class ") || trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            let name = trimmed.split('(').next().unwrap_or(trimmed);
            symbols.push(format!("  L{:4}  {}", line_num, name));
        }
    }

    let mut out = String::new();
    out.push_str(&format!("SYMBOL MAP: {} ({} total lines)\n", path.display(), total_lines));
    out.push_str(&"=".repeat(60));
    out.push('\n');
    if symbols.is_empty() {
        out.push_str("  (No top-level function/class symbols detected)\n");
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
                Ok(format!("(No unstaged or committed changes in git for {})", path.display()))
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
        return format!("(Line range {}-{} is outside file bounds of 1-{})", start, end, total);
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
    fn test_read_mode_parsing() {
        assert_eq!("skeleton".parse::<ReadMode>().unwrap(), ReadMode::Skeleton);
        assert_eq!("diff".parse::<ReadMode>().unwrap(), ReadMode::Diff);
        assert_eq!("lines:10-50".parse::<ReadMode>().unwrap(), ReadMode::Lines { start: 10, end: 50 });
    }
}

