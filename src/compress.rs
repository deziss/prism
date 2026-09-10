//! PRISM compress.rs — BM25-style sentence scoring + regex cleanup.
//!
//! LLMLingua-2 inspired: score sentences by term importance, keep top-K
//! to hit target_ratio. No external model required.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Public API ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedOutput {
    pub compressed: String,
    pub original_tokens: usize,
    pub compressed_tokens: usize,
    pub savings_pct: f64,
}

impl std::fmt::Display for CompressedOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let preview = if self.compressed.len() > 5000 {
            &self.compressed[..5000]
        } else {
            &self.compressed
        };
        write!(
            f,
            "Compressed: {} → {} tokens ({:.1}% savings)\n{}",
            self.original_tokens, self.compressed_tokens, self.savings_pct, preview
        )
    }
}

/// Compress text: BM25 sentence scoring over prose, code passed through intact.
///
/// `ratio` = fraction of tokens to keep (0.7 = keep 70%, discard 30%).
///
/// Segmentation runs first and guards both later stages. That matters for the
/// cleanup pass as much as for the scorer: `regex_clean` collapses runs of
/// spaces, which would silently reindent — and in Python, break — any code it
/// touched.
pub fn compress(text: &str, ratio: f64) -> CompressedOutput {
    if text.is_empty() {
        return CompressedOutput {
            compressed: String::new(),
            original_tokens: 0,
            compressed_tokens: 0,
            savings_pct: 0.0,
        };
    }

    let original_tokens = count_tokens_accurate(text);
    let segments = segment_protected(text);

    let protected_tokens: usize = segments
        .iter()
        .filter(|s| s.protected)
        .map(|s| count_tokens_accurate(&s.text))
        .sum();
    let prose_total: usize = segments
        .iter()
        .filter(|s| !s.protected)
        .map(|s| count_tokens_accurate(&s.text))
        .sum();

    // Nothing but code — there is nothing safe to compress.
    if prose_total == 0 {
        return CompressedOutput {
            compressed: text.to_string(),
            original_tokens,
            compressed_tokens: original_tokens,
            savings_pct: 0.0,
        };
    }

    let target_tokens = (original_tokens as f64 * ratio.clamp(0.1, 1.0)) as usize;
    // Code is never dropped, so prose absorbs the whole reduction. Keep at least
    // a fifth of it so code-heavy prompts stay readable.
    let prose_budget = target_tokens
        .saturating_sub(protected_tokens)
        .max(prose_total / 5);

    let mut out = String::with_capacity(text.len());
    for seg in &segments {
        if seg.protected {
            out.push_str(&seg.text);
            continue;
        }

        let cleaned = regex_clean(&seg.text);
        let seg_tokens = count_tokens_accurate(&seg.text);
        let share = seg_tokens as f64 / prose_total as f64;
        let seg_budget = (prose_budget as f64 * share).round() as usize;

        // Short inputs: the cleanup pass alone is enough.
        let piece = if original_tokens > 300 {
            bm25_select_prose(&cleaned, seg_budget)
        } else {
            cleaned
        };
        out.push_str(&piece);
        out.push('\n');
    }

    let compressed = out.trim_end_matches('\n').to_string();
    let compressed_tokens = count_tokens_accurate(&compressed);
    let savings_pct = (1.0 - compressed_tokens as f64 / original_tokens.max(1) as f64) * 100.0;

    CompressedOutput {
        compressed,
        original_tokens,
        compressed_tokens,
        savings_pct: savings_pct.max(0.0),
    }
}

// ── Regex cleanup (lossless or near-lossless) ─────────────────────────────────

