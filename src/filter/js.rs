//! JavaScript / TypeScript toolchain filters: tsc, eslint, biome, prettier, jest,
//! vitest, npm/pnpm/yarn, npx, bun, deno, next, playwright.
//!
//! Every filter is a real parser for the tool's output shape. Success paths collapse
//! to one `tool: …` trailer line; failures/diagnostics are kept in full (minus code
//! frames, whose content is addressed by the kept `path:line:col`) up to the
//! configured caps, and every cut is announced with a `[+N more …]` marker.

use super::common::*;
use regex::Regex;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::OnceLock;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static Regex {
            static R: OnceLock<Regex> = OnceLock::new();
            R.get_or_init(|| Regex::new($pat).expect("static regex"))
        }
    };
}

// ─── small private helpers ────────────────────────────────────────────────────

/// Normalize CR-only progress redraws and CRLF to plain `\n`.
fn norm(s: &str) -> Cow<'_, str> {
    if s.contains('\r') {
        Cow::Owned(s.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(s)
    }
}

re!(re_gutter, r"^\s*(?:>\s*)?\d+\s*\|");
re!(re_caret_only, r"^\s*\|?\s*[\^~]*\s*$");

/// Code-frame gutter lines (`> 12 |  expect(…)`, `   |     ^`, `~~~~`): the kept
/// `path:line:col` already addresses this source, so the frame is decoration.
fn is_gutter(l: &str) -> bool {
    let t = l.trim_end();
    if t.trim().is_empty() {
        return false;
    }
    re_gutter().is_match(t) || re_caret_only().is_match(t)
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {}", word)
    } else {
        format!("{} {}s", n, word)
    }
}

/// `3.2 s` / `1.23s` / `15.00ms` → compact `3.2s`.
fn compact_time(s: &str) -> String {
    s.trim()
        .replace(" s", "s")
        .replace(" ms", "ms")
        .replace(" m", "m")
}

/// Collector for one failure body: message lines kept (minus blanks/code frames),
/// only the first stack frame kept, capped at `max_diagnostics` lines.
struct Body {
    lines: Vec<String>,
    seen_frame: bool,
    dropped: usize,
    max: usize,
}

impl Body {
    fn new() -> Self {
        Body {
            lines: Vec::new(),
            seen_frame: false,
            dropped: 0,
            max: limits().max_diagnostics,
        }
    }
    fn push(&mut self, line: &str, is_frame: bool, indent: &str) {
        let t = line.trim();
        if t.is_empty() || is_gutter(line) {
            return;
        }
        if is_frame {
            if self.seen_frame {
                return;
            }
            self.seen_frame = true;
        }
        if self.lines.len() >= self.max {
            self.dropped += 1;
            return;
        }
        self.lines.push(format!("{}{}", indent, squeeze_ws(t)));
    }
    fn finish(mut self, indent: &str) -> Vec<String> {
        if self.dropped > 0 {
            self.lines
                .push(format!("{}{}", indent, more(self.dropped, "lines")))
        }
        self.lines
    }
}

#[allow(dead_code)]
fn is_at_frame(l: &str) -> bool {
    let t = l.trim_start();
    t.starts_with("at ") || t.starts_with("❯ ") && t.contains(':') && !t.contains(' ')
}

/// Group `dir/file` paths as `dir/` + indented file names, capped.
fn group_paths_by_dir(paths: &[&str]) -> Vec<String> {
    let mut by_dir: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for p in paths {
        let (d, f) = match p.rfind('/') {
            Some(i) => (&p[..i + 1], &p[i + 1..]),
            None => ("", *p),
        };
        by_dir.entry(d).or_default().push(f);
    }
    let mut out = Vec::new();
    for (d, files) in by_dir {
        if !d.is_empty() {
            out.push(d.to_string())
        }
        for f in files {
            out.push(format!(" {}", f))
        }
    }
    out
}

// ─── tsc ──────────────────────────────────────────────────────────────────────

re!(
    re_tsc_plain,
    r"^(.+?)\((\d+),(\d+)\): (error|warning) (TS\d+): (.*)$"
);
re!(
    re_tsc_pretty,
    r"^(.+?):(\d+):(\d+) - (error|warning) (TS\d+): (.*)$"
);
re!(re_tsc_global, r"^(error|warning) (TS\d+): (.*)$");
re!(re_tsc_ts, r"^\[[0-9:APM ]+\] ");

struct TsDiag {
    file: String,
    pos: String,
    kind: String,
    code: String,
    msg: String,
}

fn tsc_noise(t: &str) -> bool {
    t.starts_with("Found ") && (t.contains(" error") || t.ends_with("errors."))
        || t.starts_with("Starting compilation")
        || t.starts_with("Starting incremental compilation")
        || t.starts_with("Watching for file changes")
        || t.starts_with("File change detected")
}

