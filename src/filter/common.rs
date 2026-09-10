//! Shared helpers for every output filter.
//!
//! Fidelity contract for all filters:
//!   * never drop a match/entry silently — anything cut is announced with [`more`]
//!   * caps come from [`Limits`] (config `filters:` section / `PRISM_FILTER_*` env)
//!   * `prism cmd` tees raw output to disk when a filter truncated, so a `[+N more …]`
//!     marker is always recoverable

use serde_json::Value;
use std::sync::OnceLock;

pub use crate::config::FilterLimits as Limits;

// ── limits ────────────────────────────────────────────────────────────────────

pub fn limits() -> &'static Limits {
    static L: OnceLock<Limits> = OnceLock::new();
    L.get_or_init(|| {
        // Full chain (hub > project `.prismrc` > global > defaults), not just the
        // global file — a hub-pushed or project-local filter cap now actually applies.
        let mut l = crate::config::resolve().filters;
        l.apply_env();
        l
    })
}

// ── truncation markers ────────────────────────────────────────────────────────

/// Standard truncation marker. `what` is a plural noun: "lines", "files", "matches".
pub fn more(n: usize, what: &str) -> String {
    format!("[+{} more {}]", n, what)
}

/// True when `s` carries at least one [`more`] marker.
pub fn has_truncation(s: &str) -> bool {
    let mut rest = s;
    while let Some(i) = rest.find("[+") {
        let tail = &rest[i + 2..];
        let digits = tail.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 && tail[digits..].starts_with(" more ") {
            return true;
        }
        rest = tail;
    }
    false
}

/// Keep the first `max` lines, append a marker for the rest.
pub fn cap_lines<'a, I: IntoIterator<Item = &'a str>>(lines: I, max: usize, what: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut dropped = 0usize;
    for l in lines {
        if out.len() < max { out.push(l) } else { dropped += 1 }
    }
    let mut s = out.join("\n");
    if dropped > 0 {
        if !s.is_empty() { s.push('\n') }
        s.push_str(&more(dropped, what));
    }
    s
}

/// Same as [`cap_lines`] for owned strings.
pub fn cap_vec(mut lines: Vec<String>, max: usize, what: &str) -> Vec<String> {
    if lines.len() > max {
        let dropped = lines.len() - max;
        lines.truncate(max);
        lines.push(more(dropped, what));
    }
    lines
}

/// Keep the last `max` lines (log tails), marker first.
pub fn tail_lines(output: &str, max: usize, what: &str) -> String {
    let all: Vec<&str> = output.lines().collect();
    if all.len() <= max {
        return all.join("\n");
    }
    let dropped = all.len() - max;
    let mut s = more(dropped, what);
    s.push('\n');
    s.push_str(&all[dropped..].join("\n"));
    s
}

// ── text ──────────────────────────────────────────────────────────────────────

/// Remove ANSI CSI/OSC escape sequences.
pub fn strip_ansi(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                // CSI: parameters/intermediates 0x20..=0x3F, final byte 0x40..=0x7E
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) { break }
                }
            }
            Some(']') => {
                chars.next();
                // OSC: terminated by BEL or ESC \
                let mut prev_esc = false;
                for c in chars.by_ref() {
                    if c == '\x07' || (prev_esc && c == '\\') { break }
                    prev_esc = c == '\x1b';
                }
            }
            Some(_) => { chars.next(); }
            None => {}
        }
    }
    out
}

/// Truncate to `max` chars on a char boundary, appending `…`.
pub fn truncate(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}

/// Collapse runs of blank lines to a single blank line and trim trailing blanks.
pub fn collapse_blank(s: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut last_blank = true; // drops leading blanks too
    for l in s.lines() {
        let blank = l.trim().is_empty();
        if blank && last_blank { continue }
        out.push(if blank { "" } else { l });
        last_blank = blank;
    }
    while matches!(out.last(), Some(l) if l.is_empty()) { out.pop(); }
    out.join("\n")
}