fn regex_clean(text: &str) -> String {
    let mut s = text.to_string();

    // Remove ANSI escape codes
    if let Ok(re) = Regex::new(r"\x1b\[[0-9;]*[mGKHF]") {
        s = re.replace_all(&s, "").to_string();
    }
    // Collapse 3+ blank lines to 1
    if let Ok(re) = Regex::new(r"\n{3,}") {
        s = re.replace_all(&s, "\n\n").to_string();
    }
    // Remove trailing whitespace per line
    if let Ok(re) = Regex::new(r"(?m) +$") {
        s = re.replace_all(&s, "").to_string();
    }
    // (Repeated-sequence collapsing was attempted here via a backreference regex,
    // `(.{4,})\1{3,}` — the `regex` crate does not support backreferences at all, so
    // `Regex::new` always returned `Err` and this was silently a no-op from day one.
    // Removed rather than reimplemented without backreferences: no behavior changes.)
    // Replace long hex strings
    if let Ok(re) = Regex::new(r"\b0x[0-9a-fA-F]{32,}\b") {
        s = re.replace_all(&s, "<HEX>").to_string();
    }
    // Replace very long numeric sequences
    if let Ok(re) = Regex::new(r"\b\d{20,}\b") {
        s = re.replace_all(&s, "<NUM>").to_string();
    }
    // Collapse multiple spaces (not newlines)
    if let Ok(re) = Regex::new(r"[ \t]{2,}") {
        s = re.replace_all(&s, " ").to_string();
    }

    s.trim().to_string()
}

// ── BM25 sentence scoring ─────────────────────────────────────────────────────

// Score sentences by BM25-style importance, keep top sentences until we
// hit `target_tokens`. Re-joins in original order to preserve coherence.
// ── Code protection ───────────────────────────────────────────────────────────
//
// BM25 drops whole lines. Applied blindly to a prompt containing a fenced code
// block, it happily deletes lines from the middle of that block and forwards
// syntactically broken code to the model. Code spans are therefore segmented
// out and passed through verbatim; only prose is ever scored.

struct Segment {
    text: String,
    protected: bool,
}

fn push_segment(segs: &mut Vec<Segment>, cur: &mut String, protected: bool) {
    if !cur.is_empty() {
        segs.push(Segment {
            text: std::mem::take(cur),
            protected,
        });
    }
}

/// A line that looks like code, a diff hunk, or structured output.
fn is_code_like_line(line: &str) -> bool {
    if line.trim().is_empty() {
        return false;
    }
    line.starts_with("    ")
        || line.starts_with('\t')
        || line.starts_with("@@ ")
        || line.starts_with("diff --git")
        || line.starts_with("+++ ")
        || line.starts_with("--- ")
}

/// Split text into alternating prose and protected (code) segments.
fn segment_protected(text: &str) -> Vec<Segment> {
    let mut segs: Vec<Segment> = Vec::new();
    let mut cur = String::new();
    let mut cur_protected = false;
    let mut in_fence = false;
    let mut fence_marker = String::new();

    for line in text.split('\n') {
        let trimmed = line.trim_start();
        let is_fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");

        if in_fence {
            cur.push_str(line);
            cur.push('\n');
            if is_fence && trimmed.starts_with(&fence_marker) {
                in_fence = false;
                push_segment(&mut segs, &mut cur, true);
                cur_protected = false;
            }
            continue;
        }

        if is_fence {
            push_segment(&mut segs, &mut cur, cur_protected);
            fence_marker = trimmed
                .chars()
                .take_while(|c| *c == '`' || *c == '~')
                .collect();
            in_fence = true;
            cur.push_str(line);
            cur.push('\n');
            continue;
        }

        let code_like = is_code_like_line(line);
        if code_like != cur_protected {
            push_segment(&mut segs, &mut cur, cur_protected);
            cur_protected = code_like;
        }
        cur.push_str(line);
        cur.push('\n');
    }

    push_segment(&mut segs, &mut cur, in_fence || cur_protected);
    segs
}