pub(crate) fn filter_tsc(output: &str) -> String {
    let l = limits();
    let mut diags: Vec<TsDiag> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    for raw in norm(output).lines() {
        let line = re_tsc_ts().replace(raw, "");
        let t = line.trim_end();
        if t.trim().is_empty() || tsc_noise(t.trim()) || is_gutter(t) {
            continue;
        }
        if let Some(c) = re_tsc_plain()
            .captures(t)
            .or_else(|| re_tsc_pretty().captures(t))
        {
            diags.push(TsDiag {
                file: c[1].to_string(),
                pos: format!("{}:{}", &c[2], &c[3]),
                kind: c[4].to_string(),
                code: c[5].to_string(),
                msg: c[6].trim().to_string(),
            });
        } else if let Some(c) = re_tsc_global().captures(t) {
            diags.push(TsDiag {
                file: String::new(),
                pos: String::new(),
                kind: c[1].to_string(),
                code: c[2].to_string(),
                msg: c[3].trim().to_string(),
            });
        } else if t.starts_with(' ') || t.starts_with('\t') {
            // continuation of a multi-line message
            match diags.last_mut() {
                Some(d) => {
                    d.msg.push(' ');
                    d.msg.push_str(t.trim())
                }
                None => other.push(t.trim().to_string()),
            }
        } else {
            other.push(t.to_string());
        }
    }
    if diags.is_empty() && other.is_empty() {
        return "tsc: ok".into();
    }

    let mut out: Vec<String> = Vec::new();
    let mut cur_file: Option<&str> = None;
    let mut shown = 0usize;
    let mut cut_files: std::collections::BTreeSet<&str> = Default::default();
    let mut cut = 0usize;
    for d in &diags {
        if shown >= l.max_diagnostics {
            cut += 1;
            cut_files.insert(&d.file);
            continue;
        }
        shown += 1;
        if d.file.is_empty() {
            out.push(format!("{} {} {}", d.kind, d.code, d.msg));
            cur_file = None;
            continue;
        }
        if cur_file != Some(d.file.as_str()) {
            out.push(d.file.clone());
            cur_file = Some(&d.file);
        }
        let k = if d.kind == "error" { "" } else { "warning " };
        out.push(format!("  {} {}{} {}", d.pos, k, d.code, d.msg));
    }
    if cut > 0 {
        out.push(more(
            cut,
            &format!("errors in {}", plural(cut_files.len(), "file")),
        ))
    }
    out.extend(cap_vec(other, l.passthrough_max_lines, "lines"));

    let errors = diags.iter().filter(|d| d.kind == "error").count();
    let warnings = diags.len() - errors;
    let mut files: std::collections::BTreeSet<&str> = Default::default();
    for d in &diags {
        if !d.file.is_empty() {
            files.insert(&d.file);
        }
    }
    let mut codes: BTreeMap<&str, usize> = BTreeMap::new();
    for d in &diags {
        *codes.entry(&d.code).or_default() += 1
    }
    let mut codes: Vec<(&str, usize)> = codes.into_iter().collect();
    codes.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let codes_s = codes
        .iter()
        .map(|(c, n)| {
            if *n > 1 {
                format!("{} ×{}", c, n)
            } else {
                c.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut trailer = if diags.is_empty() {
        "tsc: ok".to_string()
    } else {
        format!("tsc: {}", plural(errors, "error"))
    };
    if warnings > 0 {
        trailer.push_str(&format!(", {}", plural(warnings, "warning")))
    }
    if !files.is_empty() {
        trailer.push_str(&format!(" in {}", plural(files.len(), "file")))
    }
    if !codes_s.is_empty() {
        trailer.push_str(&format!(" ({})", codes_s))
    }
    out.push(trailer);
    out.join("\n")
}

// ─── eslint ───────────────────────────────────────────────────────────────────

re!(
    re_eslint_msg,
    r"^\s+(\d+):(\d+)\s+(error|warning)\s+(.*?)(?:\s{2,}(\S+))?\s*$"
);
re!(
    re_eslint_summary,
    r"^\s*✖ (\d+) problems? \((\d+) errors?, (\d+) warnings?\)"
);
re!(
    re_eslint_fixable,
    r"^\s*(\d+) errors? and (\d+) warnings? potentially fixable"
);

pub(crate) fn filter_eslint(output: &str) -> String {
    let l = limits();
    let mut out: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    let mut errors = 0usize;
    let mut warnings = 0usize;
    let mut fixable: Option<(usize, usize)> = None;
    let mut summary_seen = false;
    let mut pending_file: Option<String> = None;
    let mut shown = 0usize;
    let mut cut = 0usize;
    let mut cut_files: std::collections::BTreeSet<String> = Default::default();
    let mut cur_file = String::new();
    for line in norm(output).lines() {
        let t = line.trim_end();
        if t.trim().is_empty() {
            continue;
        }
        if let Some(c) = re_eslint_summary().captures(t) {
            summary_seen = true;
            errors = c[2].parse().unwrap_or(0);
            warnings = c[3].parse().unwrap_or(0);
            continue;
        }
        if let Some(c) = re_eslint_fixable().captures(t) {
            fixable = Some((c[1].parse().unwrap_or(0), c[2].parse().unwrap_or(0)));
            continue;
        }
        if let Some(c) = re_eslint_msg().captures(t) {
            if shown >= l.max_diagnostics {
                cut += 1;
                cut_files.insert(cur_file.clone());
                continue;
            }
            shown += 1;
            if let Some(f) = pending_file.take() {
                out.push(f)
            }
            let rule = c
                .get(5)
                .map(|m| format!("  {}", m.as_str()))
                .unwrap_or_default();
            out.push(format!(
                "  {}:{} {} {}{}",
                &c[1],
                &c[2],
                &c[3],
                squeeze_ws(&c[4]),
                rule
            ));
            continue;
        }
        if !t.starts_with(' ') && !t.starts_with('\t') {
            // stylish file header (or a compact/unix single-line report — kept as-is)
            cur_file = t.to_string();
            pending_file = Some(t.to_string());
            continue;
        }
        other.push(t.trim().to_string());
    }
    if let Some(f) = pending_file.take() {
        // header without messages: treat as plain content, never drop
        other.insert(0, f);
    }
    if cut > 0 {
        out.push(more(
            cut,
            &format!("problems in {}", plural(cut_files.len(), "file")),
        ))
    }
    out.extend(cap_vec(other, l.passthrough_max_lines, "lines"));
    if !summary_seen {
        // count what we parsed when eslint printed no ✖ summary
        for line in &out {
            let t = line.trim_start();
            if let Some(c) = re_eslint_msg().captures(&format!(" {}", t)) {
                if &c[3] == "error" {
                    errors += 1
                } else {
                    warnings += 1
                }
            }
        }
    }
    if out.is_empty() && errors == 0 && warnings == 0 {
        return "eslint: ok".into();
    }
    let mut trailer = format!(
        "eslint: {}, {}",
        plural(errors, "error"),
        plural(warnings, "warning")
    );
    if let Some((fe, fw)) = fixable {
        if fe + fw > 0 {
            trailer.push_str(&format!(" ({} fixable)", fe + fw))
        }
    }
    out.push(trailer);
    out.join("\n")
}

// ─── biome ────────────────────────────────────────────────────────────────────

re!(
    re_biome_count,
    r"^(Checked|Found|Fixed|Skipped|Formatted) (\d+) (files?|errors?|warnings?)"
);

fn biome_body_line(t: &str) -> bool {
    // code frames and diff bodies carry `│`; carets/diff markers alone are decoration
    !t.contains('│')
        && !t
            .trim()
            .chars()
            .all(|c| matches!(c, '^' | '-' | '+' | ' ' | '~'))
}

pub(crate) fn filter_biome(args: &[&str], output: &str) -> String {
    let sub = args.first().copied().unwrap_or("");
    let is_diag_cmd =
        matches!(sub, "check" | "lint" | "format" | "ci" | "search") || output.contains(" ━");
    if !is_diag_cmd {
        return generic(output);
    }
    let l = limits();
    let mut blocks: Vec<Vec<String>> = Vec::new();
    let mut cur: Option<Vec<String>> = None;
    let mut cur_body: Option<Body> = None;
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut other: Vec<String> = Vec::new();
    let flush =
        |cur: &mut Option<Vec<String>>, body: &mut Option<Body>, blocks: &mut Vec<Vec<String>>| {
            if let Some(mut head) = cur.take() {
                if let Some(b) = body.take() {
                    head.extend(b.finish("  "))
                }
                blocks.push(head);
            }
        };
    for line in norm(output).lines() {
        let t = line.trim_end();
        if t.trim().is_empty() {
            continue;
        }
        if !t.starts_with(' ') && t.contains('━') {
            flush(&mut cur, &mut cur_body, &mut blocks);
            let head = t.trim_end_matches(['━', ' ']).to_string();
            cur = Some(vec![squeeze_ws(&head)]);
            cur_body = Some(Body::new());
            continue;
        }
        if let Some(c) = re_biome_count().captures(t) {
            flush(&mut cur, &mut cur_body, &mut blocks);
            let n: usize = c[2].parse().unwrap_or(0);
            let key = match (&c[1], &c[3]) {
                ("Found", w) if w.starts_with("error") => "errors",
                ("Found", w) if w.starts_with("warning") => "warnings",
                ("Checked", _) => "checked",
                ("Fixed", _) | ("Formatted", _) => "fixed",
                ("Skipped", _) => "skipped",
                _ => "other",
            };
            *counts.entry(key).or_default() += n;
            continue;
        }
        if let Some(b) = cur_body.as_mut() {
            if t.starts_with(' ') {
                if biome_body_line(t) {
                    b.push(t, false, "  ")
                }
                continue;
            }
            flush(&mut cur, &mut cur_body, &mut blocks);
        }
        other.push(t.to_string());
    }
    flush(&mut cur, &mut cur_body, &mut blocks);

    let mut out: Vec<String> = Vec::new();
    let total_blocks = blocks.len();
    for (i, b) in blocks.into_iter().enumerate() {
        if i >= l.max_diagnostics {
            out.push(more(total_blocks - i, "diagnostics"));
            break;
        }
        out.extend(b);
    }
    out.extend(cap_vec(other, l.passthrough_max_lines, "lines"));
    let errors = counts.get("errors").copied().unwrap_or(0);
    let warnings = counts.get("warnings").copied().unwrap_or(0);
    let mut trailer = if errors + warnings + total_blocks == 0 {
        "biome: ok".to_string()
    } else {
        format!(
            "biome: {}, {}",
            plural(errors, "error"),
            plural(warnings, "warning")
        )
    };
    let mut extra = Vec::new();
    if let Some(n) = counts.get("checked") {
        extra.push(format!("checked {}", n))
    }
    if let Some(n) = counts.get("fixed") {
        extra.push(format!("fixed {}", n))
    }
    if let Some(n) = counts.get("skipped") {
        extra.push(format!("skipped {}", n))
    }
    if !extra.is_empty() {
        trailer.push_str(&format!(" ({})", extra.join(", ")))
    }
    out.push(trailer);
    out.join("\n")
}

// ─── prettier ─────────────────────────────────────────────────────────────────

re!(
    re_prettier_write,
    r"^(\S.*?)\s+(\d+)\s*ms(?:\s+\((unchanged|cached)\))?\s*$"
);

pub(crate) fn filter_prettier(output: &str) -> String {
    let l = limits();
    let mut need: Vec<String> = Vec::new();
    let mut changed: Vec<String> = Vec::new();
    let mut unchanged = 0usize;
    let mut errors: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    let mut ok = false;
    for line in norm(output).lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("Checking formatting") {
            continue;
        }
        if t.starts_with("All matched files use Prettier code style") {
            ok = true;
            continue;
        }
        if let Some(rest) = t.strip_prefix("[warn] ") {
            if rest.starts_with("Code style issues found")
                || rest.starts_with("Forgot to run Prettier")
            {
                continue;
            }
            need.push(rest.to_string());
            continue;
        }
        if t.starts_with("[error]") {
            errors.push(t.to_string());
            continue;
        }
        if let Some(c) = re_prettier_write().captures(t) {
            if c.get(3).is_some() {
                unchanged += 1
            } else {
                changed.push(c[1].to_string())
            }
            continue;
        }
        other.push(t.to_string());
    }
    let mut out: Vec<String> = Vec::new();
    if !need.is_empty() {
        out.push(format!(
            "prettier: {} need formatting:",
            plural(need.len(), "file")
        ));
        out.extend(cap_vec(need, l.list_max_lines, "files"));
    }
    if !changed.is_empty() || unchanged > 0 {
        let mut h = format!("prettier: formatted {}", plural(changed.len(), "file"));
        if unchanged > 0 {
            h.push_str(&format!(" ({} unchanged)", unchanged))
        }
        if !changed.is_empty() {
            h.push(':')
        }
        out.push(h);
        out.extend(cap_vec(changed, l.list_max_lines, "files"));
    }
    out.extend(cap_vec(errors, l.max_diagnostics, "errors"));
    out.extend(cap_vec(other, l.passthrough_max_lines, "lines"));
    if out.is_empty() {
        return if ok || output.trim().is_empty() {
            "prettier: ok".into()
        } else {
            generic(output)
        };
    }
    out.join("\n")
}

// ─── jest ─────────────────────────────────────────────────────────────────────

re!(
    re_jest_counts,
    r"(\d+) (failed|passed|skipped|todo|total|obsolete|written|updated)"
);
re!(re_jest_verbose_test, r"^\s+[✓✕○√×]\s");

/// `2 failed, 5 passed, 7 total` → map.
fn count_map(s: &str) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for c in re_jest_counts().captures_iter(s) {
        m.insert(c[2].to_string(), c[1].parse().unwrap_or(0));
    }
    m
}

fn is_path_like(t: &str) -> bool {
    !t.is_empty() && !t.contains(' ') && t.contains('/') && !t.starts_with('(')
}

pub(crate) fn filter_jest(output: &str) -> String {
    let l = limits();
    let text = norm(output);
    let lines: Vec<&str> = text.lines().collect();
    // `jest --listTests`: only paths → group by directory
    let non_blank: Vec<&str> = lines
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if !non_blank.is_empty() && non_blank.iter().all(|t| is_path_like(t)) {
        let mut out = vec![format!("jest: {}", plural(non_blank.len(), "test file"))];
        out.extend(cap_vec(
            group_paths_by_dir(&non_blank),
            l.list_max_lines,
            "lines",
        ));
        return out.join("\n");
    }

    let mut out: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    let mut passed_suites = 0usize;
    let mut failed_suites: Vec<String> = Vec::new();
    let mut tests: BTreeMap<String, usize> = BTreeMap::new();
    let mut suites: BTreeMap<String, usize> = BTreeMap::new();
    let mut snaps: BTreeMap<String, usize> = BTreeMap::new();
    let mut time = String::new();
    let mut in_fail = false;
    let mut body: Option<Body> = None;
    let mut failures = 0usize;
    let mut cut_failures = 0usize;
    let mut in_summary_repeat = false;
    let mut in_coverage = false;
    let mut coverage: Vec<String> = Vec::new();

    let flush_body = |body: &mut Option<Body>, out: &mut Vec<String>| {
        if let Some(b) = body.take() {
            out.extend(b.finish("    "))
        }
    };

    for line in &lines {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        // coverage table
        if tt.starts_with("---") && tt.contains("|") && tt.ends_with("--") {
            in_coverage = true;
            continue;
        }
        if in_coverage && tt.contains('|') {
            coverage.push(squeeze_ws(tt).replace(" | ", "|"));
            continue;
        }
        in_coverage = false;

        if tt.starts_with("Summary of all failing tests") {
            in_summary_repeat = true;
            flush_body(&mut body, &mut out);
            in_fail = false;
            continue;
        }
        if let Some(rest) = tt.strip_prefix("Test Suites:") {
            suites = count_map(rest);
            flush_body(&mut body, &mut out);
            in_fail = false;
            continue;
        }
        if let Some(rest) = tt.strip_prefix("Tests:") {
            tests = count_map(rest);
            continue;
        }
        if let Some(rest) = tt.strip_prefix("Snapshots:") {
            snaps = count_map(rest);
            continue;
        }
        if let Some(rest) = tt.strip_prefix("Time:") {
            time = compact_time(rest.split(',').next().unwrap_or(""));
            continue;
        }
        if tt.starts_with("Ran all test suites")
            || tt.starts_with("RUNS ")
            || tt.starts_with("Determining test suites")
        {
            continue;
        }
        if tt.starts_with("PASS ") {
            flush_body(&mut body, &mut out);
            in_fail = false;
            passed_suites += 1;
            continue;
        }
        if let Some(rest) = tt.strip_prefix("FAIL ") {
            flush_body(&mut body, &mut out);
            in_fail = !in_summary_repeat;
            if in_fail {
                let path = rest.split(" (").next().unwrap_or(rest).trim().to_string();
                failed_suites.push(path.clone());
                if failures < l.test_max_failures {
                    out.push(format!("FAIL {}", path))
                }
            }
            continue;
        }
        if let Some(title) = tt.strip_prefix("● ") {
            if in_summary_repeat {
                continue;
            }
            flush_body(&mut body, &mut out);
            in_fail = true;
            failures += 1;
            if failures > l.test_max_failures {
                cut_failures += 1;
                continue;
            }
            out.push(format!(" ● {}", title.trim_end_matches(" ›").trim()));
            body = Some(Body::new());
            continue;
        }
        if in_fail {
            if t.starts_with(' ') || t.starts_with('\t') {
                if let Some(b) = body.as_mut() {
                    let frame = tt.starts_with("at ");
                    b.push(t, frame, "    ");
                }
                continue;
            }
            flush_body(&mut body, &mut out);
            in_fail = false;
        }
        if re_jest_verbose_test().is_match(t) {
            continue;
        }
        other.push(tt.to_string());
    }
    flush_body(&mut body, &mut out);
    if cut_failures > 0 {
        out.push(more(cut_failures, "failures"))
    }
    if !coverage.is_empty() {
        out.push("coverage:".into());
        out.extend(cap_vec(coverage, l.list_max_lines, "rows"));
    }
    out.extend(cap_vec(other, l.log_tail, "lines"));

    if tests.is_empty() && suites.is_empty() && passed_suites + failed_suites.len() == 0 {
        return if out.is_empty() {
            String::new()
        } else {
            out.join("\n")
        };
    }
    let get = |m: &BTreeMap<String, usize>, k: &str| m.get(k).copied().unwrap_or(0);
    let (tp, tf) = if tests.is_empty() {
        (passed_suites, failed_suites.len())
    } else {
        (get(&tests, "passed"), get(&tests, "failed"))
    };
    let mut parts = vec![format!("{} passed", tp), format!("{} failed", tf)];
    if get(&tests, "skipped") > 0 {
        parts.push(format!("{} skipped", get(&tests, "skipped")))
    }
    if get(&tests, "todo") > 0 {
        parts.push(format!("{} todo", get(&tests, "todo")))
    }
    let mut paren = Vec::new();
    if get(&tests, "total") > 0 {
        paren.push(plural(get(&tests, "total"), "test"))
    }
    let ns = if get(&suites, "total") > 0 {
        get(&suites, "total")
    } else {
        passed_suites + failed_suites.len()
    };
    if ns > 0 {
        paren.push(plural(ns, "suite"))
    }
    if !time.is_empty() {
        paren.push(time)
    }
    let mut trailer = format!("jest: {}", parts.join(", "));
    if !paren.is_empty() {
        trailer.push_str(&format!(" ({})", paren.join(", ")))
    }
    if get(&snaps, "failed") > 0 || get(&snaps, "obsolete") > 0 {
        trailer.push_str(&format!(
            "; snapshots: {} failed, {} obsolete",
            get(&snaps, "failed"),
            get(&snaps, "obsolete")
        ));
    }
    out.push(trailer);
    out.join("\n")
}

// ─── vitest ───────────────────────────────────────────────────────────────────

re!(
    re_vitest_file,
    r"^\s*([✓❯↓×✗])\s+(\S+)\s+\((\d+) tests?(?:\s*\|\s*([^)]*))?\)"
);
re!(
    re_vitest_summary,
    r"^\s*(Test Files|Tests|Errors|Type Errors|Duration|Start at)\s{2,}(.*)$"
);
re!(
    re_vitest_counts,
    r"(\d+) (failed|passed|skipped|todo|error)"
);

pub(crate) fn filter_vitest(output: &str) -> String {
    let l = limits();
    let text = norm(output);
    let has_fail_section = text.lines().any(|x| x.trim_start().starts_with("FAIL "));
    let mut out: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    let mut passed_files = 0usize;
    let mut summary: BTreeMap<String, String> = BTreeMap::new();
    let mut body: Option<Body> = None;
    let mut failures = 0usize;
    let mut cut = 0usize;
    let mut in_fail = false;
    let flush = |body: &mut Option<Body>, out: &mut Vec<String>| {
        if let Some(b) = body.take() {
            out.extend(b.finish("  "))
        }
    };

    for line in text.lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if tt.starts_with("RUN ") && tt.contains(" v") {
            continue;
        }
        if tt.starts_with('⎯') {
            flush(&mut body, &mut out);
            in_fail = false;
            continue;
        }
        if let Some(c) = re_vitest_summary().captures(t) {
            flush(&mut body, &mut out);
            in_fail = false;
            summary.insert(c[1].to_string(), c[2].trim().to_string());
            continue;
        }
        if let Some(c) = re_vitest_file().captures(t) {
            flush(&mut body, &mut out);
            in_fail = false;
            let glyph = &c[1];
            if glyph == "✓" && c.get(4).is_none() {
                passed_files += 1;
                continue;
            }
            let extra = c
                .get(4)
                .map(|m| format!(" | {}", m.as_str().trim()))
                .unwrap_or_default();
            out.push(format!("{} {} ({} tests{})", glyph, &c[2], &c[3], extra));
            continue;
        }
        if let Some(rest) = tt.strip_prefix("FAIL ") {
            flush(&mut body, &mut out);
            failures += 1;
            if failures > l.test_max_failures {
                cut += 1;
                in_fail = false;
                continue;
            }
            out.push(format!("FAIL {}", squeeze_ws(rest)));
            body = Some(Body::new());
            in_fail = true;
            continue;
        }
        if in_fail {
            if let Some(b) = body.as_mut() {
                let frame = tt.starts_with("❯ ");
                if frame && !tt.contains(':') {
                    continue;
                }
                b.push(t, frame, "  ");
            }
            continue;
        }
        // per-test tree lines: ✓ counted by summary; × / → kept only without a FAIL section
        if tt.starts_with("✓ ") || tt.starts_with("↓ ") || tt.starts_with("√ ") {
            continue;
        }
        if tt.starts_with("× ") || tt.starts_with("✗ ") || tt.starts_with("→ ") {
            if !has_fail_section {
                out.push(format!("  {}", squeeze_ws(tt)))
            }
            continue;
        }
        if tt.starts_with("press h")
            || tt.starts_with("Waiting for file changes")
            || tt.starts_with("Start at")
        {
            continue;
        }
        other.push(tt.to_string());
    }
    flush(&mut body, &mut out);
    if cut > 0 {
        out.push(more(cut, "failures"))
    }
    out.extend(cap_vec(other, l.log_tail, "lines"));

    if summary.is_empty() && passed_files == 0 && out.is_empty() {
        return String::new();
    }
    let counts = |k: &str| -> BTreeMap<String, usize> {
        let mut m = BTreeMap::new();
        if let Some(s) = summary.get(k) {
            for c in re_vitest_counts().captures_iter(s) {
                m.insert(c[2].to_string(), c[1].parse().unwrap_or(0));
            }
        }
        m
    };
    let tests = counts("Tests");
    let files = counts("Test Files");
    let g = |m: &BTreeMap<String, usize>, k: &str| m.get(k).copied().unwrap_or(0);
    let (tp, tf) = if tests.is_empty() {
        (passed_files, failures)
    } else {
        (g(&tests, "passed"), g(&tests, "failed"))
    };
    let mut parts = vec![format!("{} passed", tp), format!("{} failed", tf)];
    if g(&tests, "skipped") > 0 {
        parts.push(format!("{} skipped", g(&tests, "skipped")))
    }
    if g(&tests, "todo") > 0 {
        parts.push(format!("{} todo", g(&tests, "todo")))
    }
    if let Some(e) = summary.get("Errors") {
        parts.push(e.clone())
    }
    if let Some(e) = summary.get("Type Errors") {
        parts.push(format!("type errors: {}", e))
    }
    let mut paren = Vec::new();
    let total_tests: usize = tests.values().sum::<usize>();
    if total_tests > 0 {
        paren.push(plural(total_tests, "test"))
    }
    let nfiles: usize = if files.is_empty() {
        passed_files
    } else {
        files.values().sum()
    };
    if nfiles > 0 {
        paren.push(plural(nfiles, "file"))
    }
    if let Some(d) = summary.get("Duration") {
        paren.push(compact_time(d.split(" (").next().unwrap_or(d)))
    }
    let mut trailer = format!("vitest: {}", parts.join(", "));
    if !paren.is_empty() {
        trailer.push_str(&format!(" ({})", paren.join(", ")))
    }
    out.push(trailer);
    out.join("\n")
}