/// Collapse identical consecutive lines into `line  (×N)`.
pub fn dedupe_consecutive(lines: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for l in lines {
        match out.last_mut() {
            Some((prev, n)) if *prev == l => *n += 1,
            _ => out.push((l, 1)),
        }
    }
    out.into_iter()
        .map(|(l, n)| if n > 1 { format!("{}  (×{})", l, n) } else { l })
        .collect()
}

/// Squeeze internal whitespace runs to one space (for tabular tool output).
pub fn squeeze_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Box-drawing tree (cargo tree / npm ls / pnpm list) → indentation-only tree.
/// `│   ├── foo` becomes ` foo` with one leading space per depth level.
/// True when a line looks like a diagnostic worth keeping.
pub fn is_alert_line(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    ["error", "err!", "fatal", "panic", "warn", "exception", "traceback"]
        .iter()
        .any(|k| l.contains(k))
}

/// Group `"<object> <verb>"` lines by their trailing verb.
///
/// `kubectl apply` prints one line per object and so do `helm`, `nomad` and friends. The
/// verb is the information; the objects are a list. Grouped, each verb is paid for once.
/// Returns `(grouped, leftovers)` — lines whose last word is not in `verbs` come back
/// untouched so the caller decides how to render them, rather than losing them here.
pub fn group_by_verb(output: &str, verbs: &[&str], max: usize) -> (Vec<String>, Vec<String>) {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        match t.rsplit_once(' ') {
            Some((obj, verb)) if verbs.contains(&verb) => {
                match groups.iter_mut().find(|(v, _)| v == verb) {
                    Some((_, v)) => v.push(obj.to_string()),
                    None => groups.push((verb.to_string(), vec![obj.to_string()])),
                }
            }
            _ => other.push(t.to_string()),
        }
    }
    let rendered = groups
        .into_iter()
        .map(|(verb, objs)| {
            let n = objs.len();
            let shown = n.min(max);
            let mut s = format!("{} ({}): {}", verb, n, objs[..shown].join(", "));
            if n > shown {
                s.push_str(&format!(", {}", more(n - shown, "objects")));
            }
            s
        })
        .collect();
    (rendered, other)
}

pub fn flatten_tree(line: &str) -> String {
    let mut depth = 0usize;
    let mut rest = line;
    loop {
        let mut advanced = false;
        for p in ["│   ", "│ ", "    ", "  "] {
            if let Some(r) = rest.strip_prefix(p) {
                depth += 1;
                rest = r;
                advanced = true;
                break;
            }
        }
        if !advanced { break }
    }
    for p in ["├─┬ ", "└─┬ ", "├── ", "└── ", "├─ ", "└─ ", "|-- ", "`-- ", "+-- ", "\\-- "] {
        if let Some(r) = rest.strip_prefix(p) {
            depth += 1;
            rest = r;
            break;
        }
    }
    if depth == 0 {
        return line.trim_end().to_string();
    }
    format!("{}{}", " ".repeat(depth), rest.trim_end())
}

/// `1234` → `1.2K`; ls-style sizes.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{}B", bytes)
    } else if v >= 100.0 {
        format!("{:.0}{}", v, UNITS[i])
    } else {
        format!("{:.1}{}", v, UNITS[i])
    }
}

/// Parse a size like `4.6GB`, `137MB`, `10.7K`, `958` into bytes.
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let num_end = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
    let num: f64 = s[..num_end].parse().ok()?;
    let unit = s[num_end..].trim().to_ascii_uppercase();
    let mult = match unit.as_str() {
        "" | "B" => 1.0,
        "K" | "KB" | "KIB" => 1024.0,
        "M" | "MB" | "MIB" => 1024.0 * 1024.0,
        "G" | "GB" | "GIB" => 1024.0 * 1024.0 * 1024.0,
        "T" | "TB" | "TIB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((num * mult) as u64)
}

// ── args ──────────────────────────────────────────────────────────────────────