fn bm25_select_prose(text: &str, target_tokens: usize) -> String {
    // Split into sentences (paragraphs / lines as units — better for prompts)
    let sentences: Vec<&str> = text.split('\n').filter(|s| !s.trim().is_empty()).collect();

    if sentences.len() <= 3 {
        return text.to_string(); // Too short to meaningfully select
    }

    // Build corpus TF for IDF calculation
    let mut df: HashMap<String, usize> = HashMap::new();
    let tokenized: Vec<Vec<String>> = sentences.iter().map(|s| tokenize_words(s)).collect();

    for words in &tokenized {
        let mut seen = std::collections::HashSet::new();
        for w in words {
            if seen.insert(w.clone()) {
                *df.entry(w.clone()).or_insert(0) += 1;
            }
        }
    }

    let n = sentences.len() as f64;
    let k1 = 1.5f64;
    let b = 0.75f64;
    let avg_len = tokenized.iter().map(|t| t.len()).sum::<usize>() as f64 / n;

    // BM25 score each sentence against the full corpus as "query"
    // (self-relevance: sentences with high-IDF terms score higher)
    let mut scores: Vec<(usize, f64)> = tokenized
        .iter()
        .enumerate()
        .map(|(i, words)| {
            let dl = words.len() as f64;
            let score: f64 = words
                .iter()
                .map(|w| {
                    let tf = words.iter().filter(|x| *x == w).count() as f64;
                    let df_w = *df.get(w).unwrap_or(&1) as f64;
                    let idf = ((n - df_w + 0.5) / (df_w + 0.5) + 1.0).ln();
                    let tf_norm = (tf * (k1 + 1.0)) / (tf + k1 * (1.0 - b + b * dl / avg_len));
                    idf * tf_norm
                })
                .sum();

            // Boost: code lines, function signatures, key sentences
            let boost = if is_important_line(sentences[i]) {
                1.5
            } else {
                1.0
            };
            (i, score * boost)
        })
        .collect();

    // Sort by score descending
    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Greedily pick sentences until target_tokens reached
    let mut selected: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut budget = target_tokens;

    for (idx, _score) in &scores {
        if budget == 0 {
            break;
        }
        let tok = count_tokens_accurate(sentences[*idx]);
        if tok <= budget || selected.is_empty() {
            selected.insert(*idx);
            budget = budget.saturating_sub(tok);
        }
    }

    // Re-join in original order
    sentences
        .iter()
        .enumerate()
        .filter(|(i, _)| selected.contains(i))
        .map(|(_, s)| *s)
        .collect::<Vec<_>>()
        .join("\n")
}

fn tokenize_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.len() > 2)
        .map(|w| w.to_lowercase())
        .filter(|w| !is_stopword(w))
        .collect()
}

fn is_important_line(line: &str) -> bool {
    let t = line.trim();
    // Code: function/class/const/import definitions
    t.starts_with("fn ") || t.starts_with("pub ") || t.starts_with("def ")
    || t.starts_with("class ") || t.starts_with("function ")
    || t.starts_with("const ") || t.starts_with("let ") || t.starts_with("var ")
    || t.starts_with("import ") || t.starts_with("use ")
    || t.starts_with("async ") || t.starts_with("export ")
    // Markdown headers
    || t.starts_with('#')
    // Bullet points
    || t.starts_with("- ") || t.starts_with("* ") || t.starts_with("• ")
    // Error / warning lines
    || t.to_lowercase().contains("error") || t.to_lowercase().contains("warning")
}

fn is_stopword(w: &str) -> bool {
    matches!(
        w,
        "the"
            | "and"
            | "for"
            | "are"
            | "but"
            | "not"
            | "you"
            | "all"
            | "can"
            | "had"
            | "her"
            | "was"
            | "one"
            | "our"
            | "out"
            | "day"
            | "get"
            | "has"
            | "him"
            | "his"
            | "how"
            | "its"
            | "now"
            | "see"
            | "two"
            | "who"
            | "did"
            | "any"
            | "may"
            | "new"
            | "own"
            | "use"
            | "way"
            | "she"
            | "many"
            | "than"
            | "then"
            | "them"
            | "they"
            | "this"
            | "from"
            | "that"
            | "with"
            | "have"
            | "will"
            | "your"
            | "been"
            | "also"
            | "more"
            | "each"
            | "over"
            | "such"
            | "into"
            | "some"
            | "when"
            | "what"
            | "were"
            | "only"
            | "just"
    )
}