// ─── npm / pnpm / yarn ────────────────────────────────────────────────────────

/// Subcommand + remaining args, skipping leading global flags (and their values).
fn npm_split<'a>(args: &[&'a str]) -> (Option<&'a str>, Vec<&'a str>) {
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if !a.starts_with('-') {
            return (Some(a), args[i + 1..].to_vec());
        }
        if matches!(
            a,
            "-w" | "--workspace"
                | "-C"
                | "--prefix"
                | "--userconfig"
                | "--loglevel"
                | "--filter"
                | "-F"
                | "--cwd"
        ) {
            i += 2;
        } else {
            i += 1;
        }
    }
    (None, Vec::new())
}

fn is_pm_log(t: &str) -> bool {
    t.starts_with("npm ERR!")
        || t.starts_with("npm error")
        || t.starts_with("npm WARN")
        || t.starts_with("npm warn")
        || t.starts_with("npm notice")
        || t.starts_with(" WARN ")
        || t.starts_with(" ERR_PNPM")
        || t.starts_with("ERR_PNPM")
        || t.starts_with("warning ")
        || t.starts_with("error ")
        || t.starts_with("➤ YN")
}

pub(crate) fn filter_npm(args: &[&str], output: &str) -> String {
    let (sub, rest) = npm_split(args);
    let json = has_flag(args, None, Some("--json"));
    match sub {
        Some("ls") | Some("list") | Some("la") | Some("ll") | Some("why") | Some("explain") => {
            if json {
                npm_ls_json(output)
            } else {
                npm_tree(output)
            }
        }
        Some("run") | Some("run-script") if rest.iter().all(|a| a.starts_with('-')) => {
            npm_scripts(output)
        }
        Some("run") | Some("run-script") | Some("test") | Some("t") | Some("tst")
        | Some("start") | Some("exec") | Some("dlx") => npm_script_output(&rest, output),
        Some("install") | Some("i") | Some("add") | Some("ci") | Some("update") | Some("up")
        | Some("upgrade") | Some("uninstall") | Some("remove") | Some("rm") | Some("un")
        | Some("dedupe") | Some("prune") | Some("link") | Some("rebuild") | Some("import") => {
            npm_install(output)
        }
        Some("config") | Some("c") => match rest.first() {
            Some(&"list") | Some(&"ls") => npm_config_list(output),
            _ => generic(output),
        },
        Some("pkg") | Some("version") | Some("view") | Some("info") | Some("show") | Some("v") => {
            compact_json_output(output, generic)
        }
        Some("outdated") => {
            if json {
                compact_json_output(output, npm_outdated)
            } else {
                npm_outdated(output)
            }
        }
        Some("audit") => {
            if json {
                compact_json_output(output, npm_audit)
            } else {
                npm_audit(output)
            }
        }
        Some("fund") | Some("licenses") => npm_tree(output),
        _ => {
            if json {
                compact_json_output(output, generic)
            } else {
                generic(output)
            }
        }
    }
}

