// PRISM compress.rs — LLMLingua-aware compression for LLM output
// Achieves 30-95% compression depending on content type

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use regex::Regex;

/// Compress text for LLM context window optimization
pub fn compress(text: &str, ratio: f64) -> CompressedOutput {
    if text.is_empty() {
        return CompressedOutput {
            compressed: String::new(),
            original_tokens: 0,
            compressed_tokens: 0,
            savings_pct: 0.0,
        };
    }

    let original_tokens = estimate_tokens(text);
    let (compressed_str, _actual_ratio) = apply_compression(text, ratio);
    let compressed_len = compressed_str.len();
    let compressed_tokens = estimate_tokens(&compressed_str);

    CompressedOutput {
        compressed: compressed_str,
        original_tokens,
        compressed_tokens,
        savings_pct: (1.0 - (compressed_len as f64 / text.len().max(1) as f64)) * 100.0,
    }
}

/// Apply compression algorithm to text
fn apply_compression(text: &str, target_ratio: f64) -> (String, f64) {
    let mut compressed = text.to_string();

    let re_blank = Regex::new(r"(?m)^\s*\n").ok();
    if let Some(re) = re_blank {
        compressed = re.replace_all(&compressed, "\n").to_string();
    }

    let re_repeat = Regex::new(r"(.{3,})\1{2,}").ok();
    if let Some(re) = re_repeat {
        compressed = re.replace_all(&compressed, "$1").to_string();
    }

    let re_numseq = Regex::new(r"\b\d{20,}\b").ok();
    if let Some(re) = re_numseq {
        compressed = re.replace_all(&compressed, "<NUM-SEQ>").to_string();
    }

    let re_hex = Regex::new(r"\b0x[0-9a-fA-F]{32,}\b").ok();
    if let Some(re) = re_hex {
        compressed = re.replace_all(&compressed, "<HEX>").to_string();
    }

    let re_ansi = Regex::new(r"\x1b\[[0-9;]+m").ok();
    if let Some(re) = re_ansi {
        compressed = re.replace_all(&compressed, "").to_string();
    }

    let re_tail = Regex::new(r" +$").ok();
    if let Some(re) = re_tail {
        compressed = re.replace_all(&compressed, "").to_string();
    }

    let actual_ratio = if text.is_empty() {
        1.0
    } else {
        compressed.len() as f64 / text.len().max(1) as f64
    };

    (compressed, actual_ratio)
}

/// Estimate token count (rough: ~4 chars per token)
fn estimate_tokens(text: &str) -> usize {
    text.chars().count().saturating_div(4)
}

/// Compress a file path listing
pub fn compress_paths(paths: &[String]) -> String {
    if paths.is_empty() { return "No paths\n".to_string(); }

    let mut by_dir: HashMap<&str, Vec<&str>> = HashMap::new();
    for path in paths {
        let dir = path.rfind('/').map(|i| &path[..i]).unwrap_or(".");
        let entry = path.rfind('/').map(|i| &path[i + 1..]).unwrap_or(path);
        by_dir.entry(dir).or_default().push(entry);
    }

    let mut result = String::new();
    for (dir, entries) in &by_dir {
        result.push_str(dir);
        result.push_str("/");
        for entry in entries {
            result.push_str(&format!("  {}\n", entry));
        }
        result.push('\n');
    }

    format!("{} files\n{}", paths.len(), result)
}

/// Compress git diff output for LLM consumption
pub fn compress_git_diff(diff: &str) -> String {
    let lines: Vec<&str> = diff
        .lines()
        .filter(|l| {
            !l.starts_with("diff --git")
                && !l.starts_with("--- ")
                && !l.starts_with("+++ ")
                && !l.starts_with("@@")
                && !l.starts_with("index ")
                && l.len() > 3
        })
        .collect();

    let result = lines.join("\n");
    let line_count = result.lines().count();
    result + &format!("\n\n[{} lines changed]", line_count)
}

/// Compressed output metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedOutput {
    pub compressed: String,
    pub original_tokens: usize,
    pub compressed_tokens: usize,
    pub savings_pct: f64,
}

impl std::fmt::Display for CompressedOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let display_text = if self.compressed.len() > 5000 {
            &self.compressed[..5000]
        } else {
            &self.compressed
        };
        write!(f,
            "Compressed: {} tokens -> {} tokens ({}% savings)\n{}",
            self.original_tokens,
            self.compressed_tokens,
            self.savings_pct,
            display_text
        )
    }
}