/// First non-flag argument, skipping the value of common option-with-value flags.
pub fn find_subcommand<'a>(args: &[&'a str]) -> Option<&'a str> {
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        if matches!(arg, "-C" | "-c" | "--git-dir" | "--work-tree" | "--manifest-path" | "-n" | "--namespace" | "--context" | "--profile" | "--region")
            && i + 1 < args.len()
        {
            i += 2;
            continue;
        }
        if arg.starts_with('-') {
            i += 1;
            continue;
        }
        return Some(arg);
    }
    None
}

/// True if `short` (e.g. "-l", also matched inside clusters like "-la") or `long` is present.
pub fn has_flag(args: &[&str], short: Option<char>, long: Option<&str>) -> bool {
    args.iter().any(|a| {
        if let Some(l) = long {
            if *a == l || a.starts_with(&format!("{}=", l)) { return true }
        }
        if let Some(c) = short {
            if a.starts_with('-') && !a.starts_with("--") && a[1..].contains(c) { return true }
        }
        false
    })
}

/// Value of `--flag=value` or `--flag value`.
pub fn flag_value<'a>(args: &[&'a str], long: &str) -> Option<&'a str> {
    for (i, a) in args.iter().enumerate() {
        if let Some(v) = a.strip_prefix(long).and_then(|r| r.strip_prefix('=')) {
            return Some(v);
        }
        if *a == long {
            return args.get(i + 1).copied();
        }
    }
    None
}

// ── JSON ──────────────────────────────────────────────────────────────────────

/// Parse a JSON document, or a stream of whitespace/newline separated documents.
pub fn parse_json(s: &str) -> Option<Vec<Value>> {
    let t = s.trim();
    if t.is_empty() { return None }
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Some(vec![v]);
    }
    let mut out = Vec::new();
    let mut de = serde_json::Deserializer::from_str(t).into_iter::<Value>();
    while let Some(v) = de.next() {
        out.push(v.ok()?);
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Render JSON as a compact YAML-like block: unquoted keys, 1-space indent,
/// inline scalar arrays, and uniform object arrays as `k1|k2` tables.
/// Lossless except for announced caps (`json_max_array`, `json_max_lines`).
pub fn compact_json(v: &Value) -> String {
    let l = limits();
    let mut lines = Vec::new();
    render_value(v, 0, None, &mut lines, l.json_max_array);
    let out = cap_vec(lines, l.json_max_lines, "lines");
    out.join("\n")
}

/// Filter helper: render a JSON output (or JSON stream); fall back to `fallback` when not JSON.
pub fn compact_json_output(output: &str, fallback: impl Fn(&str) -> String) -> String {
    match parse_json(output) {
        Some(docs) => docs.iter().map(compact_json).collect::<Vec<_>>().join("\n---\n"),
        None => fallback(output),
    }
}

fn key_str(k: &str) -> String {
    let plain = !k.is_empty()
        && k.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '@' | ':'));
    if plain { k.to_string() } else { serde_json::to_string(k).unwrap_or_default() }
}