// ── npm ls tree ──────────────────────────────────────────────────────────────

/// Split a dependency-tree line into (depth, node text). `None` when the line is
/// not a tree node. npm/pnpm trees use 2-char units (`│ `, `  `), yarn 3-char.
fn tree_node(line: &str) -> Option<(usize, &str)> {
    let mut rest = line;
    let mut depth = 0usize;
    loop {
        let mut advanced = false;
        for u in ["│ ", "| ", "  "] {
            if let Some(r) = rest.strip_prefix(u) {
                rest = r;
                depth += 1;
                advanced = true;
                break;
            }
        }
        if !advanced {
            break;
        }
    }
    let rest = rest.trim_start_matches(' ');
    for c in [
        "├─┬ ",
        "└─┬ ",
        "├── ",
        "└── ",
        "├─ ",
        "└─ ",
        "+-- ",
        "`-- ",
        "|-- ",
        "\\-- ",
    ] {
        if let Some(r) = rest.strip_prefix(c) {
            return Some((depth + 1, r.trim_end()));
        }
    }
    None
}

fn npm_tree(output: &str) -> String {
    let l = limits();
    let mut root: Vec<String> = Vec::new();
    let mut nodes: Vec<(usize, String)> = Vec::new();
    let mut logs: Vec<String> = Vec::new();
    let mut deduped = 0usize;
    let mut empty = false;
    for line in norm(output).lines() {
        let t = line.trim_end();
        if t.trim().is_empty() || t.starts_with("Legend:") {
            continue;
        }
        if is_pm_log(t.trim_start()) {
            logs.push(squeeze_ws(t));
            continue;
        }
        match tree_node(t) {
            Some((_, "(empty)")) => empty = true,
            Some((d, text)) => {
                if text.ends_with(" deduped") {
                    deduped += 1;
                    continue;
                }
                nodes.push((d, text.to_string()));
            }
            None => root.push(squeeze_ws(t)),
        }
    }
    let mut out: Vec<String> = Vec::new();
    if empty && nodes.is_empty() {
        match root.last_mut() {
            Some(r) => r.push_str(" (empty)"),
            None => out.push("(empty)".into()),
        }
    }
    out.extend(root);

    // depth-aware cap: collapse the deepest levels first so every top-level dep survives
    let total = nodes.len();
    let max_depth = nodes.iter().map(|n| n.0).max().unwrap_or(0);
    let mut show_depth = max_depth;
    while show_depth > 1 && nodes.iter().filter(|n| n.0 <= show_depth).count() > l.list_max_lines {
        show_depth -= 1;
    }
    let kept: Vec<String> = nodes
        .iter()
        .filter(|n| n.0 <= show_depth)
        .map(|(d, s)| format!("{}{}", " ".repeat(*d), s))
        .collect();
    let depth_cut = total - kept.len();
    out.extend(cap_vec(kept, l.list_max_lines, "lines"));
    if depth_cut > 0 {
        out.push(more(depth_cut, &format!("deps at depth >{}", show_depth)))
    }
    if deduped > 0 {
        out.push(more(deduped, "deduped entries"))
    }
    out.extend(cap_vec(logs, l.max_diagnostics, "lines"));
    out.join("\n")
}

/// `npm ls --json`: `{name: {version, resolved, overridden, dependencies}}` →
/// `name: version` leaves and `name@version: {…}` nodes (drops resolved URLs and
/// `overridden: false`, keeps every other field such as `problems`/`invalid`).
fn npm_ls_json(output: &str) -> String {
    use serde_json::{Map, Value};
    fn simplify_deps(deps: &Map<String, Value>) -> Value {
        let mut out = Map::new();
        for (name, v) in deps {
            let Some(o) = v.as_object() else {
                out.insert(name.clone(), v.clone());
                continue;
            };
            let ver = o
                .get("version")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let mut extra = Map::new();
            for (k, val) in o {
                match k.as_str() {
                    "version" | "resolved" | "dependencies" => {}
                    "overridden" if val == &Value::Bool(false) => {}
                    _ => {
                        extra.insert(k.clone(), val.clone());
                    }
                }
            }
            let children = o
                .get("dependencies")
                .and_then(|d| d.as_object())
                .filter(|d| !d.is_empty());
            let key = if ver.is_empty() {
                name.clone()
            } else {
                format!("{}@{}", name, ver)
            };
            match (children, extra.is_empty()) {
                (None, true) => {
                    out.insert(
                        name.clone(),
                        Value::String(if ver.is_empty() { "?".into() } else { ver }),
                    );
                }
                (None, false) => {
                    if !ver.is_empty() {
                        extra.insert("version".into(), Value::String(ver));
                    }
                    out.insert(name.clone(), Value::Object(extra));
                }
                (Some(ch), true) => {
                    out.insert(key, simplify_deps(ch));
                }
                (Some(ch), false) => {
                    extra.insert("dependencies".into(), simplify_deps(ch));
                    out.insert(key, Value::Object(extra));
                }
            }
        }
        Value::Object(out)
    }
    match parse_json(output) {
        Some(docs) => docs
            .iter()
            .map(|doc| {
                let mut v = doc.clone();
                if let Some(o) = v.as_object_mut() {
                    if let Some(deps) = o.get("dependencies").and_then(|d| d.as_object()).cloned() {
                        o.insert("dependencies".into(), simplify_deps(&deps));
                    }
                }
                compact_json(&v)
            })
            .collect::<Vec<_>>()
            .join("\n---\n"),
        None => npm_tree(output),
    }
}

// ── npm run (scripts list / script output) ───────────────────────────────────