// ── Token counting (accurate via tiktoken) ────────────────────────────────────

fn count_tokens_accurate(text: &str) -> usize {
    // Shared, lazily-built tokenizer — see analytics::bpe.
    match crate::analytics::bpe() {
        Some(b) => b.encode_ordinary(text).len(),
        // Fallback: chars / 3.5 ≈ tokens (BPE approximation)
        None => (text.chars().count() as f64 / 3.5) as usize,
    }
}

// ── Helpers (used by other modules) ──────────────────────────────────────────

pub fn compress_paths(paths: &[String]) -> String {
    if paths.is_empty() {
        return "No paths\n".to_string();
    }
    let mut by_dir: HashMap<&str, Vec<&str>> = HashMap::new();
    for path in paths {
        let dir = path.rfind('/').map(|i| &path[..i]).unwrap_or(".");
        let file = path
            .rfind('/')
            .map(|i| &path[i + 1..])
            .unwrap_or(path.as_str());
        by_dir.entry(dir).or_default().push(file);
    }
    let mut result = format!("{} files\n", paths.len());
    let mut dirs: Vec<_> = by_dir.iter().collect();
    dirs.sort_by_key(|(d, _)| *d);
    for (dir, files) in dirs {
        result.push_str(&format!("{}/\n", dir));
        for f in files {
            result.push_str(&format!("  {}\n", f));
        }
    }
    result
}

pub fn compress_git_diff(diff: &str) -> String {
    let lines: Vec<&str> = diff
        .lines()
        .filter(|l| {
            !l.starts_with("diff --git")
                && !l.starts_with("--- ")
                && !l.starts_with("+++ ")
                && !l.starts_with("index ")
                && l.len() > 3
        })
        .collect();
    let count = lines.len();
    format!("{}\n\n[{} changed lines]", lines.join("\n"), count)
}

#[cfg(test)]
mod code_protection_tests {
    use super::*;

    /// Long enough to push `compress` past its BM25 threshold.
    fn prose(lines: usize) -> String {
        (0..lines)
            .map(|i| {
                format!(
                    "Paragraph {i} explains a routine detail about the surrounding system \
                     and repeats several common words so the scorer has something to rank."
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn fenced_code_block_survives_verbatim() {
        let code = "```rust\nfn main() {\n    let x = 1;\n    let y = 2;\n    println!(\"{}\", x + y);\n}\n```";
        let input = format!("{}\n{}\n{}", prose(40), code, prose(40));

        let out = compress(&input, 0.3).compressed;

        for line in code.lines() {
            assert!(
                out.contains(line),
                "compression dropped a line from inside a fenced block: {line:?}"
            );
        }
    }

    #[test]
    fn diff_hunk_survives_verbatim() {
        let diff = "@@ -1,4 +1,4 @@\n-let old = 1;\n+let new = 2;\n context line here";
        let input = format!("{}\n{}\n{}", prose(40), diff, prose(40));

        let out = compress(&input, 0.3).compressed;

        for line in diff.lines() {
            assert!(
                out.contains(line),
                "compression dropped a diff line: {line:?}"
            );
        }
    }

    #[test]
    fn segmentation_marks_fences_protected() {
        let segs = segment_protected("intro line\n```\ncode\n```\noutro line");
        assert!(segs.iter().any(|s| s.protected && s.text.contains("code")));
        assert!(
            segs.iter()
                .any(|s| !s.protected && s.text.contains("intro"))
        );
    }

    #[test]
    fn compression_never_grows_short_text() {
        let short = "hello world";
        let out = compress(short, 0.5);
        assert!(out.compressed_tokens <= out.original_tokens.max(1));
    }
}