pub fn scalar_str(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => {
            let needs_quote = s.is_empty()
                || s.contains('\n')
                || s.starts_with(' ')
                || s.ends_with(' ')
                || matches!(s.as_str(), "null" | "true" | "false")
                || s.parse::<f64>().is_ok()
                || s.starts_with('[')
                || s.starts_with('{')
                || s.contains(" | ");
            if needs_quote { serde_json::to_string(s).unwrap_or_default() } else { s.clone() }
        }
        Value::Array(a) if a.is_empty() => "[]".into(),
        Value::Object(o) if o.is_empty() => "{}".into(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn is_scalar(v: &Value) -> bool {
    !matches!(v, Value::Array(a) if !a.is_empty()) && !matches!(v, Value::Object(o) if !o.is_empty())
}

fn uniform_keys(arr: &[Value]) -> Option<Vec<String>> {
    if arr.len() < 2 { return None }
    let first = arr.first()?.as_object()?;
    if first.is_empty() || first.len() > 12 { return None }
    let keys: Vec<String> = first.keys().cloned().collect();
    for v in arr {
        let o = v.as_object()?;
        if o.len() != keys.len() { return None }
        for k in &keys {
            let val = o.get(k)?;
            if !is_scalar(val) { return None }
        }
    }
    Some(keys)
}

fn render_value(v: &Value, depth: usize, key: Option<&str>, out: &mut Vec<String>, max_arr: usize) {
    let pad = " ".repeat(depth);
    let prefix = |k: Option<&str>| k.map(|k| format!("{}: ", key_str(k))).unwrap_or_default();
    match v {
        Value::Object(o) if !o.is_empty() => {
            if let Some(k) = key {
                out.push(format!("{}{}:", pad, key_str(k)));
            }
            let child = if key.is_some() { depth + 1 } else { depth };
            for (k, val) in o {
                render_value(val, child, Some(k), out, max_arr);
            }
        }
        Value::Array(a) if !a.is_empty() => {
            let shown = a.len().min(max_arr);
            if a.iter().all(is_scalar) {
                let items: Vec<String> = a[..shown].iter().map(scalar_str).collect();
                let inline = items.join(", ");
                if inline.len() <= 120 {
                    let extra = if a.len() > shown { format!(", {}", more(a.len() - shown, "items")) } else { String::new() };
                    out.push(format!("{}{}[{}{}]", pad, prefix(key), inline, extra));
                } else {
                    out.push(format!("{}{}[{}]", pad, prefix(key), a.len()));
                    for it in items { out.push(format!("{} - {}", pad, it)); }
                    if a.len() > shown { out.push(format!("{} {}", pad, more(a.len() - shown, "items"))); }
                }
            } else if let Some(keys) = uniform_keys(a) {
                out.push(format!("{}{}[{}] {}", pad, prefix(key), a.len(), keys.iter().map(|k| key_str(k)).collect::<Vec<_>>().join("|")));
                for it in &a[..shown] {
                    let o = it.as_object().unwrap();
                    let row: Vec<String> = keys.iter().map(|k| scalar_str(&o[k]).replace('|', "\\|")).collect();
                    out.push(format!("{} {}", pad, row.join("|")));
                }
                if a.len() > shown { out.push(format!("{} {}", pad, more(a.len() - shown, "rows"))); }
            } else {
                out.push(format!("{}{}[{}]", pad, prefix(key), a.len()));
                for it in &a[..shown] {
                    let start = out.len();
                    render_value(it, depth + 2, None, out, max_arr);
                    if let Some(first) = out.get_mut(start) {
                        // turn first child line into "- ..." bullet
                        let trimmed = first.trim_start().to_string();
                        *first = format!("{} - {}", pad, trimmed);
                    }
                }
                if a.len() > shown { out.push(format!("{} {}", pad, more(a.len() - shown, "items"))); }
            }
        }
        scalar => out.push(format!("{}{}{}", pad, prefix(key), scalar_str(scalar))),
    }
}

// ── generic fallback ──────────────────────────────────────────────────────────

/// Conservative passthrough for commands without a dedicated filter:
/// ANSI stripped, blank runs collapsed, capped at `passthrough_max_lines`.
pub fn generic(output: &str) -> String {
    let l = limits();
    let clean = collapse_blank(&strip_ansi(output));
    cap_lines(clean.lines(), l.passthrough_max_lines, "lines")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn marker_roundtrip() {
        let m = more(12, "files");
        assert_eq!(m, "[+12 more files]");
        assert!(has_truncation(&format!("x\n{}", m)));
        assert!(!has_truncation("[+ not a marker]"));
        assert!(!has_truncation("array[+1] more"));
    }

    #[test]
    fn strip_ansi_handles_csi_and_osc() {
        assert_eq!(strip_ansi("\x1b[1;31mred\x1b[0m plain"), "red plain");
        assert_eq!(strip_ansi("\x1b]8;;http://x\x07link\x1b]8;;\x07"), "link");
        assert_eq!(strip_ansi("no escapes"), "no escapes");
    }

    #[test]
    fn truncate_is_char_safe() {
        assert_eq!(truncate("héllo wörld", 6), "héllo…");
        assert_eq!(truncate("short", 10), "short");
    }

    #[test]
    fn flatten_tree_levels() {
        assert_eq!(flatten_tree("├── anyhow v1.0"), " anyhow v1.0");
        assert_eq!(flatten_tree("│   ├── bytes v1.11.1"), "  bytes v1.11.1");
        assert_eq!(flatten_tree("│ ├── @angular/core@19"), "  @angular/core@19");
        assert_eq!(flatten_tree("├─┬ @nestjs/cli@11"), " @nestjs/cli@11");
        assert_eq!(flatten_tree("root v0.1.0"), "root v0.1.0");
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(958), "958B");
        assert_eq!(human_size(10957), "10.7K");
        assert_eq!(parse_size("4.6GB"), Some((4.6 * 1024.0 * 1024.0 * 1024.0) as u64));
        assert_eq!(parse_size("137MB"), Some(137 * 1024 * 1024));
    }

    #[test]
    fn compact_json_shapes() {
        let v = json!({
            "name": "prism", "version": "0.1.0", "private": true, "empty": {}, "none": null,
            "tags": ["a", "b"],
            "deps": {"serde": "^1", "tokio": "1.40"},
            "rows": [{"id": 1, "ok": true}, {"id": 2, "ok": false}],
            "mixed": [1, {"x": [1,2]}]
        });
        let s = compact_json(&v);
        assert!(s.contains("name: prism"));
        assert!(s.contains("private: true"));
        assert!(s.contains("empty: {}"));
        assert!(s.contains("tags: [a, b]"));
        assert!(s.contains("deps:\n serde: ^1\n tokio: \"1.40\""), "{}", s); // numeric-looking strings stay quoted
        assert!(s.contains("rows: [2] id|ok\n 1|true\n 2|false"));
        assert!(s.contains("mixed: [2]"));
        assert!(s.contains("version: 0.1.0"));
    }

    #[test]
    fn compact_json_quotes_ambiguous_strings() {
        let s = compact_json(&json!({"n": "123", "b": "true", "e": "", "sp": " x"}));
        assert!(s.contains("n: \"123\""));
        assert!(s.contains("b: \"true\""));
        assert!(s.contains("e: \"\""));
        assert!(s.contains("sp: \" x\""));
    }

    #[test]
    fn parse_json_stream() {
        let docs = parse_json("{\"a\":1}\n{\"a\":2}").unwrap();
        assert_eq!(docs.len(), 2);
        assert!(parse_json("not json").is_none());
    }

    #[test]
    fn find_subcommand_skips_option_values() {
        assert_eq!(find_subcommand(&["-C", "/tmp", "status", "-s"]), Some("status"));
        assert_eq!(find_subcommand(&["--no-pager", "log"]), Some("log"));
        assert_eq!(find_subcommand(&["-n", "kube-system", "get", "pods"]), Some("get"));
    }

    #[test]
    fn has_flag_clusters() {
        assert!(has_flag(&["-la"], Some('l'), Some("--long")));
        assert!(has_flag(&["--long"], Some('l'), Some("--long")));
        assert!(!has_flag(&["--color=always"], Some('l'), Some("--long")));
    }

    #[test]
    fn caps_announce_dropped() {
        let s = cap_lines(["a", "b", "c"], 2, "lines");
        assert_eq!(s, "a\nb\n[+1 more lines]");
        let t = tail_lines("1\n2\n3\n4", 2, "lines");
        assert_eq!(t, "[+2 more lines]\n3\n4");
    }

    #[test]
    fn dedupe_counts() {
        let v = dedupe_consecutive(["x", "x", "y", "x"].iter().map(|s| s.to_string()));
        assert_eq!(v, vec!["x  (×2)", "y", "x"]);
    }
}