fn npm_scripts(output: &str) -> String {
    let l = limits();
    let mut pkg = String::new();
    let mut scripts: Vec<String> = Vec::new();
    let mut name: Option<String> = None;
    let mut other: Vec<String> = Vec::new();
    for line in norm(output).lines() {
        let t = line.trim_end();
        if t.trim().is_empty() {
            continue;
        }
        if let Some(rest) = t
            .strip_prefix("Lifecycle scripts included in ")
            .or_else(|| t.strip_prefix("Scripts available in "))
        {
            pkg = rest
                .split(" via")
                .next()
                .unwrap_or(rest)
                .trim_end_matches(':')
                .to_string();
            continue;
        }
        if t.starts_with("available via") {
            continue;
        }
        let indent = t.len() - t.trim_start().len();
        if indent >= 4
            || (indent >= 1 && name.is_some() && !t.trim_start().starts_with(char::is_alphanumeric))
        {
            if let Some(n) = name.take() {
                scripts.push(format!("  {}: {}", n, t.trim()));
                continue;
            }
        }
        if (1..4).contains(&indent) {
            if let Some(n) = name.take() {
                scripts.push(format!("  {}", n))
            }
            name = Some(t.trim().to_string());
            continue;
        }
        // pnpm/yarn style `  name` / `    cmd` handled above; anything else kept
        if !t.starts_with(' ') && is_pm_log(t) {
            other.push(t.to_string());
            continue;
        }
        other.push(t.to_string());
    }
    if let Some(n) = name.take() {
        scripts.push(format!("  {}", n))
    }
    if scripts.is_empty() {
        return generic(output);
    }
    let mut out = vec![if pkg.is_empty() {
        "scripts:".to_string()
    } else {
        format!("scripts ({}):", pkg)
    }];
    out.extend(cap_vec(scripts, l.list_max_lines, "scripts"));
    out.extend(cap_vec(other, l.passthrough_max_lines, "lines"));
    out.join("\n")
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum Tool {
    Jest,
    Vitest,
    Tsc,
    Next,
    Eslint,
    Prettier,
    Playwright,
    Biome,
    Generic,
}

/// Detect the tool behind a script from the banner command (if any) or the output.
fn detect_tool(cmd: Option<&str>, body: &str) -> Tool {
    if let Some(cmd) = cmd {
        let mut toks = cmd
            .split_whitespace()
            .filter(|t| !t.contains('=') || t.starts_with('-'))
            .peekable();
        // skip runner prefixes
        while let Some(t) = toks.peek() {
            if matches!(
                *t,
                "npx"
                    | "pnpm"
                    | "yarn"
                    | "bunx"
                    | "bun"
                    | "npm"
                    | "exec"
                    | "run"
                    | "dlx"
                    | "cross-env"
                    | "dotenv"
                    | "node"
            ) || t.starts_with('-')
                || t.contains('=')
            {
                toks.next();
            } else {
                break;
            }
        }
        if let Some(t) = toks.next() {
            let base = t.rsplit('/').next().unwrap_or(t);
            let base = base.strip_suffix(".js").unwrap_or(base);
            match base {
                "jest" => return Tool::Jest,
                "vitest" => return Tool::Vitest,
                "tsc" | "vue-tsc" => return Tool::Tsc,
                "next" => return Tool::Next,
                "eslint" => return Tool::Eslint,
                "prettier" => return Tool::Prettier,
                "playwright" => return Tool::Playwright,
                "biome" => return Tool::Biome,
                _ => {}
            }
        }
    }
    if body.contains("Test Suites:")
        || (body.contains("\nTests:") && (body.contains("PASS ") || body.contains("FAIL ")))
    {
        return Tool::Jest;
    }
    if body.contains(" RUN  v") || body.contains("Test Files ") {
        return Tool::Vitest;
    }
    if body
        .lines()
        .any(|x| re_tsc_plain().is_match(x.trim()) || re_tsc_pretty().is_match(x.trim()))
    {
        return Tool::Tsc;
    }
    if body.contains("Route (app)")
        || body.contains("Route (pages)")
        || body.contains("Creating an optimized production build")
    {
        return Tool::Next;
    }
    if body.contains("Running ") && body.contains(" tests using ") {
        return Tool::Playwright;
    }
    if body.lines().any(|x| re_eslint_summary().is_match(x)) {
        return Tool::Eslint;
    }
    if body.contains("[warn] ") && body.contains("Prettier")
        || body.contains("All matched files use Prettier")
    {
        return Tool::Prettier;
    }
    if body.contains(" ━━") && (body.contains("Checked ") || body.contains(" lint/")) {
        return Tool::Biome;
    }
    Tool::Generic
}

fn run_tool(tool: Tool, args: &[&str], body: &str) -> String {
    match tool {
        Tool::Jest => filter_jest(body),
        Tool::Vitest => filter_vitest(body),
        Tool::Tsc => filter_tsc(body),
        Tool::Next => filter_next(if args.is_empty() { &["build"] } else { args }, body),
        Tool::Eslint => filter_eslint(body),
        Tool::Prettier => filter_prettier(body),
        Tool::Playwright => filter_playwright(body),
        Tool::Biome => filter_biome(args, body),
        Tool::Generic => generic(body),
    }
}

/// Collapse npm's lifecycle-failure block to one line; keep every other log line.
fn collapse_pm_logs(logs: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut script: Option<String> = None;
    let mut code: Option<String> = None;
    let mut cmd: Option<String> = None;
    let mut log_path: Option<String> = None;
    for l in logs {
        let t = l
            .trim_start_matches("npm ERR! ")
            .trim_start_matches("npm error ")
            .trim_start_matches("npm error")
            .trim();
        if let Some(i) = t.find("Lifecycle script `") {
            let s = &t[i + 18..];
            script = Some(s.split('`').next().unwrap_or(s).to_string());
            continue;
        }
        if t.starts_with("code ") && script.is_some() {
            code = Some(t[5..].to_string());
            continue;
        }
        if let Some(c) = t.strip_prefix("command ") {
            if c != "failed" {
                cmd = Some(c.to_string())
            }
            continue;
        }
        if (t.starts_with("path ") || t.starts_with("workspace ") || t.starts_with("location "))
            && script.is_some()
        {
            continue;
        }
        if let Some(p) = t.strip_prefix("A complete log of this run can be found in:") {
            log_path = Some(p.trim().to_string());
            continue;
        }
        if t.is_empty() {
            continue;
        }
        out.push(l);
    }
    if let Some(s) = script {
        let mut line = format!("npm error: script `{}` failed", s);
        if let Some(c) = code {
            line.push_str(&format!(" (exit {})", c))
        }
        if let Some(c) = cmd {
            line.push_str(&format!(" — {}", c))
        }
        out.push(line);
    }
    if let Some(p) = log_path {
        out.push(format!("npm log: {}", p))
    }
    out
}

fn npm_script_output(args: &[&str], output: &str) -> String {
    let text = norm(output);
    let mut banner_cmd: Option<String> = None;
    let mut body: Vec<&str> = Vec::new();
    let mut logs: Vec<String> = Vec::new();
    let mut banner_seen = 0usize;
    for line in text.lines() {
        let t = line.trim_end();
        if banner_seen < 2 && body.is_empty() {
            if t.trim().is_empty() {
                continue;
            }
            if let Some(b) = t.strip_prefix("> ") {
                banner_seen += 1;
                if banner_seen == 2 {
                    banner_cmd = Some(b.to_string())
                }
                continue;
            }
            if let Some(b) = t.strip_prefix("$ ") {
                banner_cmd = Some(b.to_string());
                banner_seen = 2;
                continue;
            }
        }
        if is_pm_log(t)
            && (t.starts_with("npm ")
                || t.starts_with(" ERR_PNPM")
                || t.starts_with("ERR_PNPM")
                || t.starts_with(" WARN "))
        {
            logs.push(squeeze_ws(t));
            continue;
        }
        body.push(line);
    }
    let body_s = body.join("\n");
    let tool = detect_tool(banner_cmd.as_deref(), &body_s);
    let tool_args: Vec<&str> = banner_cmd
        .as_deref()
        .map(|c| c.split_whitespace().skip(1).collect())
        .unwrap_or_default();
    let mut out = run_tool(
        tool,
        if tool_args.is_empty() {
            args
        } else {
            &tool_args
        },
        &body_s,
    );
    let logs = collapse_pm_logs(logs);
    if !logs.is_empty() {
        if !out.is_empty() {
            out.push('\n')
        }
        out.push_str(&cap_vec(logs, limits().max_diagnostics, "lines").join("\n"));
    }
    out
}

// ── npm install / pnpm install / yarn install ────────────────────────────────

re!(
    re_npm_added,
    r"^(added|removed|changed|up to date|audited|found)\b"
);
re!(
    re_vulns,
    r"^\d+ vulnerabilit(y|ies)|^found \d+ vulnerabilit"
);
re!(
    re_deprecated,
    r"^(?:npm (?:WARN|warn) deprecated|warning) (\S+?)(?:: | )"
);
re!(re_pnpm_pkg, r"^[+-] \S+ \S+");

fn npm_install(output: &str) -> String {
    let l = limits();
    let mut summary: Vec<String> = Vec::new();
    let mut deprecated: Vec<String> = Vec::new();
    let mut warns: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut pkgs: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    let mut notice_update: Option<String> = None;
    let mut skip_after_vuln_hint = false;
    let mut in_err = false;
    for line in norm(output).lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            in_err = false;
            continue;
        }
        // progress redraws / spinners / bars
        if tt.starts_with('[')
            && (tt.contains("idealTree")
                || tt.contains("reify")
                || tt.contains("###")
                || tt.contains("...."))
        {
            continue;
        }
        if tt
            .chars()
            .all(|c| matches!(c, '+' | '-' | '.' | '#' | '=' | ' ' | '⠀'..='⣿'))
        {
            continue;
        }
        if tt.starts_with("Progress:") && !tt.ends_with("done") {
            continue;
        }
        if tt.starts_with('[')
            && tt.contains('/')
            && tt.contains(']')
            && (tt.contains("Resolving")
                || tt.contains("Fetching")
                || tt.contains("Linking")
                || tt.contains("Building"))
        {
            continue;
        }
        // funding / audit hints
        if tt.contains("looking for funding")
            || tt.starts_with("run `npm fund`")
            || tt.starts_with("Run `npm audit`")
        {
            continue;
        }
        if tt.starts_with("To address all issues")
            || tt.starts_with("To address issues that do not require attention")
        {
            skip_after_vuln_hint = true;
            continue;
        }
        if skip_after_vuln_hint
            && (tt.starts_with("npm audit fix") || tt.starts_with("Some issues need review"))
        {
            continue;
        }
        skip_after_vuln_hint = false;
        // npm notice: keep the "new version" line once, drop its changelog/update hints
        if let Some(n) = tt.strip_prefix("npm notice") {
            let n = n.trim();
            if n.is_empty()
                || n.starts_with("Changelog:")
                || n.starts_with("To update run:")
                || n.starts_with("Run `npm install -g")
            {
                continue;
            }
            if n.contains("version of npm available") {
                if notice_update.is_none() {
                    notice_update = Some(format!("npm notice: {}", n))
                }
                continue;
            }
            warns.push(format!("npm notice {}", n));
            continue;
        }
        if let Some(c) = re_deprecated().captures(tt) {
            if tt.starts_with("npm") || tt.contains("deprecated") {
                deprecated.push(c[1].to_string());
                continue;
            }
        }
        if tt.starts_with("npm ERR!")
            || tt.starts_with("npm error")
            || tt.starts_with(" ERR_PNPM")
            || tt.starts_with("ERR_PNPM")
            || tt.starts_with("error ")
            || tt.starts_with("error:")
            || tt.starts_with("✖")
        {
            errors.push(squeeze_ws(tt));
            in_err = true;
            continue;
        }
        if in_err && (t.starts_with(' ') || t.starts_with('\t')) {
            errors.push(squeeze_ws(tt));
            continue;
        }
        in_err = false;
        if tt.starts_with("npm WARN")
            || tt.starts_with("npm warn")
            || tt.starts_with(" WARN ")
            || tt.starts_with("WARN ")
            || tt.starts_with("warning ")
            || tt.starts_with("warn ")
            || tt.starts_with("warn:")
        {
            warns.push(squeeze_ws(tt));
            continue;
        }
        if re_vulns().is_match(tt) {
            summary.push(tt.to_string());
            continue;
        }
        if re_npm_added().is_match(tt) {
            summary.push(
                tt.replace(" packages", "")
                    .replace(" package", "")
                    .replace(", and ", ", "),
            );
            continue;
        }
        if re_pnpm_pkg().is_match(tt) || tt.starts_with("installed ") {
            pkgs.push(tt.to_string());
            continue;
        }
        if tt.ends_with("dependencies:")
            || tt == "Packages: +0"
            || tt.starts_with("Packages:")
            || tt.starts_with("Progress:")
            || tt.starts_with("Done in")
            || tt.starts_with("Lockfile is up to date")
            || tt.starts_with("Already up to date")
            || tt.starts_with("success ")
            || tt.starts_with("info ")
            || tt.starts_with("yarn install v")
            || tt.starts_with("Saved lockfile")
            || tt.contains("packages installed")
            || tt.starts_with("Checked ")
        {
            summary.push(squeeze_ws(tt));
            continue;
        }
        other.push(tt.to_string());
    }
    let mut out: Vec<String> = Vec::new();
    out.extend(cap_vec(pkgs, l.list_max_lines, "packages"));
    out.extend(summary);
    if !deprecated.is_empty() {
        let n = deprecated.len();
        let shown: Vec<String> = deprecated.iter().take(l.json_max_array).cloned().collect();
        let mut s = format!("{} deprecated: {}", n, shown.join(", "));
        if n > shown.len() {
            s.push_str(&format!(", {}", more(n - shown.len(), "deprecated")))
        }
        out.push(s);
    }
    out.extend(cap_vec(warns, l.max_diagnostics, "warnings"));
    out.extend(cap_vec(errors, l.passthrough_max_lines, "error lines"));
    if let Some(n) = notice_update {
        out.push(n)
    }
    out.extend(cap_vec(other, l.passthrough_max_lines, "lines"));
    out.join("\n")
}

// ── npm config list / outdated / audit ───────────────────────────────────────

fn npm_config_list(output: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut node_v = None;
    let mut npm_v = None;
    for line in norm(output).lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(c) = t.strip_prefix(';') {
            let c = c.trim();
            if let Some(v) = c.strip_prefix("node version = ") {
                node_v = Some(v.to_string())
            }
            if let Some(v) = c.strip_prefix("npm version = ") {
                npm_v = Some(v.to_string())
            }
            continue;
        }
        out.push(squeeze_ws(t));
    }
    let env = match (node_v, npm_v) {
        (Some(n), Some(m)) => format!("node {}, npm {}", n, m),
        (Some(n), None) => format!("node {}", n),
        (None, Some(m)) => format!("npm {}", m),
        _ => String::new(),
    };
    if out.is_empty() {
        let mut s = "npm config: no user-level settings (defaults only)".to_string();
        if !env.is_empty() {
            s.push_str(&format!("; {}", env))
        }
        return s;
    }
    let l = limits();
    let mut res = cap_vec(out, l.list_max_lines, "settings");
    if !env.is_empty() {
        res.push(env)
    }
    res.join("\n")
}

fn npm_outdated(output: &str) -> String {
    let l = limits();
    let mut rows: Vec<String> = Vec::new();
    let mut header_done = false;
    for line in norm(output).lines() {
        let t = line.trim_end();
        if t.trim().is_empty() {
            continue;
        }
        if t.starts_with("Package") && t.contains("Current") {
            if !header_done {
                rows.push("Package Current Wanted Latest Depended-by".into());
                header_done = true
            }
            continue;
        }
        let cols: Vec<&str> = t.split_whitespace().collect();
        if cols.len() >= 5 && cols[4].starts_with("node_modules/") {
            // drop Location when it is the obvious node_modules/<name>
            let mut keep: Vec<&str> = cols[..4].to_vec();
            if cols[4] != format!("node_modules/{}", cols[0]) {
                keep.push(cols[4])
            }
            keep.extend(&cols[5..]);
            rows.push(keep.join(" "));
        } else {
            rows.push(squeeze_ws(t));
        }
    }
    if rows.is_empty() {
        return "npm outdated: all up to date".into();
    }
    cap_vec(rows, l.list_max_lines, "packages").join("\n")
}

fn npm_audit(output: &str) -> String {
    let l = limits();
    let mut out: Vec<String> = Vec::new();
    let mut skip_hint = false;
    for line in norm(output).lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() || tt.starts_with("# npm audit report") {
            continue;
        }
        if tt.starts_with("To address all issues")
            || tt.starts_with("To address issues that do not require")
        {
            skip_hint = true;
            continue;
        }
        if skip_hint
            && (tt.starts_with("npm audit fix") || tt.starts_with("Some issues need review"))
        {
            continue;
        }
        skip_hint = false;
        if tt.starts_with("Run `npm audit`") {
            continue;
        }
        out.push(squeeze_ws(t));
    }
    if out.is_empty() {
        return "npm audit: no vulnerabilities".into();
    }
    cap_vec(out, l.passthrough_max_lines, "lines").join("\n")
}

// ─── npx ──────────────────────────────────────────────────────────────────────

pub(crate) fn filter_npx(args: &[&str], output: &str) -> String {
    // skip npx's own flags (and the value of -p/--package/-c)
    let mut i = 0;
    while i < args.len() && args[i].starts_with('-') {
        if matches!(args[i], "-p" | "--package" | "-c" | "--call") {
            i += 2
        } else {
            i += 1
        }
    }
    let Some(tool) = args.get(i) else {
        return generic(output);
    };
    let rest = &args[i + 1..];
    // `@scope/pkg@ver` / `pkg@ver` / `./node_modules/.bin/tsc` → bare tool name
    let base = tool.rsplit('/').next().unwrap_or(tool);
    // `base` already had any `@scope/` prefix stripped by the `/`-split above, so a
    // leading `@` here and not doesn't change how the version suffix is split off.
    let name = base.split('@').next().unwrap_or(base);
    match name {
        "tsc" | "vue-tsc" => filter_tsc(output),
        "eslint" => filter_eslint(output),
        "biome" => filter_biome(rest, output),
        "prettier" => filter_prettier(output),
        "jest" => filter_jest(output),
        "vitest" => filter_vitest(output),
        "playwright" => filter_playwright(output),
        "next" => filter_next(rest, output),
        // prisma lives in db.rs (not importable from here) — conservative passthrough
        _ => generic(output),
    }
}

// ─── playwright ───────────────────────────────────────────────────────────────

re!(
    re_pw_result,
    r"^\s*([✓✘×✗\-])\s+\d+\s+(?:\[([^\]]+)\]\s+›\s+)?(\S+?):(\d+):(\d+)\s+›\s+(.*?)(?:\s+\((\d+(?:\.\d+)?m?s)\))?\s*$"
);
re!(re_pw_failure, r"^\s*(\d+)\)\s+(.*?)\s*─*\s*$");
re!(
    re_pw_count,
    r"^\s*(\d+) (passed|failed|flaky|skipped|did not run|interrupted)(?:\s+\(([^)]+)\))?\s*$"
);

pub(crate) fn filter_playwright(output: &str) -> String {
    let l = limits();
    let text = norm(output);
    let mut per_file: BTreeMap<String, (usize, usize)> = BTreeMap::new(); // passed, skipped
    let mut failed_lines: Vec<String> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut time = String::new();
    let mut body: Option<Body> = None;
    let mut in_fail = false;
    let mut failures = 0usize;
    let mut cut = 0usize;
    let mut in_count_list = false;
    let flush = |body: &mut Option<Body>, out: &mut Vec<String>| {
        if let Some(b) = body.take() {
            out.extend(b.finish("    "))
        }
    };

    for line in text.lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if let Some(c) = re_pw_count().captures(t) {
            flush(&mut body, &mut out);
            in_fail = false;
            counts.insert(c[2].to_string(), c[1].parse().unwrap_or(0));
            if let Some(m) = c.get(3) {
                time = m.as_str().to_string()
            }
            in_count_list = true;
            continue;
        }
        if in_count_list && t.starts_with("    ") {
            continue;
        } // repeated failure titles under "N failed"
        in_count_list = false;
        if tt.starts_with("Running ") && tt.contains(" tests using ")
            || tt.starts_with("Running ") && tt.contains(" test using ")
        {
            continue;
        }
        if tt.starts_with("To open last HTML report")
            || tt.starts_with("npx playwright show-report")
            || tt.starts_with("Serving HTML report")
        {
            continue;
        }
        if let Some(c) = re_pw_result().captures(t) {
            flush(&mut body, &mut out);
            in_fail = false;
            let file = c[3].to_string();
            let e = per_file.entry(file.clone()).or_default();
            match &c[1] {
                "✓" => e.0 += 1,
                "-" => e.1 += 1,
                _ => {
                    let proj = c
                        .get(2)
                        .map(|m| format!("[{}] ", m.as_str()))
                        .unwrap_or_default();
                    let time = c
                        .get(7)
                        .map(|m| format!(" ({})", m.as_str()))
                        .unwrap_or_default();
                    failed_lines.push(format!(
                        "✘ {}{}:{}:{} › {}{}",
                        proj, file, &c[4], &c[5], &c[6], time
                    ));
                }
            }
            continue;
        }
        if let Some(c) = re_pw_failure().captures(t) {
            if tt.contains(" › ") || tt.contains(":") {
                flush(&mut body, &mut out);
                failures += 1;
                if failures > l.test_max_failures {
                    cut += 1;
                    in_fail = false;
                    continue;
                }
                out.push(format!("{}) {}", &c[1], squeeze_ws(&c[2])));
                body = Some(Body::new());
                in_fail = true;
                continue;
            }
        }
        if in_fail {
            if t.starts_with(' ') || t.starts_with('\t') {
                if tt.chars().all(|c| c == '─' || c == ' ') {
                    continue;
                }
                if let Some(b) = body.as_mut() {
                    let cleaned = tt.trim_end_matches(['─', ' ']);
                    b.push(cleaned, tt.starts_with("at "), "    ");
                }
                continue;
            }
            flush(&mut body, &mut out);
            in_fail = false;
        }
        other.push(tt.to_string());
    }
    flush(&mut body, &mut out);

    let mut res: Vec<String> = Vec::new();
    let mut file_lines: Vec<String> = Vec::new();
    for (f, (p, s)) in &per_file {
        if *p + *s == 0 {
            continue;
        }
        let mut s_ = format!("✓ {} {} passed", f, p);
        if *s > 0 {
            s_.push_str(&format!(", {} skipped", s))
        }
        file_lines.push(s_);
    }
    res.extend(cap_vec(file_lines, l.list_max_lines, "files"));
    res.extend(cap_vec(failed_lines, l.test_max_failures, "failed tests"));
    res.extend(out);
    if cut > 0 {
        res.push(more(cut, "failures"))
    }
    res.extend(cap_vec(other, l.log_tail, "lines"));

    if counts.is_empty() && per_file.is_empty() {
        return if res.is_empty() {
            String::new()
        } else {
            res.join("\n")
        };
    }
    let g = |k: &str| counts.get(k).copied().unwrap_or(0);
    let (p, f) = if counts.is_empty() {
        (per_file.values().map(|v| v.0).sum(), failures)
    } else {
        (g("passed"), g("failed"))
    };
    let mut parts = vec![format!("{} passed", p), format!("{} failed", f)];
    for k in ["flaky", "skipped", "did not run", "interrupted"] {
        if g(k) > 0 {
            parts.push(format!("{} {}", g(k), k))
        }
    }
    let mut trailer = format!("playwright: {}", parts.join(", "));
    if !time.is_empty() {
        trailer.push_str(&format!(" ({})", time))
    }
    res.push(trailer);
    res.join("\n")
}

// ─── next ─────────────────────────────────────────────────────────────────────

re!(re_size_unit, r"(\d) (k?B|MB)");
re!(re_next_legend, r"^\s*([○●ƒλℇ◐])\s+\(([^)]+)\)");

fn next_progress(t: &str) -> bool {
    let t = t
        .trim_start_matches(['-', ' '])
        .trim_start_matches("info")
        .trim_start_matches(['-', ' ']);
    t.starts_with("Creating an optimized production build")
        || t.starts_with("Collecting page data")
        || t.starts_with("Generating static pages")
        || t.starts_with("Finalizing page optimization")
        || t.starts_with("Collecting build traces")
        || t.starts_with("Linting and checking validity of types")
        || t.starts_with("Checking validity of types")
        || t.starts_with("Skipping validation of types")
        || t.starts_with("Skipping linting")
        || t.starts_with("Loaded env from")
}

pub(crate) fn filter_next(args: &[&str], output: &str) -> String {
    match args.first().copied() {
        Some("build") => next_build(output),
        Some("lint") => filter_eslint(output),
        _ => generic(output),
    }
}

fn next_build(output: &str) -> String {
    let l = limits();
    let mut out: Vec<String> = Vec::new();
    let mut routes: Vec<String> = Vec::new();
    let mut legend: Vec<String> = Vec::new();
    let mut in_table = false;
    for line in norm(output).lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if let Some(c) = re_next_legend().captures(t) {
            in_table = false;
            legend.push(format!("{} {}", &c[1], &c[2]));
            continue;
        }
        if tt.starts_with("Route (") {
            in_table = true;
            let head = squeeze_ws(tt).replace("First Load JS", "First-Load-JS");
            out.push(format!("{}:", head));
            continue;
        }
        if in_table {
            let body = tt.trim_start_matches(['┌', '├', '└', '│', ' ']);
            if body.is_empty() {
                continue;
            }
            if body.starts_with('+')
                || body.starts_with("chunks/")
                || body.starts_with("other shared")
                || body.starts_with("css/")
                || body.starts_with("ƒ Middleware")
            {
                routes.push(format!(
                    " {}",
                    re_size_unit().replace_all(&squeeze_ws(body), "$1$2")
                ));
                continue;
            }
            if body
                .chars()
                .next()
                .map(|c| !c.is_ascii_alphabetic())
                .unwrap_or(false)
                || body.starts_with('/')
            {
                routes.push(format!(
                    " {}",
                    re_size_unit().replace_all(&squeeze_ws(body), "$1$2")
                ));
                continue;
            }
            in_table = false;
        }
        if next_progress(tt) || is_gutter(t) {
            continue;
        }
        out.push(squeeze_ws(tt));
    }
    if !routes.is_empty() {
        let n = routes.len();
        let shown: Vec<String> = routes.into_iter().take(l.list_max_lines).collect();
        let cut = n - shown.len();
        out.extend(shown);
        if cut > 0 {
            out.push(more(cut, "routes"))
        }
    }
    if !legend.is_empty() {
        out.push(format!("legend: {}", legend.join(", ")))
    }
    if out.is_empty() {
        return "next build: ok".into();
    }
    cap_vec(out, l.passthrough_max_lines, "lines").join("\n")
}

// ─── bun ──────────────────────────────────────────────────────────────────────

re!(re_bun_pass, r"^\s*(?:✓|\(pass\))\s");
re!(
    re_bun_fail,
    r"^\s*(?:✗|\(fail\))\s+(.*?)(?:\s+\[[\d.]+m?s\])?\s*$"
);
re!(re_bun_skip, r"^\s*(?:»|\(skip\)|\(todo\))\s");
re!(re_bun_summary, r"^\s*(\d+) (pass|fail|skip|todo)\s*$");
re!(
    re_bun_ran,
    r"^Ran (\d+) tests? across (\d+) files?\.?\s*\[([\d.]+m?s)\]"
);

pub(crate) fn filter_bun(args: &[&str], output: &str) -> String {
    match args.first().copied() {
        Some("test") => bun_test(output),
        Some("install") | Some("i") | Some("add") | Some("a") | Some("remove") | Some("rm")
        | Some("update") | Some("ci") => npm_install(output),
        Some("build") => bun_build(output),
        Some("run") | Some("x") => npm_script_output(&args[1..], output),
        _ => generic(output),
    }
}

fn bun_test(output: &str) -> String {
    let l = limits();
    let mut out: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut ran: Option<(usize, usize, String)> = None;
    let mut cur_file: Option<String> = None;
    let mut file_printed = false;
    let mut body: Option<Body> = None;
    let mut failures = 0usize;
    let mut cut = 0usize;
    let flush = |body: &mut Option<Body>, out: &mut Vec<String>| {
        if let Some(b) = body.take() {
            out.extend(b.finish("    "))
        }
    };
    for line in norm(output).lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if tt.starts_with("bun test v") {
            continue;
        }
        if let Some(c) = re_bun_summary().captures(t) {
            flush(&mut body, &mut out);
            counts.insert(c[2].to_string(), c[1].parse().unwrap_or(0));
            continue;
        }
        if tt.ends_with("expect() calls") {
            continue;
        }
        if let Some(c) = re_bun_ran().captures(tt) {
            ran = Some((
                c[1].parse().unwrap_or(0),
                c[2].parse().unwrap_or(0),
                c[3].to_string(),
            ));
            continue;
        }
        if !t.starts_with(' ') && tt.ends_with(':') && !tt.contains(' ') {
            flush(&mut body, &mut out);
            cur_file = Some(tt.trim_end_matches(':').to_string());
            file_printed = false;
            continue;
        }
        if re_bun_pass().is_match(t) || re_bun_skip().is_match(t) {
            flush(&mut body, &mut out);
            continue;
        }
        if let Some(c) = re_bun_fail().captures(t) {
            flush(&mut body, &mut out);
            failures += 1;
            if failures > l.test_max_failures {
                cut += 1;
                continue;
            }
            if let (Some(f), false) = (&cur_file, file_printed) {
                out.push(format!("{}:", f));
                file_printed = true
            }
            out.push(format!("  ✗ {}", &c[1]));
            body = Some(Body::new());
            continue;
        }
        if body.is_some() && (t.starts_with(' ') || t.starts_with('\t')) {
            if let Some(b) = body.as_mut() {
                b.push(t, tt.starts_with("at "), "    ")
            }
            continue;
        }
        flush(&mut body, &mut out);
        other.push(tt.to_string());
    }
    flush(&mut body, &mut out);
    if cut > 0 {
        out.push(more(cut, "failures"))
    }
    out.extend(cap_vec(other, l.log_tail, "lines"));
    if counts.is_empty() && ran.is_none() {
        return if out.is_empty() {
            String::new()
        } else {
            out.join("\n")
        };
    }
    let g = |k: &str| counts.get(k).copied().unwrap_or(0);
    let mut parts = vec![
        format!("{} passed", g("pass")),
        format!("{} failed", g("fail")),
    ];
    if g("skip") > 0 {
        parts.push(format!("{} skipped", g("skip")))
    }
    if g("todo") > 0 {
        parts.push(format!("{} todo", g("todo")))
    }
    let mut trailer = format!("bun test: {}", parts.join(", "));
    if let Some((n, f, t)) = ran {
        trailer.push_str(&format!(
            " ({}, {}, {})",
            plural(n, "test"),
            plural(f, "file"),
            t
        ))
    }
    out.push(trailer);
    out.join("\n")
}

fn bun_build(output: &str) -> String {
    let l = limits();
    let lines: Vec<String> = norm(output)
        .lines()
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .map(squeeze_ws)
        .collect();
    if lines.is_empty() {
        return "bun build: ok".into();
    }
    cap_vec(lines, l.passthrough_max_lines, "lines").join("\n")
}

// ─── deno ─────────────────────────────────────────────────────────────────────

re!(
    re_deno_test_line,
    r"^(.*?) \.\.\. (ok|FAILED|ignored)(?: \(([^)]+)\))?\s*$"
);
re!(
    re_deno_result,
    r"^(ok|FAILED) \|\s*(\d+) passed(?:\s*\((\d+) steps?\))? \|\s*(\d+) failed(?:\s*\((\d+) steps?\))?(?: \|\s*(\d+) ignored)?.*?\(([^)]+)\)"
);
re!(re_deno_check_err, r"^(TS\d+) \[(ERROR|WARN)\]: (.*)$");
re!(re_deno_lint_rule, r"^\((\S+)\) (.*)$");

pub(crate) fn filter_deno(args: &[&str], output: &str) -> String {
    match args.first().copied() {
        Some("test") => deno_test(output),
        Some("check") | Some("lint") => deno_check_lint(output),
        Some("fmt") => deno_fmt(output),
        _ => generic(output),
    }
}

fn deno_test(output: &str) -> String {
    let l = limits();
    let mut out: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    let mut result: Option<String> = None;
    let mut section = "";
    let mut body: Option<Body> = None;
    let mut failures = 0usize;
    let mut cut = 0usize;
    let flush = |body: &mut Option<Body>, out: &mut Vec<String>| {
        if let Some(b) = body.take() {
            out.extend(b.finish("  "))
        }
    };
    for line in norm(output).lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if tt.starts_with("Check file://")
            || tt.starts_with("running ") && tt.contains(" tests from ")
            || tt.starts_with("running ") && tt.contains(" test from ")
        {
            continue;
        }
        if tt == "ERRORS" || tt == "FAILURES" {
            flush(&mut body, &mut out);
            section = if tt == "ERRORS" { "errors" } else { "failures" };
            continue;
        }
        if let Some(c) = re_deno_result().captures(tt) {
            flush(&mut body, &mut out);
            let mut parts = vec![format!("{} passed", &c[2]), format!("{} failed", &c[4])];
            if let Some(i) = c.get(6) {
                if i.as_str() != "0" {
                    parts.push(format!("{} ignored", i.as_str()))
                }
            }
            result = Some(format!("deno test: {} ({})", parts.join(", "), &c[7]));
            continue;
        }
        if tt == "error: Test failed" {
            continue;
        }
        if section == "failures" {
            continue;
        } // duplicate of the ERRORS headers
        if section == "errors" {
            if tt.contains(" => ") && !t.starts_with(' ') {
                flush(&mut body, &mut out);
                failures += 1;
                if failures > l.test_max_failures {
                    cut += 1;
                    continue;
                }
                out.push(tt.to_string());
                body = Some(Body::new());
                continue;
            }
            if let Some(b) = body.as_mut() {
                if tt.starts_with("[Diff]") {
                    continue;
                }
                let frame = tt.starts_with("at ");
                // source echo (`throw new AssertionError(message);`) precedes its caret line
                if tt.starts_with("throw ") {
                    continue;
                }
                b.push(t, frame, "  ");
            }
            continue;
        }
        if let Some(c) = re_deno_test_line().captures(tt) {
            if &c[2] == "ok" || &c[2] == "ignored" {
                continue;
            }
            // FAILED lines are repeated with details in the ERRORS section
            if !output.contains("ERRORS") {
                out.push(format!("✗ {}", &c[1]))
            }
            continue;
        }
        other.push(tt.to_string());
    }
    flush(&mut body, &mut out);
    if cut > 0 {
        out.push(more(cut, "failures"))
    }
    out.extend(cap_vec(other, l.log_tail, "lines"));
    if let Some(r) = result {
        out.push(r)
    }
    out.join("\n")
}

fn deno_check_lint(output: &str) -> String {
    let l = limits();
    let mut out: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    let mut errors = 0usize;
    let mut warnings = 0usize;
    let mut checked: Option<String> = None;
    let mut found: Option<String> = None;
    let mut body: Option<Body> = None;
    let flush = |body: &mut Option<Body>, out: &mut Vec<String>| {
        if let Some(b) = body.take() {
            out.extend(b.finish("  "))
        }
    };
    for line in norm(output).lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if tt.starts_with("Check file://") || tt.starts_with("Check ") && tt.contains("://") {
            continue;
        }
        if let Some(c) = re_deno_check_err().captures(tt) {
            flush(&mut body, &mut out);
            if &c[2] == "ERROR" {
                errors += 1
            } else {
                warnings += 1
            }
            if errors + warnings > l.max_diagnostics {
                continue;
            }
            out.push(format!("{} {}", &c[1], &c[3]));
            body = Some(Body::new());
            continue;
        }
        if let Some(c) = re_deno_lint_rule().captures(tt) {
            flush(&mut body, &mut out);
            errors += 1;
            if errors > l.max_diagnostics {
                continue;
            }
            out.push(format!("({}) {}", &c[1], &c[2]));
            body = Some(Body::new());
            continue;
        }
        if tt.starts_with("Checked ") {
            checked = Some(tt.trim_end_matches('.').to_string());
            continue;
        }
        if tt.starts_with("Found ") && (tt.contains("problem") || tt.contains("error")) {
            found = Some(tt.to_string());
            continue;
        }
        if tt.starts_with("error: Type checking failed") || tt.starts_with("error: Found ") {
            continue;
        }
        if let Some(b) = body.as_mut() {
            if t.starts_with(' ')
                || tt.starts_with("at ")
                || tt.starts_with("hint:")
                || tt.starts_with("docs:")
            {
                b.push(t, tt.starts_with("at "), "  ");
                continue;
            }
        }
        flush(&mut body, &mut out);
        other.push(tt.to_string());
    }
    flush(&mut body, &mut out);
    let shown = errors + warnings;
    if shown > l.max_diagnostics {
        out.push(more(shown - l.max_diagnostics, "diagnostics"))
    }
    out.extend(cap_vec(other, l.passthrough_max_lines, "lines"));
    let mut trailer = if errors + warnings == 0 && found.is_none() {
        "deno: ok".to_string()
    } else {
        format!(
            "deno: {}, {}",
            plural(errors, "problem"),
            plural(warnings, "warning")
        )
    };
    if let Some(c) = checked {
        trailer.push_str(&format!(" ({})", c.to_lowercase()))
    }
    out.push(trailer);
    out.join("\n")
}

fn deno_fmt(output: &str) -> String {
    let l = limits();
    let mut files: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    let mut summary: Option<String> = None;
    for line in norm(output).lines() {
        let tt = line.trim();
        if tt.is_empty() {
            continue;
        }
        if let Some(f) = tt.strip_prefix("from ") {
            files.push(f.trim_end_matches(':').to_string());
            continue;
        }
        if tt.starts_with("error: Found ") {
            summary = Some(tt.trim_start_matches("error: ").to_string());
            continue;
        }
        if tt.starts_with("Checked ") {
            summary = Some(tt.to_string());
            continue;
        }
        if tt.starts_with('|')
            || tt
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
        {
            continue;
        } // diff body
        other.push(tt.to_string());
    }
    let mut out = Vec::new();
    let had_files = !files.is_empty();
    if had_files {
        out.push(format!(
            "deno fmt: {} not formatted:",
            plural(files.len(), "file")
        ));
        out.extend(cap_vec(files, l.list_max_lines, "files"));
    }
    out.extend(cap_vec(other, l.passthrough_max_lines, "lines"));
    match summary {
        Some(s) if !had_files => out.push(format!("deno fmt: {}", s)),
        Some(s) => out.push(s),
        None if out.is_empty() => out.push("deno fmt: ok".into()),
        None => {}
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsc_groups_by_file_and_counts_codes() {
        let out = filter_tsc(
            "src/app.ts(12,5): error TS2304: Cannot find name 'foo'.\nsrc/app.ts(20,9): error TS2345: Argument of type 'string' is not assignable.\nsrc/db.ts(3,1): error TS2304: Cannot find name 'bar'.\nFound 3 errors in 2 files.\n",
        );
        assert!(out.contains("src/app.ts"), "{}", out);
        assert!(out.contains("TS2304"), "{}", out);
        assert!(out.contains("TS2345"), "{}", out);
        assert!(out.contains("src/db.ts"), "{}", out);
        assert!(out.contains('3'), "{}", out);
        assert_eq!(filter_tsc(""), "tsc: ok");
    }

    #[test]
    fn eslint_and_prettier_shapes() {
        let es = filter_eslint(
            "/app/src/a.js\n  12:5  error  Unexpected console statement  no-console\n  20:1  warning  Missing semicolon  semi\n\n/app/src/b.js\n  3:9  error  'x' is assigned but never used  no-unused-vars\n\n✖ 3 problems (2 errors, 1 warning)\n",
        );
        assert!(es.contains("a.js"), "{}", es);
        assert!(es.contains("no-console"), "{}", es);
        assert!(es.contains("b.js"), "{}", es);
        assert!(es.contains("no-unused-vars"), "{}", es);
        let pr = filter_prettier(
            "Checking formatting...\n[warn] src/a.ts\n[warn] src/b.ts\n[warn] Code style issues found in 2 files.\n",
        );
        assert!(pr.contains("src/a.ts") && pr.contains("src/b.ts"), "{}", pr);
        assert!(
            filter_prettier("Checking formatting...\nAll matched files use Prettier code style!\n")
                .to_lowercase()
                .contains("ok")
                || filter_prettier("All matched files use Prettier code style!\n")
                    .contains("Prettier")
        );
    }

    #[test]
    fn jest_and_vitest_summarise_and_keep_failures() {
        let jest = filter_jest(
            "PASS src/a.test.ts\nFAIL src/b.test.ts\n  ● Calc › adds\n\n    expect(received).toBe(expected)\n\n    Expected: 4\n    Received: 3\n\n      12 |   expect(add(1,2)).toBe(4);\n         |          ^\n      at Object.<anonymous> (src/b.test.ts:12:22)\n\nTest Suites: 1 failed, 1 passed, 2 total\nTests:       1 failed, 5 passed, 6 total\nTime:        1.234 s\n",
        );
        assert!(
            jest.contains("Calc › adds") || jest.contains("Calc"),
            "{}",
            jest
        );
        assert!(jest.contains("Expected: 4"), "{}", jest);
        assert!(jest.contains("src/b.test.ts:12:22"), "{}", jest);
        assert!(jest.contains('5') && jest.contains('1'), "{}", jest);
        assert!(!jest.contains("|          ^"), "code frame kept: {}", jest);
        let vi = filter_vitest(
            " ✓ src/a.test.ts (3 tests) 12ms\n ❯ src/b.test.ts (2 tests | 1 failed) 8ms\n   × adds numbers\n     → expected 3 to be 4\n\n Test Files  1 failed | 1 passed (2)\n      Tests  1 failed | 4 passed (5)\n   Duration  1.20s\n",
        );
        assert!(vi.contains("adds numbers"), "{}", vi);
        assert!(vi.contains("expected 3 to be 4"), "{}", vi);
        assert!(vi.contains('4'), "{}", vi);
    }

    #[test]
    fn npm_ls_keeps_every_dependency_or_counts_it() {
        let raw = "prism-hub-backend@0.1.0 /app\n├── @nestjs/cli@11.0.21\n├─┬ @nestjs/common@11.1.19\n│ ├── reflect-metadata@0.2.2\n│ └── rxjs@7.8.2 deduped\n└── zod@3.25.76\n";
        let out = filter_npm(&["ls", "--all"], raw);
        for dep in [
            "@nestjs/cli@11.0.21",
            "@nestjs/common@11.1.19",
            "reflect-metadata@0.2.2",
            "zod@3.25.76",
        ] {
            assert!(out.contains(dep), "missing {}\n{}", dep, out);
        }
        // box drawing is decoration; indentation carries the same nesting
        assert!(!out.contains('├'), "{}", out);
        assert!(!out.contains('│'), "{}", out);
    }

    #[test]
    fn npm_ls_cap_is_announced() {
        let mut raw = String::from("app@1.0.0 /app\n");
        for i in 0..400 {
            raw.push_str(&format!("├── pkg{}@1.0.{}\n", i, i));
        }
        let out = filter_npm(&["ls"], &raw);
        assert!(has_truncation(&out), "{}", out);
    }

    #[test]
    fn npm_install_and_run_and_config() {
        let inst = filter_npm(
            &["install"],
            "npm warn deprecated inflight@1.0.6: This module is not supported\nnpm warn deprecated glob@7.2.3: Glob versions prior to v9 are no longer supported\n\nadded 412 packages, and audited 413 packages in 12s\n\n52 packages are looking for funding\n  run `npm fund` for details\n\n3 vulnerabilities (1 moderate, 2 high)\n",
        );
        assert!(
            inst.contains("added 412") && inst.contains("audited 413"),
            "{}",
            inst
        );
        assert!(inst.contains("3 vulnerabilities"), "{}", inst);
        assert!(!inst.contains("npm fund"), "{}", inst);
        let run = filter_npm(
            &["run"],
            "Lifecycle scripts included in app@0.1.0:\n  start\n    nest start\n  test\n    jest\navailable via `npm run`:\n  build\n    nest build\n",
        );
        assert!(
            run.contains("start") && run.contains("nest start"),
            "{}",
            run
        );
        assert!(
            run.contains("build") && run.contains("nest build"),
            "{}",
            run
        );
        assert!(!run.contains("Lifecycle scripts included"), "{}", run);
        let cfg = filter_npm(
            &["config", "list"],
            "; environment-related config\n\n; no_proxy = \"localhost\"\nregistry = \"https://registry.npmjs.org/\"\n",
        );
        assert!(cfg.contains("registry"), "{}", cfg);
    }

    #[test]
    fn npx_delegates_by_tool_and_playwright_next_bun_deno() {
        let tsc = filter_npx(
            &["tsc", "--noEmit"],
            "src/a.ts(1,1): error TS1005: ';' expected.\n",
        );
        assert!(tsc.contains("TS1005"), "{}", tsc);
        let pw = filter_playwright(
            "  ✓  1 [chromium] › tests/a.spec.ts:3:1 › loads (1.2s)\n  ✘  2 [chromium] › tests/b.spec.ts:9:1 › submits (0.8s)\n    Error: expect(received).toBe(expected)\n      at tests/b.spec.ts:12:20\n\n  1 failed\n  1 passed (2.5s)\n",
        );
        assert!(pw.contains("b.spec.ts"), "{}", pw);
        assert!(pw.contains("failed"), "{}", pw);
        let next = filter_next(
            &["build"],
            "   ▲ Next.js 15.0.0\n\n   Creating an optimized production build ...\n ✓ Compiled successfully\n\nRoute (app)                    Size     First Load JS\n┌ ○ /                          1.2 kB          89 kB\n└ ○ /about                     0.9 kB          88 kB\n",
        );
        assert!(next.contains('/'), "{}", next);
        assert!(
            next.contains("Compiled") || next.contains("89 kB"),
            "{}",
            next
        );
        let bun = filter_bun(
            &["test"],
            " 12 pass\n  1 fail\n Ran 13 tests across 3 files.\n",
        );
        assert!(bun.contains("12") && bun.contains('1'), "{}", bun);
        let deno = filter_deno(
            &["check", "main.ts"],
            "Check file:///app/main.ts\nerror: TS2304 [ERROR]: Cannot find name 'x'.\n",
        );
        assert!(
            deno.contains("TS2304") || deno.contains("Cannot find name"),
            "{}",
            deno
        );
    }

    #[test]
    fn empty_and_odd_inputs_never_panic() {
        for raw in [
            "",
            "\n\n\n",
            "├──\n",
            "│\n",
            "✓\n",
            "── ✓ 🎉\n",
            "a\r\nb\r\n",
            "{\n",
            "npm ERR! boom\n",
        ] {
            let _ = filter_tsc(raw);
            let _ = filter_eslint(raw);
            let _ = filter_prettier(raw);
            let _ = filter_jest(raw);
            let _ = filter_vitest(raw);
            let _ = filter_npm(&["ls"], raw);
            let _ = filter_npm(&["install"], raw);
            let _ = filter_npm(&["run", "build"], raw);
            let _ = filter_npx(&["tsc"], raw);
            let _ = filter_playwright(raw);
            let _ = filter_next(&["build"], raw);
            let _ = filter_bun(&["test"], raw);
            let _ = filter_deno(&["test"], raw);
            let _ = filter_biome(&["check"], raw);
        }
    }

    #[test]
    fn npm_errors_are_never_dropped() {
        let out = filter_npm(
            &["install"],
            "npm ERR! code ERESOLVE\nnpm ERR! ERESOLVE unable to resolve dependency tree\nnpm ERR! A complete log of this run can be found in: /home/u/.npm/_logs/x.log\n",
        );
        assert!(out.contains("ERESOLVE"), "{}", out);
        assert!(out.contains("unable to resolve dependency tree"), "{}", out);
    }
}
