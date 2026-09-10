//! Cloud CLI filters: `aws`, `gcloud`, `az`.
//!
//! Output *shape* is detected before the subcommand is consulted:
//!   * JSON (aws default, `--format=json`, az default) → pruned + [`compact_json`]
//!   * aws `--output table` box tables → `key: value` / `a|b|c` rows, borders dropped
//!   * YAML (gcloud describe, `--output yaml`) → parsed and rendered like JSON
//!   * fixed-width tables (gcloud lists, `az -o table`) → `a|b|c` rows sliced at the
//!     header's column starts so empty cells keep their position
//!   * tab-separated (`--output text`, `-o tsv`) → tabs squeezed
//! Per-subcommand text parsers handle the rest (s3 ls/cp/sync, logs tail, builds,
//! run deploy, config list, az login, …). Every cap comes from [`limits`]; every cut is
//! announced with [`more`]. The only silent drops are decoration: spinner frames,
//! progress bars, docker layer chatter, usage/hint boilerplate.

use super::common::*;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::OnceLock;

// ─── shared ───────────────────────────────────────────────────────────────────

/// CRLF / bare CR (progress bars redrawn in place) → LF so parsers see one record per line.
fn normalize(s: &str) -> String {
    if s.contains('\r') { s.replace("\r\n", "\n").replace('\r', "\n") } else { s.to_string() }
}

/// First `n` command words, skipping flags and the values of known option-with-value flags.
fn cmd_words<'a>(args: &[&'a str], valued: &[&str], n: usize) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() && out.len() < n {
        let a = args[i];
        if a.starts_with('-') {
            if !a.contains('=') && valued.contains(&a) { i += 1 }
        } else {
            out.push(a);
        }
        i += 1;
    }
    out
}

/// Lower-cased value of the first `--output x` / `--output=x` / `-o x` style flag in `names`.
fn out_format(args: &[&str], names: &[&str]) -> Option<String> {
    for (i, a) in args.iter().enumerate() {
        for n in names {
            if let Some(v) = a.strip_prefix(n).and_then(|r| r.strip_prefix('=')) {
                return Some(v.to_ascii_lowercase());
            }
            if a == n {
                return args.get(i + 1).map(|v| v.to_ascii_lowercase());
            }
        }
    }
    None
}

fn is_empty_val(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

fn is_scalar_val(v: &Value) -> bool {
    !matches!(v, Value::Array(a) if !a.is_empty()) && !matches!(v, Value::Object(o) if !o.is_empty())
}

/// Unquoted string for identifiers we place in prose (`scalar_str` would quote numeric-looking ids).
fn plain(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => scalar_str(other),
    }
}

/// `[{Key: k, Value: v}, …]` (aws Tags/TagSet/TagList, lower-case variants) → `k=v, k2=v2`.
fn fold_tags(a: &[Value]) -> Option<String> {
    if a.is_empty() { return None }
    let mut parts = Vec::with_capacity(a.len());
    for it in a {
        let o = it.as_object()?;
        if o.len() != 2 { return None }
        let k = o.get("Key").or_else(|| o.get("key"))?.as_str()?;
        let v = o.get("Value").or_else(|| o.get("value"))?;
        if !is_scalar_val(v) { return None }
        parts.push(format!("{}={}", k, plain(v)));
    }
    Some(parts.join(", "))
}

/// Cloud JSON pre-transform: drop boto `ResponseMetadata`, fold tag lists, prune keys whose
/// value is null/""/[]/{}. Inside arrays of objects the prune is per *column* (a key is dropped
/// only when empty in every row) so uniform arrays still render as `k1|k2` tables.
/// `dropped` counts removed keys so the caller can announce them.
fn cloud_transform(v: &mut Value, dropped: &mut usize) {
    match v {
        Value::Object(o) => {
            if o.remove("ResponseMetadata").is_some() { *dropped += 1 }
            for val in o.values_mut() { cloud_transform(val, dropped) }
            let before = o.len();
            o.retain(|_, val| !is_empty_val(val));
            *dropped += before - o.len();
        }
        Value::Array(a) => {
            if let Some(s) = fold_tags(a) {
                *v = Value::String(s);
                return;
            }
            if !a.is_empty() && a.iter().all(Value::is_object) {
                let mut keys: BTreeSet<String> = BTreeSet::new();
                for it in a.iter_mut() {
                    let o = it.as_object_mut().unwrap();
                    if o.remove("ResponseMetadata").is_some() { *dropped += 1 }
                    for (k, val) in o.iter_mut() {
                        cloud_transform(val, dropped);
                        keys.insert(k.clone());
                    }
                }
                for k in keys {
                    if a.iter().all(|it| it.get(&k).map_or(true, is_empty_val)) {
                        for it in a.iter_mut() {
                            if it.as_object_mut().unwrap().remove(&k).is_some() { *dropped += 1 }
                        }
                    }
                }
            } else {
                for it in a.iter_mut() { cloud_transform(it, dropped) }
            }
        }
        _ => {}
    }
}

/// Render JSON documents through the cloud transform + [`compact_json`], announcing pruned keys.
fn render_docs(docs: &[Value]) -> String {
    let mut dropped = 0usize;
    let parts: Vec<String> = docs
        .iter()
        .map(|d| {
            let mut v = d.clone();
            cloud_transform(&mut v, &mut dropped);
            compact_json(&v)
        })
        .collect();
    let mut s = parts.join("\n---\n");
    if dropped > 0 {
        if !s.is_empty() { s.push('\n') }
        s.push_str(&more(dropped, "empty fields"));
    }
    s
}

/// Split stderr preface (warnings, notices) from a JSON body that starts further down.
fn split_json(output: &str) -> (Vec<&str>, Option<Vec<Value>>) {
    let lines: Vec<&str> = output.lines().collect();
    if let Some(p) = lines.iter().position(|l| {
        let t = l.trim_start();
        t.starts_with('{') || t.starts_with('[')
    }) {
        let rest = lines[p..].join("\n");
        if let Some(docs) = parse_json(&rest) {
            return (lines[..p].to_vec(), Some(docs));
        }
    }
    (lines, None)
}

// ── YAML ──────────────────────────────────────────────────────────────────────

fn yaml_line(l: &str) -> bool {
    let t = l.trim_start();
    let t = t.strip_prefix("- ").unwrap_or(t);
    if t == "-" || t.starts_with("---") { return true }
    match t.find(':') {
        Some(i) if i > 0 => {
            let key = &t[..i];
            let rest = &t[i + 1..];
            key.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '"' | '\'' | '@'))
                && (rest.is_empty() || rest.starts_with(' '))
        }
        _ => false,
    }
}

/// Parse YAML output (one or more documents) into JSON values. Guarded by a cheap shape
/// check so prose and error text never get "parsed" as a YAML scalar.
fn yaml_docs(s: &str) -> Option<Vec<Value>> {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 2 { return None }
    let first = lines[0].trim_start();
    if first.starts_with("ERROR") || first.starts_with("WARNING") || first.starts_with("An error") {
        return None;
    }
    let yamlish = lines.iter().filter(|l| yaml_line(l)).count();
    if yamlish * 2 < lines.len() { return None }
    let mut out = Vec::new();
    for doc in serde_yaml::Deserializer::from_str(s) {
        let v = Value::deserialize(doc).ok()?;
        if !(v.is_object() || v.is_array()) { return None }
        out.push(v);
    }
    if out.is_empty() { None } else { Some(out) }
}

// ── fixed-width tables (gcloud, az -o table, kubectl-style) ───────────────────

/// Char indices where a cell starts: column 0, or any non-space after ≥2 spaces.
fn col_starts(line: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut spaces = 2usize;
    for (i, c) in line.chars().enumerate() {
        if c == ' ' {
            spaces += 1;
        } else {
            if spaces >= 2 { starts.push(i) }
            spaces = 0;
        }
    }
    starts
}

fn slice_cols(line: &str, starts: &[usize]) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut cells = Vec::with_capacity(starts.len());
    for (i, &s) in starts.iter().enumerate() {
        let e = starts.get(i + 1).copied().unwrap_or(chars.len()).min(chars.len());
        let s = s.min(e);
        cells.push(chars[s..e].iter().collect::<String>().trim().to_string());
    }
    cells
}

fn is_dash_rule(l: &str) -> bool {
    let t = l.trim();
    !t.is_empty() && t.chars().all(|c| c == '-' || c == ' ') && t.starts_with('-')
}

fn is_upper_cell(c: &str) -> bool {
    !c.is_empty()
        && c.chars().next().map_or(false, |f| f.is_ascii_uppercase())
        && c.chars().all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || matches!(ch, '_' | '.' | '/' | '-' | ' ' | '(' | ')'))
}

/// A header row: ≥2 columns and either all-UPPERCASE cells (gcloud) or a dashed underline (az).
fn is_table_header(line: &str, next: Option<&str>) -> bool {
    let starts = col_starts(line);
    if starts.len() < 2 { return false }
    if next.map_or(false, is_dash_rule) { return true }
    slice_cols(line, &starts).iter().all(|c| is_upper_cell(c))
}

fn join_pipe(cells: &[String]) -> String {
    let mut c: Vec<String> = cells.iter().map(|x| x.replace('|', "\\|")).collect();
    while c.len() > 1 && c.last().map_or(false, |x| x.is_empty()) { c.pop(); } // trailing empties are unambiguous
    c.join("|")
}

/// Header (+ optional underline) + rows → pipe rows, capped at `list_max_lines` data rows.
fn fixed_table(lines: &[&str]) -> Vec<String> {
    let header = lines[0];
    let mut starts = col_starts(header);
    let mut body = &lines[1..];
    if let Some(u) = body.first() {
        if is_dash_rule(u) {
            starts = col_starts(u); // underline gives exact column starts
            body = &body[1..];
        }
    }
    let mut out = vec![join_pipe(&slice_cols(header, &starts))];
    let rows: Vec<String> = body.iter().map(|l| join_pipe(&slice_cols(l, &starts))).collect();
    out.extend(cap_vec(rows, limits().list_max_lines, "rows"));
    out
}

/// Walk blank-separated blocks; render any fixed-width table found, pass other lines through.
fn render_blocks(lines: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim().is_empty() {
            out.push(String::new());
            i += 1;
            continue;
        }
        let mut j = i;
        while j < lines.len() && !lines[j].trim().is_empty() { j += 1 }
        let block: Vec<&str> = lines[i..j].iter().map(|s| s.trim_end()).collect();
        let hdr = (0..block.len().saturating_sub(1))
            .find(|&p| is_table_header(block[p], block.get(p + 1).copied()));
        match hdr {
            // need at least one data row besides header (+ underline)
            Some(p) if block.len() - p >= 2 && !(block.len() - p == 2 && is_dash_rule(block[p + 1])) => {
                out.extend(block[..p].iter().map(|s| s.to_string()));
                out.extend(fixed_table(&block[p..]));
            }
            _ => out.extend(block.iter().map(|s| s.to_string())),
        }
        i = j;
    }
    out
}

// ── aws --output table (box tables) ───────────────────────────────────────────

fn is_box_border(l: &str) -> bool {
    let t = l.trim();
    t.len() >= 2 && t.contains('-') && t.chars().all(|c| matches!(c, '-' | '+' | '|'))
}

/// `+----+` box tables → titles as `Title:`, vertical blocks as `key: value`,
/// header+rows as `a|b|c`; nesting depth (`||`) becomes indentation.
fn aws_box_table(output: &str) -> Option<String> {
    let lines: Vec<&str> = output.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 3 || !lines.iter().any(|l| is_box_border(l)) { return None }
    if !lines.iter().all(|l| is_box_border(l) || (l.starts_with('|') && l.ends_with('|'))) { return None }

    struct Block { depth: usize, rows: Vec<Vec<String>> }
    let mut blocks: Vec<Block> = Vec::new();
    let mut cur: Option<Block> = None;
    for l in &lines {
        if is_box_border(l) {
            if let Some(b) = cur.take() { blocks.push(b) }
            continue;
        }
        let depth = l.chars().take_while(|c| *c == '|').count();
        let cells: Vec<String> = l.trim_matches('|').split('|').map(|c| c.trim().to_string()).collect();
        match cur.as_mut() {
            Some(b) if b.depth == depth && b.rows[0].len() == cells.len() => b.rows.push(cells),
            _ => {
                if let Some(b) = cur.take() { blocks.push(b) }
                cur = Some(Block { depth, rows: vec![cells] });
            }
        }
    }
    if let Some(b) = cur.take() { blocks.push(b) }

    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < blocks.len() {
        let b = &blocks[i];
        let pad = " ".repeat(b.depth.saturating_sub(1));
        let width = b.rows[0].len();
        let next_is_data = blocks.get(i + 1).map_or(false, |n| n.depth == b.depth && n.rows[0].len() == width);
        if width == 1 {
            for r in &b.rows { out.push(format!("{}{}:", pad, r[0])) }
        } else if b.rows.len() == 1 && next_is_data {
            // header row, then the data block
            out.push(format!("{}{}", pad, join_pipe(&b.rows[0])));
            for r in &blocks[i + 1].rows { out.push(format!("{}{}", pad, join_pipe(r))) }
            i += 1;
        } else if width == 2 {
            for r in &b.rows { out.push(format!("{}{}: {}", pad, r[0], r[1])) }
        } else {
            for r in &b.rows { out.push(format!("{}{}", pad, join_pipe(r))) }
        }
        i += 1;
    }
    Some(cap_vec(out, limits().list_max_lines, "rows").join("\n"))
}

// ── logs ──────────────────────────────────────────────────────────────────────

/// Line template for dedupe: every digit → `#` so timestamps/counters/ids don't defeat grouping.
/// Digit-run *width* is kept (`10`≠`100`) so only same-shaped lines collapse.
fn template_of(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect()
}

/// Consecutive lines with the same template collapse to the first one + `(×N)`.
fn dedupe_template(lines: Vec<String>) -> Vec<String> {
    let mut out: Vec<(String, String, usize)> = Vec::new(); // (line, template, count)
    for l in lines {
        let t = template_of(&l);
        match out.last_mut() {
            Some((_, prev, n)) if *prev == t => *n += 1,
            _ => out.push((l, t, 1)),
        }
    }
    out.into_iter().map(|(l, _, n)| if n > 1 { format!("{}  (×{})", l, n) } else { l }).collect()
}

fn is_log_alert(l: &str) -> bool {
    let u = l.to_ascii_uppercase();
    ["ERROR", "WARN", "FATAL", "EXCEPTION", "TRACEBACK", "PANIC", "FAIL", "CRITICAL"].iter().any(|k| u.contains(k))
}

/// Keep the last `max` lines; lines from the dropped head that satisfy `pin` are kept too,
/// followed by one marker for the rest.
fn tail_pinned(lines: Vec<String>, max: usize, pin: impl Fn(&str) -> bool) -> Vec<String> {
    if lines.len() <= max { return lines }
    let cut = lines.len() - max;
    let mut out: Vec<String> = lines[..cut].iter().filter(|l| pin(l)).cloned().collect();
    let dropped = cut - out.len();
    if dropped > 0 { out.push(more(dropped, "lines")) }
    out.extend_from_slice(&lines[cut..]);
    out
}

fn fmt_epoch_ms(ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ms).map(|d| d.format("%Y-%m-%dT%H:%M:%S.%3fZ").to_string())
}

// ── text fallbacks ────────────────────────────────────────────────────────────

/// Tab-separated (`--output text`, `-o tsv`): squeeze whitespace, cap.
fn tsv_text(output: &str) -> String {
    let lines: Vec<String> = output.lines().map(squeeze_ws).filter(|l| !l.is_empty()).collect();
    cap_vec(lines, limits().list_max_lines, "lines").join("\n")
}

fn looks_tsv(output: &str) -> bool {
    let mut n = 0usize;
    let mut tabs = 0usize;
    for l in output.lines().filter(|l| !l.trim().is_empty()) {
        n += 1;
        if l.contains('\t') { tabs += 1 }
    }
    n > 0 && tabs * 2 >= n
}

/// Shape-detected fallback for any cloud CLI: box table → YAML → TSV → fixed tables/text.
fn structured_text(output: &str) -> String {
    if let Some(t) = aws_box_table(output) { return t }
    if let Some(docs) = yaml_docs(output) { return render_docs(&docs) }
    if looks_tsv(output) { return tsv_text(output) }
    let lines: Vec<String> = output.lines().map(|l| l.trim_end().to_string()).collect();
    text_lines(render_blocks(&lines))
}

/// Blank runs collapsed *before* dedupe (so blanks never become `(×N)`), identical
/// neighbours counted, capped at `passthrough_max_lines`.
fn text_lines(lines: Vec<String>) -> String {
    let text = collapse_blank(&lines.join("\n"));
    let deduped = dedupe_consecutive(text.lines().map(str::to_string));
    cap_vec(deduped, limits().passthrough_max_lines, "lines").join("\n")
}

// ─── AWS ──────────────────────────────────────────────────────────────────────

const AWS_VALUED: &[&str] = &[
    "--profile", "--region", "--output", "--endpoint-url", "--query", "--cli-read-timeout",
    "--cli-connect-timeout", "--ca-bundle", "--color", "--cli-binary-format",
];

const AWS_ERROR_PREFIXES: &[&str] = &[
    "An error occurred", "Unable to locate credentials", "aws: error", "fatal error", "Could not connect",
    "usage: aws", "Invalid choice", "Partial credentials", "The config profile", "Error when retrieving",
    "Unknown options", "Invalid endpoint", "Error parsing parameter", "Unknown output type",
    "Expecting value", "argument ", "Error:",
];

fn is_aws_error(output: &str) -> bool {
    output.lines().any(|l| {
        let t = l.trim_start();
        AWS_ERROR_PREFIXES.iter().any(|p| t.starts_with(p))
    })
}

/// Errors verbatim; `usage:` boilerplate dropped; the 300-service "valid choices" list counted.
fn aws_error_text(output: &str) -> String {
    const BOILER: &[&str] = &[
        "usage: aws [options]", "To see help text, you can run:", "aws help", "aws <command> help",
        "aws <command> <subcommand> help", "Note: AWS CLI version 2",
    ];
    let mut out: Vec<String> = Vec::new();
    let mut choices = 0usize;
    let mut in_choices = false;
    for raw in output.lines() {
        let t = raw.trim();
        if t.is_empty() { continue }
        if in_choices {
            choices += 1;
            continue;
        }
        if BOILER.iter().any(|b| t.starts_with(b)) { continue }
        out.push(raw.trim_end().to_string());
        if t.ends_with("valid choices are:") { in_choices = true }
    }
    if choices > 0 { out.push(more(choices, "choices")) }
    cap_vec(out, limits().passthrough_max_lines, "lines").join("\n")
}

/// `aws s3 ls`: `date time size key`, `PRE dir/` → `dir/`, summary lines kept.
fn aws_s3_ls(output: &str) -> String {
    let mut lines = Vec::new();
    for raw in output.lines() {
        let t = squeeze_ws(raw);
        if t.is_empty() { continue }
        lines.push(t.strip_prefix("PRE ").map(str::to_string).unwrap_or(t));
    }
    cap_vec(lines, limits().ls_max_entries, "entries").join("\n")
}

/// `aws s3 cp/sync/mv/rm`: progress bars dropped, transfer lines capped, everything else kept.
fn aws_s3_transfer(output: &str) -> String {
    let l = limits();
    let mut out: Vec<String> = Vec::new();
    let mut transfers = 0usize;
    let mut dropped = 0usize;
    let mut progress = 0usize;
    for raw in output.lines() {
        let t = squeeze_ws(raw);
        if t.is_empty() { continue }
        if t.starts_with("Completed ") && (t.contains(" remaining") || t.contains(" with ")) {
            progress += 1;
            continue;
        }
        let op = t.strip_prefix("(dryrun) ").unwrap_or(&t);
        let op = op.split(':').next().unwrap_or("");
        if matches!(op, "upload" | "download" | "copy" | "move" | "delete") {
            transfers += 1;
            if transfers > l.list_max_lines {
                dropped += 1;
                continue;
            }
        }
        out.push(t);
    }
    if dropped > 0 { out.push(more(dropped, "transfers")) }
    if out.is_empty() && progress > 0 {
        out.push(format!("aws s3: {} progress lines, no transfers listed", progress));
    }
    out.join("\n")
}

fn is_iso_ts(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 19 && b[4] == b'-' && b[7] == b'-' && b[10] == b'T' && b[13] == b':'
}

/// `aws logs tail`: `<ts> <stream> <msg>` → `<ts> <msg>`, template-deduped, tailed with alerts pinned.
fn aws_logs_tail(output: &str) -> String {
    let mut lines = Vec::new();
    for raw in output.lines() {
        let t = raw.trim_end();
        if t.is_empty() { continue }
        let mut it = t.splitn(3, ' ');
        match (it.next(), it.next(), it.next()) {
            (Some(ts), Some(_stream), Some(msg)) if is_iso_ts(ts) => lines.push(format!("{} {}", ts, msg.trim_start())),
            _ => lines.push(t.to_string()),
        }
    }
    tail_pinned(dedupe_template(lines), limits().log_tail, is_log_alert).join("\n")
}

/// `aws logs get-log-events` / `filter-log-events` JSON → `<iso ts> <message>` lines
/// (stream names / ingestion times dropped), then the remaining keys (tokens…) compactly.
fn aws_log_events(docs: &[Value]) -> String {
    let obj = match docs {
        [Value::Object(o)] if o.get("events").map_or(false, Value::is_array) => o,
        _ => return render_docs(docs),
    };
    let events = obj["events"].as_array().unwrap();
    let mut lines = Vec::with_capacity(events.len());
    for e in events {
        let ts = e.get("timestamp").and_then(Value::as_i64).and_then(fmt_epoch_ms);
        let msg = e.get("message").and_then(Value::as_str).unwrap_or("").trim_end();
        lines.push(match ts {
            Some(ts) => format!("{} {}", ts, msg),
            None => msg.to_string(),
        });
    }
    let mut out = vec![format!("{} events", events.len())];
    out.extend(tail_pinned(dedupe_template(lines), limits().log_tail, is_log_alert));
    let mut rest = Value::Object(obj.clone());
    rest.as_object_mut().unwrap().remove("events");
    let mut dropped = 0usize;
    cloud_transform(&mut rest, &mut dropped);
    if rest.as_object().map_or(false, |o| !o.is_empty()) { out.push(compact_json(&rest)) }
    out.join("\n")
}

/// `aws sts get-caller-identity` → one line.
fn aws_sts_identity(docs: &[Value]) -> String {
    if let [Value::Object(o)] = docs {
        if let (Some(a), Some(arn), Some(u)) = (o.get("Account"), o.get("Arn"), o.get("UserId")) {
            return format!("Account: {}  Arn: {}  UserId: {}", plain(a), plain(arn), plain(u));
        }
    }
    render_docs(docs)
}

pub(crate) fn filter_aws(args: &[&str], output: &str) -> String {
    let output = normalize(output);
    if output.trim().is_empty() { return String::new() }
    let w = cmd_words(args, AWS_VALUED, 2);
    let (svc, op) = (w.first().copied().unwrap_or(""), w.get(1).copied().unwrap_or(""));

    let (preface, docs) = split_json(&output);
    if let Some(docs) = docs {
        let body = match (svc, op) {
            ("sts", "get-caller-identity") => aws_sts_identity(&docs),
            ("logs", "get-log-events" | "filter-log-events") => aws_log_events(&docs),
            _ => render_docs(&docs),
        };
        let mut out: Vec<String> = preface.iter().filter(|l| !l.trim().is_empty()).map(|l| l.trim_end().to_string()).collect();
        out.push(body);
        return out.join("\n");
    }
    match (svc, op) {
        ("s3", "ls") => aws_s3_ls(&output),
        ("s3", "cp" | "sync" | "mv" | "rm") => aws_s3_transfer(&output),
        ("logs", "tail") => aws_logs_tail(&output),
        _ if is_aws_error(&output) => aws_error_text(&output),
        _ if out_format(args, &["--output"]).as_deref() == Some("text") => tsv_text(&output),
        _ => structured_text(&output),
    }
}

// ─── GCLOUD ───────────────────────────────────────────────────────────────────

const GCLOUD_VALUED: &[&str] = &[
    "--project", "--account", "--configuration", "--impersonate-service-account", "--billing-project",
    "--format", "--filter", "--sort-by", "--limit", "--page-size", "--verbosity", "--flags-file",
    "--access-token-file", "--region", "--zone", "--platform",
];

const GCLOUD_HINTS: &[&str] = &[
    "$ gcloud ", "To set the active account, run:", "To take a quick anonymous survey",
    "Updates are available for some Google Cloud CLI components", "installed components, run:",
    "To update all installed components", "To install or remove components", "To update your SDK installation",
    "To view your current configuration",
];

fn is_spinner(t: &str) -> bool {
    match t.chars().next() {
        Some(c) if ('\u{2800}'..='\u{28FF}').contains(&c) => true, // braille spinner frames
        Some('.') => t.chars().all(|c| c == '.'),
        Some(c) if "═║╔╗╚╝─│┌┐└┘├┤╠╣".contains(c) => t.chars().all(|c| "═║╔╗╚╝─│┌┐└┘├┤╠╣ ".contains(c)),
        _ => false,
    }
}

/// Drop gcloud decoration: spinner frames, dot progress, box banners, update/survey hints.
fn gcloud_denoise<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    lines
        .into_iter()
        .map(|l| l.trim_end())
        .filter(|l| {
            let t = l.trim_start();
            !(is_spinner(t) || GCLOUD_HINTS.iter().any(|h| t.starts_with(h)))
        })
        .map(str::to_string)
        .collect()
}

/// Generic gcloud text: denoise, YAML if it is YAML (describe), fixed tables → pipe rows.
fn gcloud_text(output: &str) -> String {
    let lines = gcloud_denoise(output.lines());
    // stderr notices (WARNING:/ERROR:) precede the payload; keep them, detect the rest
    let split = lines.iter().position(|l| {
        let t = l.trim_start();
        !(t.is_empty() || t.starts_with("WARNING:") || t.starts_with("ERROR:") || t.starts_with("Updated property"))
    });
    let (notices, body) = match split {
        Some(p) => (&lines[..p], &lines[p..]),
        None => (&lines[..], &lines[lines.len()..]),
    };
    let body_text = body.join("\n");
    let mut out: Vec<String> = notices.iter().filter(|l| !l.trim().is_empty()).cloned().collect();
    if body.is_empty() {
        return out.join("\n");
    }
    if let Some(docs) = yaml_docs(&body_text) {
        out.push(render_docs(&docs));
        return out.join("\n");
    }
    if looks_tsv(&body_text) {
        out.push(tsv_text(&body_text));
        return out.join("\n");
    }
    out.extend(render_blocks(body));
    text_lines(out)
}

/// `gcloud config list` → `section: k=v, k2=v2`.
fn gcloud_config_list(output: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut section: Option<(String, Vec<String>)> = None;
    let flush = |section: &mut Option<(String, Vec<String>)>, out: &mut Vec<String>| {
        if let Some((name, kv)) = section.take() {
            out.push(if kv.is_empty() { format!("{}:", name) } else { format!("{}: {}", name, kv.join(", ")) });
        }
    };
    for l in gcloud_denoise(output.lines()) {
        let t = l.trim();
        if t.is_empty() { continue }
        if t.starts_with('[') && t.ends_with(']') && t.len() > 2 {
            flush(&mut section, &mut out);
            section = Some((t[1..t.len() - 1].to_string(), Vec::new()));
            continue;
        }
        if let (Some(s), Some((k, v))) = (section.as_mut(), t.split_once(" = ")) {
            s.1.push(format!("{}={}", k.trim(), v.trim()));
            continue;
        }
        flush(&mut section, &mut out);
        if let Some(rest) = t.strip_prefix("Your active configuration is: ") {
            out.push(format!("active configuration: {}", rest.trim_matches(|c| c == '[' || c == ']')));
        } else {
            out.push(t.to_string());
        }
    }
    flush(&mut section, &mut out);
    cap_vec(out, limits().passthrough_max_lines, "lines").join("\n")
}

fn is_dots_done(t: &str) -> bool {
    t.contains("...") && (t.ends_with("done") || t.ends_with("done."))
}

/// `gcloud run deploy` (and friends): successful step lines fold into `✓ N steps`; failed /
/// pending steps, the deployed-revision line, the URL and every ERROR/WARNING line stay.
fn gcloud_deploy(output: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut pending = 0usize;
    for l in gcloud_denoise(output.lines()) {
        let t = l.trim();
        if t.is_empty() { continue }
        if t.starts_with('✓') || t.starts_with('✔') || is_dots_done(t) {
            pending += 1;
            continue;
        }
        if t == "Done." || t == "Deploying..." || t.starts_with("Deploying new service...") || t == "Building using Dockerfile and deploying container to Cloud Run service..." {
            continue;
        }
        if pending > 0 {
            out.push(format!("✓ {} steps", pending));
            pending = 0;
        }
        out.push(t.to_string());
    }
    if pending > 0 { out.push(format!("✓ {} steps", pending)) }
    cap_vec(dedupe_consecutive(out), limits().passthrough_max_lines, "lines").join("\n")
}

fn build_noise_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // docker layer/progress chatter, gsutil copy chatter — after an optional `Step #N: ` prefix
        // (docker's ` ---> ` lines carry a second space, hence `\s*`)
        Regex::new(
            r#"^(Step #\d+(?: - "[^"]*")?:\s*)?(?:[0-9a-f]{12}: (?:Preparing|Waiting|Pushing|Pushed|Layer already exists|Mounted from |Verifying Checksum|Download complete|Pull complete|Already exists|Pulling fs layer|Downloading|Extracting)|Sending build context to Docker daemon|Removing intermediate container |Already have image|[-/|\\] \[\d+ files\]|---> |Fetching storage object: gs://|Copying gs://|Operation completed over \d+ objects)"#,
        )
        .unwrap()
    })
}

fn step_prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"^(Step #\d+(?: - "[^"]*")?): ?(.*)$"#).unwrap())
}

fn starting_step_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"^Starting (Step #\d+(?: - "[^"]*")?)$"#).unwrap())
}

fn is_build_status(l: &str) -> bool {
    let t = l.trim_start();
    is_log_alert(l)
        || t.starts_with("Step #")
        || t.starts_with("Starting Step")
        || t.starts_with("Finished Step")
        || t.starts_with("starting build")
        || t.starts_with("Created [")
        || t.starts_with("Logs are available")
        || t.starts_with("Successfully")
        || matches!(t, "FETCHSOURCE" | "BUILD" | "PUSH" | "DONE" | "REMOTE BUILD OUTPUT")
        || t.contains("SUCCESS") || t.contains("TIMEOUT") || t.contains("CANCELLED")
        || t.starts_with("ID ") || t.starts_with("ID|")
}

/// `gcloud builds submit|log`: docker layer chatter dropped, repeated `Step #N:` prefixes
/// factored into one header (`Starting Step #N - "x":` when present) + indented lines,
/// consecutive duplicates counted, trailing status table → pipe rows, tailed with
/// status/error lines pinned.
fn gcloud_build_log(output: &str) -> String {
    let noise = build_noise_re();
    let step = step_prefix_re();
    let starting = starting_step_re();
    let mut lines: Vec<String> = Vec::new();
    let mut status: Vec<String> = Vec::new(); // trailing `ID  CREATE_TIME … STATUS` table
    let mut cur_step: Option<String> = None;
    for l in gcloud_denoise(output.lines()) {
        let t = l.trim();
        if t.is_empty() { continue }
        if noise.is_match(t) { continue }
        // `----- REMOTE BUILD OUTPUT -----` dividers → bare title; pure rules dropped
        if t.starts_with("---") && t.ends_with("---") {
            let inner = t.trim_matches('-').trim();
            if !inner.is_empty() { lines.push(inner.to_string()) }
            cur_step = None;
            continue;
        }
        if !status.is_empty() || (t.starts_with("ID ") && t.contains("STATUS")) {
            status.push(l.clone());
            continue;
        }
        if let Some(c) = starting.captures(t) {
            // "Starting Step #0 - "build"" doubles as the header for that step's lines
            let prefix = c.get(1).map_or("", |m| m.as_str());
            lines.push(format!("{}:", t));
            cur_step = Some(prefix.to_string());
            continue;
        }
        if let Some(c) = step.captures(t) {
            let prefix = c.get(1).map_or("", |m| m.as_str());
            let rest = c.get(2).map_or("", |m| m.as_str());
            if cur_step.as_deref() != Some(prefix) {
                lines.push(format!("{}:", prefix));
                cur_step = Some(prefix.to_string());
            }
            if !rest.trim().is_empty() { lines.push(format!(" {}", rest.trim_end())) }
            continue;
        }
        cur_step = None;
        lines.push(l.to_string());
    }
    let mut lines = dedupe_consecutive(lines);
    if !status.is_empty() {
        let refs: Vec<&str> = status.iter().map(String::as_str).collect();
        lines.extend(fixed_table(&refs));
    }
    tail_pinned(lines, limits().log_tail, is_build_status).join("\n")
}

pub(crate) fn filter_gcloud(args: &[&str], output: &str) -> String {
    let output = normalize(output);
    if output.trim().is_empty() { return String::new() }
    let w = cmd_words(args, GCLOUD_VALUED, 3);

    let (preface, docs) = split_json(&output);
    if let Some(docs) = docs {
        let mut out: Vec<String> = gcloud_denoise(preface).into_iter().filter(|l| !l.trim().is_empty()).collect();
        out.push(render_docs(&docs));
        return out.join("\n");
    }
    match w.as_slice() {
        ["config", "list", ..] => gcloud_config_list(&output),
        ["builds", "submit" | "log", ..] => gcloud_build_log(&output),
        ["run", "deploy", ..] | ["run", "services" | "jobs", "deploy"] | ["app", "deploy", ..] | ["functions", "deploy", ..] => {
            gcloud_deploy(&output)
        }
        _ => gcloud_text(&output),
    }
}

// ─── AZ ───────────────────────────────────────────────────────────────────────

const AZ_VALUED: &[&str] = &[
    "--subscription", "-o", "--output", "--query", "-g", "--resource-group", "-n", "--name", "-l", "--location",
];

const AZ_LOGIN_BOILER: &[&str] = &[
    "A web browser has been opened", "Retrieving tenants and subscriptions", "[Tenant and subscription selection]",
    "The default is marked with an *", "Select a subscription and tenant",
];

/// Pull `WARNING:` lines out of az output. Warnings mentioning error/deprecation stay verbatim;
/// a single other warning is kept; two or more collapse to an announced count.
fn az_split_warnings(output: &str) -> (Vec<String>, String) {
    let mut kept: Vec<String> = Vec::new();
    let mut plain_warn: Vec<String> = Vec::new();
    let mut body: Vec<&str> = Vec::new();
    for l in output.lines() {
        let t = l.trim_start();
        if let Some(w) = t.strip_prefix("WARNING:") {
            let lw = w.to_ascii_lowercase();
            if lw.contains("error") || lw.contains("deprecat") {
                kept.push(t.trim_end().to_string());
            } else {
                plain_warn.push(t.trim_end().to_string());
            }
        } else {
            body.push(l);
        }
    }
    match plain_warn.len() {
        0 => {}
        1 => kept.push(plain_warn.remove(0)),
        n => kept.push(more(n, "warnings")),
    }
    (kept, body.join("\n"))
}

fn is_az_error(output: &str) -> bool {
    output.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("ERROR:") || t.starts_with("az: error") || t.starts_with("usage: az")
    })
}

/// az errors verbatim; the trailing "Examples from AI knowledge base" help block is counted.
fn az_error_text(output: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut help = 0usize;
    let mut in_help = false;
    for l in output.lines() {
        let t = l.trim();
        if t.is_empty() { continue }
        if t.starts_with("Examples from ") && t.ends_with("knowledge base:") { in_help = true }
        if in_help {
            help += 1;
            continue;
        }
        out.push(l.trim_end().to_string());
    }
    if help > 0 { out.push(more(help, "help lines")) }
    cap_vec(out, limits().passthrough_max_lines, "lines").join("\n")
}

/// `az login`: JSON array or the interactive tenant/subscription table → one line per
/// subscription (`*` = default, id, tenant), boilerplate prose dropped.
fn az_login(output: &str) -> String {
    let l = limits();
    if let Some(docs) = parse_json(output) {
        if let [Value::Array(subs)] = docs.as_slice() {
            if subs.iter().all(|s| s.get("name").is_some()) {
                let rows: Vec<String> = subs
                    .iter()
                    .map(|s| {
                        let mark = if s.get("isDefault").and_then(Value::as_bool) == Some(true) { "*" } else { " " };
                        let tenant = s.get("tenantDisplayName").or_else(|| s.get("tenantId")).map(plain).unwrap_or_default();
                        let state = s.get("state").and_then(Value::as_str).unwrap_or("Enabled");
                        let state = if state == "Enabled" { String::new() } else { format!(" [{}]", state) };
                        format!("{} {} {} ({}){}", mark, plain(&s["name"]), s.get("id").map(plain).unwrap_or_default(), tenant, state)
                    })
                    .collect();
                let mut out = vec![format!("az login: ok ({} subscriptions)", subs.len())];
                out.extend(cap_vec(rows, l.list_max_lines, "subscriptions"));
                return out.join("\n");
            }
        }
        return render_docs(&docs);
    }
    let lines: Vec<&str> = output.lines().collect();
    let hdr = lines.iter().position(|l| l.trim_start().starts_with("No ") && l.contains("Subscription"));
    let Some(h) = hdr else {
        return az_structured(output, None);
    };
    let starts = match lines.get(h + 1) {
        Some(u) if is_dash_rule(u) => col_starts(u),
        _ => col_starts(lines[h]),
    };
    let mut rows: Vec<String> = Vec::new();
    let mut i = h + 1;
    if lines.get(i).map_or(false, |u| is_dash_rule(u)) { i += 1 }
    while i < lines.len() && !lines[i].trim().is_empty() {
        let cells = slice_cols(lines[i], &starts);
        // "[1] *" → default marker
        let mark = if cells.first().map_or(false, |c| c.contains('*')) { "*" } else { " " };
        let name = cells.get(1).cloned().unwrap_or_default();
        let id = cells.get(2).cloned().unwrap_or_default();
        let tenant = cells.get(3).cloned().unwrap_or_default();
        rows.push(format!("{} {} {} ({})", mark, name, id, tenant));
        i += 1;
    }
    let mut out = vec![format!("az login: ok ({} subscriptions)", rows.len())];
    out.extend(cap_vec(rows, l.list_max_lines, "subscriptions"));
    // outcome lines (`Tenant:` / `Subscription:`) and anything else that is not boilerplate
    let mut announcements = false;
    for (k, l) in lines.iter().enumerate() {
        if k >= h && k < i { continue }
        let t = l.trim();
        if t.is_empty() || AZ_LOGIN_BOILER.iter().any(|b| t.starts_with(b)) { continue }
        if t == "[Announcements]" { announcements = true }
        if announcements { continue }
        out.push(t.to_string());
    }
    out.join("\n")
}

/// az payload: JSON → compact; tsv/table/yaml by shape; errors verbatim; else generic text.
fn az_structured(output: &str, fmt: Option<&str>) -> String {
    let (preface, docs) = split_json(output);
    if let Some(docs) = docs {
        let mut out: Vec<String> = preface.iter().filter(|l| !l.trim().is_empty()).map(|l| l.trim_end().to_string()).collect();
        out.push(render_docs(&docs));
        return out.join("\n");
    }
    if is_az_error(output) { return az_error_text(output) }
    if fmt == Some("tsv") { return tsv_text(output) }
    structured_text(output)
}

pub(crate) fn filter_az(args: &[&str], output: &str) -> String {
    let output = normalize(output);
    if output.trim().is_empty() { return String::new() }
    let (mut out, body) = az_split_warnings(&output);
    let w = cmd_words(args, AZ_VALUED, 2);
    let fmt = out_format(args, &["--output", "-o"]);
    if !body.trim().is_empty() {
        let rendered = match w.first().copied() {
            Some("login") => az_login(&body),
            _ => az_structured(&body, fmt.as_deref()),
        };
        if !rendered.is_empty() { out.push(rendered) }
    }
    out.join("\n")
}

// ─── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_all(out: &str, needles: &[&str]) {
        for n in needles {
            assert!(out.contains(n), "missing {:?} in:\n{}", n, out);
        }
    }

    // ── aws JSON ──────────────────────────────────────────────────────────────

    const EC2_JSON: &str = r#"{
    "Reservations": [
        {
            "Groups": [],
            "Instances": [
                {
                    "AmiLaunchIndex": 0,
                    "ImageId": "ami-0abcdef1234567890",
                    "InstanceId": "i-0123456789abcdef0",
                    "InstanceType": "t3.micro",
                    "KeyName": "prod-key",
                    "LaunchTime": "2024-01-15T10:23:45+00:00",
                    "Monitoring": { "State": "disabled" },
                    "Placement": { "AvailabilityZone": "us-east-1a", "GroupName": "", "Tenancy": "default" },
                    "PrivateIpAddress": "10.0.1.23",
                    "ProductCodes": [],
                    "PublicIpAddress": "54.12.34.56",
                    "State": { "Code": 16, "Name": "running" },
                    "StateTransitionReason": "",
                    "SubnetId": "subnet-0a1b2c3d",
                    "VpcId": "vpc-0f9e8d7c",
                    "SecurityGroups": [
                        { "GroupName": "web-sg", "GroupId": "sg-0011223344" },
                        { "GroupName": "ssh-sg", "GroupId": "sg-5566778899" }
                    ],
                    "ClientToken": "",
                    "EbsOptimized": false,
                    "Tags": [
                        { "Key": "Name", "Value": "web-1" },
                        { "Key": "env", "Value": "prod" }
                    ],
                    "IamInstanceProfile": {
                        "Arn": "arn:aws:iam::123456789012:instance-profile/web-role",
                        "Id": "AIPAEXAMPLE"
                    }
                }
            ],
            "OwnerId": "123456789012",
            "ReservationId": "r-0aa1bb2cc3"
        }
    ],
    "ResponseMetadata": { "RequestId": "deadbeef-0000", "HTTPStatusCode": 200 }
}"#;

    #[test]
    fn aws_json_golden() {
        let out = filter_aws(&["ec2", "describe-instances"], EC2_JSON);
        assert!(out.contains("Tags: Name=web-1, env=prod"), "{}", out);
        assert!(!out.contains("ResponseMetadata"), "{}", out);
        assert!(!out.contains("ProductCodes"), "{}", out);
        assert!(!out.contains("GroupName: \"\""), "{}", out);
        assert!(!out.contains("ClientToken"), "{}", out);
        assert!(out.contains("LaunchTime: 2024-01-15T10:23:45+00:00"), "{}", out);
        // Groups[], ProductCodes[], GroupName"", StateTransitionReason"", ClientToken"", ResponseMetadata = 6
        assert!(out.contains("[+6 more empty fields]"), "{}", out);
        // serde_json sorts keys (no preserve_order) — crate-wide convention from compact_json
        assert!(out.contains("SecurityGroups: [2] GroupId|GroupName\n     sg-0011223344|web-sg"), "{}", out);
        assert!(has_truncation(&out));
        assert!(out.len() < EC2_JSON.len() / 2, "{} vs {}", out.len(), EC2_JSON.len());
    }

    #[test]
    fn aws_json_fidelity() {
        let out = filter_aws(&["ec2", "describe-instances"], EC2_JSON);
        assert_all(&out, &[
            "ami-0abcdef1234567890", "i-0123456789abcdef0", "t3.micro", "prod-key", "us-east-1a",
            "10.0.1.23", "54.12.34.56", "running", "subnet-0a1b2c3d", "vpc-0f9e8d7c", "web-sg",
            "sg-0011223344", "ssh-sg", "sg-5566778899", "arn:aws:iam::123456789012:instance-profile/web-role",
            "AIPAEXAMPLE", "123456789012", "r-0aa1bb2cc3", "Tenancy: default", "EbsOptimized: false",
        ]);
    }

    #[test]
    fn aws_json_column_prune_keeps_tables() {
        // Ip is null in one row only → column kept, table shape preserved
        let j = r#"[{"Id":"i-1","Ip":null,"Gone":null},{"Id":"i-2","Ip":"1.2.3.4","Gone":""}]"#;
        let out = filter_aws(&["ec2", "describe-instances", "--query", "x"], j);
        assert!(out.contains("[2] Id|Ip"), "{}", out);
        assert!(out.contains("i-1|null"), "{}", out);
        assert!(!out.contains("Gone"), "{}", out);
        assert!(out.contains("[+2 more empty fields]"), "{}", out);
    }

    #[test]
    fn aws_json_preface_notice_kept() {
        let raw = "Python 3.7 deprecation warning\n{\"Buckets\": [{\"Name\": \"my-bucket\", \"CreationDate\": \"2023-01-01T00:00:00+00:00\"}]}";
        let out = filter_aws(&["s3api", "list-buckets"], raw);
        assert_all(&out, &["Python 3.7 deprecation warning", "my-bucket", "2023-01-01T00:00:00+00:00"]);
    }

    #[test]
    fn aws_sts_one_line() {
        let raw = "{\n    \"UserId\": \"AIDAEXAMPLEID\",\n    \"Account\": \"123456789012\",\n    \"Arn\": \"arn:aws:iam::123456789012:user/dev\"\n}";
        let out = filter_aws(&["sts", "get-caller-identity"], raw);
        assert_eq!(out, "Account: 123456789012  Arn: arn:aws:iam::123456789012:user/dev  UserId: AIDAEXAMPLEID");
    }

    #[test]
    fn aws_log_events_json() {
        let raw = r#"{
    "events": [
        {"timestamp": 1705312425000, "message": "START RequestId: aaa Version: $LATEST", "ingestionTime": 1705312426000},
        {"timestamp": 1705312425100, "message": "processed 10 items", "ingestionTime": 1705312426000},
        {"timestamp": 1705312425200, "message": "processed 12 items", "ingestionTime": 1705312426000},
        {"timestamp": 1705312425300, "message": "ERROR boom", "ingestionTime": 1705312426000}
    ],
    "nextForwardToken": "f/1234567890/s",
    "nextBackwardToken": "b/0987654321/s"
}"#;
        let out = filter_aws(&["logs", "get-log-events", "--log-group-name", "g"], raw);
        assert!(out.starts_with("4 events\n"), "{}", out);
        // 1705312425000 ms = 2024-01-15T09:53:45Z
        assert!(out.contains("2024-01-15T09:53:45.000Z START RequestId: aaa"), "{}", out);
        assert!(out.contains("processed 10 items  (×2)"), "{}", out);
        assert!(!out.contains("processed 12"), "{}", out);
        assert!(out.contains("ERROR boom"), "{}", out);
        assert!(!out.contains("ingestionTime"), "{}", out);
        assert_all(&out, &["nextForwardToken: f/1234567890/s", "nextBackwardToken: b/0987654321/s"]);
    }

    // ── aws table / text / yaml ───────────────────────────────────────────────

    const EC2_TABLE: &str = "\
-------------------------------------------------------------------
|                        DescribeInstances                        |
+-----------------------------------------------------------------+
||                           Instances                           ||
|+--------------------+------------------------------------------+|
||  InstanceId        |  i-0123456789abcdef0                     ||
||  InstanceType      |  t3.micro                                ||
||  PrivateIpAddress  |  10.0.1.23                               ||
|+--------------------+------------------------------------------+|
|||                            Tags                             |||
||+------------------------------+------------------------------+||
|||             Key              |            Value             |||
||+------------------------------+------------------------------+||
|||  Name                        |  web-1                       |||
|||  env                         |  prod                        |||
||+------------------------------+------------------------------+||
";

    #[test]
    fn aws_table_golden() {
        let out = filter_aws(&["ec2", "describe-instances", "--output", "table"], EC2_TABLE);
        let expect = "DescribeInstances:\n Instances:\n InstanceId: i-0123456789abcdef0\n InstanceType: t3.micro\n PrivateIpAddress: 10.0.1.23\n  Tags:\n  Key|Value\n  Name|web-1\n  env|prod";
        assert_eq!(out, expect);
    }

    #[test]
    fn aws_table_drops_borders_no_plus() {
        let out = filter_aws(&["ec2", "describe-instances", "--output", "table"], EC2_TABLE);
        assert!(!out.contains("+--"));
        assert!(!out.contains("||"));
    }

    #[test]
    fn aws_text_tsv() {
        let raw = "i-0123456789abcdef0\tt3.micro\trunning\ni-0fedcba9876543210\tt3.small\tstopped\n";
        let out = filter_aws(&["ec2", "describe-instances", "--output", "text"], raw);
        assert_eq!(out, "i-0123456789abcdef0 t3.micro running\ni-0fedcba9876543210 t3.small stopped");
        // content-detected without the flag too
        assert_eq!(filter_aws(&["ec2", "describe-instances"], raw), out);
    }

    #[test]
    fn aws_yaml_output() {
        let raw = "Buckets:\n- CreationDate: '2023-01-01T00:00:00+00:00'\n  Name: my-bucket\n- CreationDate: '2023-02-01T00:00:00+00:00'\n  Name: other-bucket\nOwner:\n  DisplayName: me\n  ID: abcdef0123456789\n";
        let out = filter_aws(&["s3api", "list-buckets", "--output", "yaml"], raw);
        assert!(out.contains("Buckets: [2] CreationDate|Name"), "{}", out);
        assert_all(&out, &["my-bucket", "other-bucket", "2023-02-01T00:00:00+00:00", "DisplayName: me", "ID: abcdef0123456789"]);
    }

    // ── aws s3 ────────────────────────────────────────────────────────────────

    #[test]
    fn aws_s3_ls_golden() {
        let raw = "                           PRE logs/\n                           PRE tmp/\n2024-01-15 10:23:45     123456 data/report.csv\n2024-01-16 08:00:00          0 data/empty.txt\n\nTotal Objects: 2\n   Total Size: 123456\n";
        let out = filter_aws(&["s3", "ls", "s3://my-bucket/", "--summarize"], raw);
        assert_eq!(out, "logs/\ntmp/\n2024-01-15 10:23:45 123456 data/report.csv\n2024-01-16 08:00:00 0 data/empty.txt\nTotal Objects: 2\nTotal Size: 123456");
    }

    #[test]
    fn aws_s3_ls_cap_announced() {
        let n = limits().ls_max_entries + 5;
        let raw: String = (0..n).map(|i| format!("2024-01-15 10:23:45 {} data/f{}.txt\n", i, i)).collect();
        let out = filter_aws(&["s3", "ls", "s3://b/"], &raw);
        assert!(out.contains(&more(5, "entries")), "{}", out);
        assert!(out.contains("data/f0.txt"));
    }

    #[test]
    fn aws_s3_sync_progress_dropped_errors_kept() {
        let raw = "Completed 1.0 MiB/5.0 MiB (1.2 MiB/s) with 3 file(s) remaining\rCompleted 2.0 MiB/5.0 MiB (1.2 MiB/s) with 3 file(s) remaining\rupload: ./a.txt to s3://my-bucket/a.txt\nCompleted 3.0 MiB/5.0 MiB (1.2 MiB/s) with 2 file(s) remaining\rupload: ./dir/b.txt to s3://my-bucket/dir/b.txt\nupload failed: ./c.txt to s3://my-bucket/c.txt An error occurred (AccessDenied) when calling the PutObject operation: Access Denied\ndelete: s3://my-bucket/old.txt\n";
        let out = filter_aws(&["s3", "sync", ".", "s3://my-bucket"], raw);
        assert!(!out.contains("Completed"), "{}", out);
        assert_eq!(out.lines().count(), 4, "{}", out);
        assert_all(&out, &[
            "upload: ./a.txt to s3://my-bucket/a.txt",
            "upload: ./dir/b.txt to s3://my-bucket/dir/b.txt",
            "upload failed: ./c.txt to s3://my-bucket/c.txt An error occurred (AccessDenied) when calling the PutObject operation: Access Denied",
            "delete: s3://my-bucket/old.txt",
        ]);
    }

    #[test]
    fn aws_s3_cp_only_progress() {
        let out = filter_aws(&["s3", "cp", "a", "s3://b/a"], "Completed 256.0 KiB/1.0 MiB (1.2 MiB/s) with 1 file(s) remaining\r");
        assert_eq!(out, "aws s3: 1 progress lines, no transfers listed");
    }

    #[test]
    fn aws_s3_transfer_cap() {
        let n = limits().list_max_lines + 3;
        let raw: String = (0..n).map(|i| format!("upload: ./f{}.txt to s3://b/f{}.txt\n", i, i)).collect();
        let out = filter_aws(&["s3", "sync", ".", "s3://b"], &raw);
        assert!(out.contains(&more(3, "transfers")), "{}", out);
    }

    // ── aws logs tail ─────────────────────────────────────────────────────────

    #[test]
    fn aws_logs_tail_drops_stream_dedupes() {
        let raw = "2024-01-15T10:23:45.123000+00:00 2024/01/15/[$LATEST]abc123 START RequestId: 1\n2024-01-15T10:23:45.200000+00:00 2024/01/15/[$LATEST]abc123 processed 10 items\n2024-01-15T10:23:45.300000+00:00 2024/01/15/[$LATEST]abc123 processed 11 items\n2024-01-15T10:23:45.400000+00:00 2024/01/15/[$LATEST]abc123 ERROR timeout after 3000ms\n";
        let out = filter_aws(&["logs", "tail", "/aws/lambda/fn"], raw);
        assert!(!out.contains("[$LATEST]abc123"), "{}", out);
        assert!(out.contains("2024-01-15T10:23:45.200000+00:00 processed 10 items  (×2)"), "{}", out);
        assert!(out.contains("2024-01-15T10:23:45.400000+00:00 ERROR timeout after 3000ms"), "{}", out);
        assert!(out.contains("START RequestId: 1"));
    }

    #[test]
    fn aws_logs_tail_pins_errors_beyond_tail() {
        let n = limits().log_tail + 20;
        let mut raw = String::from("2024-01-15T10:00:00.000000+00:00 s ERROR early failure\n");
        for i in 0..n {
            raw.push_str(&format!("2024-01-15T10:00:00.000000+00:00 s line {} value {}\n", i, i * 7 % 5));
        }
        let out = filter_aws(&["logs", "tail", "g"], &raw);
        // line templates dedupe fully → tiny output, but the error survives
        assert!(out.contains("ERROR early failure"), "{}", out);
        // short-format (no stream column) lines are untouched
        let short = filter_aws(&["logs", "tail", "g", "--format", "short"], "10:23:45 hello world\n");
        assert_eq!(short, "10:23:45 hello world");
    }

    #[test]
    fn tail_pinned_marker() {
        let lines: Vec<String> = (0..10).map(|i| if i == 1 { "ERROR x".into() } else { format!("l{}", i) }).collect();
        let out = tail_pinned(lines, 3, is_log_alert);
        assert_eq!(out, vec!["ERROR x", "[+6 more lines]", "l7", "l8", "l9"]);
    }

    // ── aws errors / empty / unknown ──────────────────────────────────────────

    #[test]
    fn aws_error_verbatim() {
        let raw = "\nAn error occurred (AccessDenied) when calling the ListBuckets operation: Access Denied\n";
        let out = filter_aws(&["s3api", "list-buckets"], raw);
        assert_eq!(out, "An error occurred (AccessDenied) when calling the ListBuckets operation: Access Denied");
        let creds = filter_aws(&["ec2", "describe-instances"], "Unable to locate credentials. You can configure credentials by running \"aws configure\".\n");
        assert_eq!(creds, "Unable to locate credentials. You can configure credentials by running \"aws configure\".");
    }

    #[test]
    fn aws_usage_error_boilerplate_and_choices() {
        let raw = "usage: aws [options] <command> <subcommand> [<subcommand> ...] [parameters]\nTo see help text, you can run:\n\n  aws help\n  aws <command> help\n  aws <command> <subcommand> help\n\naws: error: argument command: Invalid choice, valid choices are:\n\naccessanalyzer                           | account\nacm                                      | acm-pca\namplify                                  | amplifybackend\n";
        let out = filter_aws(&["s3x"], raw);
        assert_eq!(out, "aws: error: argument command: Invalid choice, valid choices are:\n[+3 more choices]");
    }

    #[test]
    fn aws_empty_and_unknown() {
        assert_eq!(filter_aws(&["ec2", "describe-instances"], ""), "");
        assert_eq!(filter_aws(&["s3", "ls"], "\n\n"), "");
        assert_eq!(filter_aws(&["s3", "sync", ".", "s3://b"], ""), "");
        let out = filter_aws(&["ecr", "get-login-password"], "eyJwYXlsb2FkIjoiYWJj\n\n\nsecond\n");
        assert_eq!(out, "eyJwYXlsb2FkIjoiYWJj\n\nsecond");
    }

    #[test]
    fn aws_words_skip_global_options() {
        let raw = "{\"UserId\": \"U\", \"Account\": \"1\", \"Arn\": \"a\"}";
        let out = filter_aws(&["--profile", "prod", "--region", "eu-west-1", "sts", "get-caller-identity", "--output", "json"], raw);
        assert_eq!(out, "Account: 1  Arn: a  UserId: U");
    }

    // ── gcloud ────────────────────────────────────────────────────────────────

    const GCLOUD_LIST: &str = "\
NAME     ZONE           MACHINE_TYPE  PREEMPTIBLE  INTERNAL_IP  EXTERNAL_IP    STATUS
web-1    us-central1-a  e2-medium                  10.128.0.2   34.120.10.11   RUNNING
batch-7  us-central1-b  n2-standard-4 true         10.128.0.9                  TERMINATED
";

    #[test]
    fn gcloud_table_keeps_empty_cells_positional() {
        let out = filter_gcloud(&["compute", "instances", "list"], GCLOUD_LIST);
        let expect = "NAME|ZONE|MACHINE_TYPE|PREEMPTIBLE|INTERNAL_IP|EXTERNAL_IP|STATUS\nweb-1|us-central1-a|e2-medium||10.128.0.2|34.120.10.11|RUNNING\nbatch-7|us-central1-b|n2-standard-4|true|10.128.0.9||TERMINATED";
        assert_eq!(out, expect);
    }

    #[test]
    fn gcloud_table_cap() {
        let n = limits().list_max_lines + 4;
        let mut raw = String::from("NAME     ZONE\n");
        for i in 0..n { raw.push_str(&format!("vm-{:<5} us-central1-a\n", i)) }
        let out = filter_gcloud(&["compute", "instances", "list"], &raw);
        assert!(out.contains(&more(4, "rows")), "{}", out);
        assert!(out.contains("vm-0"));
    }

    #[test]
    fn gcloud_json_format() {
        let raw = "[\n  {\n    \"name\": \"web-1\",\n    \"status\": \"RUNNING\",\n    \"labels\": {},\n    \"zone\": \"https://www.googleapis.com/compute/v1/projects/p/zones/us-central1-a\"\n  }\n]";
        let out = filter_gcloud(&["compute", "instances", "list", "--format=json"], raw);
        assert_all(&out, &["name: web-1", "status: RUNNING", "zones/us-central1-a"]);
        assert!(!out.contains("labels"), "{}", out);
        assert!(out.contains("[+1 more empty fields]"), "{}", out);
    }

    #[test]
    fn gcloud_describe_yaml() {
        let raw = "WARNING: Some requests generated warnings.\nid: '1234567890123456789'\nkind: compute#instance\nmachineType: https://www.googleapis.com/compute/v1/projects/p/zones/us-central1-a/machineTypes/e2-medium\nname: web-1\nnetworkInterfaces:\n- accessConfigs:\n  - natIP: 34.120.10.11\n    type: ONE_TO_ONE_NAT\n  networkIP: 10.128.0.2\nstatus: RUNNING\ntags:\n  fingerprint: abc=\n  items:\n  - http-server\n  - https-server\n";
        let out = filter_gcloud(&["compute", "instances", "describe", "web-1"], raw);
        assert!(out.starts_with("WARNING: Some requests generated warnings.\n"), "{}", out);
        assert_all(&out, &["name: web-1", "status: RUNNING", "natIP: 34.120.10.11", "networkIP: 10.128.0.2", "items: [http-server, https-server]", "fingerprint: abc=", "\"1234567890123456789\"", "machineTypes/e2-medium"]);
        // parsed, not passed through: raw YAML has the bullet at column 0
        assert!(!out.contains("\n- accessConfigs:"), "{}", out);
        assert!(out.contains("networkInterfaces: [1]"), "{}", out);
    }

    #[test]
    fn gcloud_config_list_compact() {
        let raw = "[compute]\nregion = us-central1\nzone = us-central1-a\n[core]\naccount = dev@example.com\ndisable_usage_reporting = True\nproject = my-project\n\nYour active configuration is: [default]\n";
        let out = filter_gcloud(&["config", "list"], raw);
        assert_eq!(out, "compute: region=us-central1, zone=us-central1-a\ncore: account=dev@example.com, disable_usage_reporting=True, project=my-project\nactive configuration: default");
    }

    #[test]
    fn gcloud_config_set_kept() {
        let out = filter_gcloud(&["config", "set", "project", "my-project"], "Updated property [core/project].\n");
        assert_eq!(out, "Updated property [core/project].");
    }

    #[test]
    fn gcloud_auth_list_table_hints_dropped() {
        let raw = "   Credentialed Accounts\nACTIVE  ACCOUNT\n*       dev@example.com\n        ci@my-project.iam.gserviceaccount.com\n\nTo set the active account, run:\n    $ gcloud config set account `ACCOUNT`\n\n";
        let out = filter_gcloud(&["auth", "list"], raw);
        assert_eq!(out, "   Credentialed Accounts\nACTIVE|ACCOUNT\n*|dev@example.com\n|ci@my-project.iam.gserviceaccount.com");
    }

    #[test]
    fn gcloud_run_deploy_success() {
        let raw = "Deploying container to Cloud Run service [api] in project [my-project] region [us-central1]\nDeploying...\nCreating Revision......................done\nRouting traffic.....done\nSetting IAM Policy....done\nDone.\nService [api] revision [api-00012-xyz] has been deployed and is serving 100 percent of traffic.\nService URL: https://api-abc123-uc.a.run.app\n";
        let out = filter_gcloud(&["run", "deploy", "api", "--image", "gcr.io/p/api"], raw);
        assert_eq!(out, "Deploying container to Cloud Run service [api] in project [my-project] region [us-central1]\n✓ 3 steps\nService [api] revision [api-00012-xyz] has been deployed and is serving 100 percent of traffic.\nService URL: https://api-abc123-uc.a.run.app");
    }

    #[test]
    fn gcloud_run_deploy_failure_keeps_error_and_failed_step() {
        let raw = "Deploying container to Cloud Run service [api] in project [my-project] region [us-central1]\n⠛ Deploying... Creating Revision...\n⠶ Deploying... Creating Revision...\nX Deploying... Revision deployment failed. Container failed to start.\n  ✓ Creating Revision...\n  X Routing traffic...\n  . Setting IAM Policy...\nDeployment failed\nERROR: (gcloud.run.deploy) The user-provided container failed to start and listen on the port defined provided by the PORT=8080 environment variable.\nLogs URL: https://console.cloud.google.com/logs/viewer?project=my-project&resource=cloud_run_revision/service_name/api/revision_name/api-00013-abc\n";
        let out = filter_gcloud(&["run", "deploy", "api"], raw);
        assert!(!out.contains('⠛'), "{}", out);
        assert_all(&out, &[
            "X Deploying... Revision deployment failed. Container failed to start.",
            "✓ 1 steps",
            "X Routing traffic...",
            ". Setting IAM Policy...",
            "ERROR: (gcloud.run.deploy) The user-provided container failed to start",
            "Logs URL: https://console.cloud.google.com/logs/viewer?project=my-project",
        ]);
    }

    #[test]
    fn gcloud_builds_log_golden() {
        let raw = "\
Creating temporary archive of 12 file(s) totalling 45.2 KiB before compression.
Uploading tarball of [.] to [gs://my-project_cloudbuild/source/1705312345.12-abc.tgz]
Created [https://cloudbuild.googleapis.com/v1/projects/my-project/locations/global/builds/8f1a2b3c-4d5e].
Logs are available at [ https://console.cloud.google.com/cloud-build/builds/8f1a2b3c-4d5e?project=123 ].
----------------------------- REMOTE BUILD OUTPUT ------------------------------
starting build \"8f1a2b3c-4d5e\"

FETCHSOURCE
Fetching storage object: gs://my-project_cloudbuild/source/1705312345.12-abc.tgz#1705312346
Copying gs://my-project_cloudbuild/source/1705312345.12-abc.tgz#1705312346...
/ [1 files][ 45.2 KiB/ 45.2 KiB]
Operation completed over 1 objects/45.2 KiB.
BUILD
Starting Step #0 - \"build\"
Step #0 - \"build\": Already have image (with digest): gcr.io/cloud-builders/docker
Step #0 - \"build\": Sending build context to Docker daemon  12.34kB
Step #0 - \"build\": Step 1/3 : FROM node:20-alpine
Step #0 - \"build\":  ---> 1a2b3c4d5e6f
Step #0 - \"build\": Step 2/3 : WORKDIR /app
Step #0 - \"build\":  ---> Running in 9f8e7d6c5b4a
Step #0 - \"build\": Removing intermediate container 9f8e7d6c5b4a
Step #0 - \"build\": Step 3/3 : COPY . .
Step #0 - \"build\": Successfully built abcdef123456
Step #0 - \"build\": Successfully tagged gcr.io/my-project/app:latest
Finished Step #0 - \"build\"
Starting Step #1 - \"push\"
Step #1 - \"push\": The push refers to repository [gcr.io/my-project/app]
Step #1 - \"push\": 5f70bf18a086: Preparing
Step #1 - \"push\": 5f70bf18a086: Waiting
Step #1 - \"push\": 5f70bf18a086: Pushing
Step #1 - \"push\": 5f70bf18a086: Pushed
Step #1 - \"push\": latest: digest: sha256:0123456789abcdef size: 1234
Finished Step #1 - \"push\"
PUSH
DONE
--------------------------------------------------------------------------------

ID                                    CREATE_TIME                DURATION  SOURCE                                                          IMAGES                          STATUS
8f1a2b3c-4d5e                         2024-01-15T10:23:45+00:00  1M23S     gs://my-project_cloudbuild/source/1705312345.12-abc.tgz         gcr.io/my-project/app:latest    SUCCESS
";
        let out = filter_gcloud(&["builds", "submit", "--tag", "gcr.io/my-project/app"], raw);
        assert!(!out.contains("--->"), "{}", out);
        assert!(!out.contains("Preparing"), "{}", out);
        assert!(!out.contains("Removing intermediate"), "{}", out);
        assert!(!out.contains("+-"), "{}", out);
        assert!(!out.contains("Copying gs://"), "{}", out);
        assert!(!out.contains("Already have image"), "{}", out);
        assert!(out.contains("REMOTE BUILD OUTPUT"), "{}", out);
        // step prefix factored once, onto the Starting line
        assert_eq!(out.matches("Step #0 - \"build\":").count(), 1, "{}", out);
        assert!(out.contains("Starting Step #0 - \"build\":\n Step 1/3 : FROM node:20-alpine\n Step 2/3 : WORKDIR /app"), "{}", out);
        // status table → positional pipe row
        assert!(out.contains("ID|CREATE_TIME|DURATION|SOURCE|IMAGES|STATUS\n8f1a2b3c-4d5e|2024-01-15T10:23:45+00:00|1M23S|"), "{}", out);
        assert_all(&out, &[
            "Step 1/3 : FROM node:20-alpine", "Step 3/3 : COPY . .", "Successfully tagged gcr.io/my-project/app:latest",
            "latest: digest: sha256:0123456789abcdef size: 1234", "8f1a2b3c-4d5e", "gs://my-project_cloudbuild/source/1705312345.12-abc.tgz",
            "1M23S", "SUCCESS", "Finished Step #1 - \"push\"", "FETCHSOURCE",
            "Logs are available at [ https://console.cloud.google.com/cloud-build/builds/8f1a2b3c-4d5e?project=123 ].",
        ]);
        // gsutil FETCHSOURCE chatter is decoration: the source URL survives in Uploading + status table
        assert!(!out.contains("Fetching storage object"), "{}", out);
        assert!(out.len() < raw.len() / 2, "{} vs {}", out.len(), raw.len());
    }

    #[test]
    fn gcloud_builds_log_tail_pins_error() {
        let n = limits().log_tail + 30;
        let mut raw = String::from("Step #0: ERROR: build failed early\n");
        for i in 0..n { raw.push_str(&format!("Step #0: compiling module {} of {}\n", i, n)) }
        raw.push_str("ERROR: (gcloud.builds.submit) build 8f1a-2b3c completed with status \"FAILURE\"\n");
        let out = filter_gcloud(&["builds", "submit"], &raw);
        assert!(out.contains("ERROR: build failed early"), "{}", out);
        assert!(out.contains("completed with status \"FAILURE\""), "{}", out);
        assert!(has_truncation(&out), "{}", out);
    }

    #[test]
    fn gcloud_error_and_empty() {
        let raw = "ERROR: (gcloud.compute.instances.list) Some requests did not succeed:\n - Failed to find project my-project\n";
        let out = filter_gcloud(&["compute", "instances", "list"], raw);
        assert_eq!(out, "ERROR: (gcloud.compute.instances.list) Some requests did not succeed:\n - Failed to find project my-project");
        assert_eq!(filter_gcloud(&["compute", "instances", "list"], ""), "");
        assert_eq!(filter_gcloud(&["run", "deploy"], "\n"), "");
        assert_eq!(filter_gcloud(&["builds", "log"], ""), "");
        assert_eq!(filter_gcloud(&["config", "list"], ""), "");
        assert_eq!(filter_gcloud(&["projects", "list"], "Listed 0 items.\n"), "Listed 0 items.");
    }

    #[test]
    fn gcloud_update_banner_dropped() {
        let raw = "Updated property [core/project].\n\nUpdates are available for some Google Cloud CLI components.  To update all\ninstalled components, run:\n  $ gcloud components update\n";
        let out = filter_gcloud(&["config", "set", "project", "p"], raw);
        assert_eq!(out, "Updated property [core/project].");
    }

    // ── az ────────────────────────────────────────────────────────────────────

    #[test]
    fn az_json_group_create() {
        let raw = "{\n  \"id\": \"/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/rg-demo\",\n  \"location\": \"eastus\",\n  \"managedBy\": null,\n  \"name\": \"rg-demo\",\n  \"properties\": {\n    \"provisioningState\": \"Succeeded\"\n  },\n  \"tags\": null,\n  \"type\": \"Microsoft.Resources/resourceGroups\"\n}\n";
        let out = filter_az(&["group", "create", "-n", "rg-demo", "-l", "eastus"], raw);
        assert_all(&out, &["/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/rg-demo", "location: eastus", "name: rg-demo", "provisioningState: Succeeded", "type: Microsoft.Resources/resourceGroups"]);
        assert!(!out.contains("managedBy"), "{}", out);
        assert!(out.contains("[+2 more empty fields]"), "{}", out);
    }

    #[test]
    fn az_table_output() {
        let raw = "Name       ResourceGroup    Location    Zones    PowerState\n---------  ---------------  ----------  -------  ------------\nvm-web-1   rg-demo          eastus      1        VM running\nvm-db-1    rg-demo          eastus               VM deallocated\n";
        let out = filter_az(&["vm", "list", "-d", "-o", "table"], raw);
        assert_eq!(out, "Name|ResourceGroup|Location|Zones|PowerState\nvm-web-1|rg-demo|eastus|1|VM running\nvm-db-1|rg-demo|eastus||VM deallocated");
    }

    #[test]
    fn az_tsv_output() {
        let out = filter_az(&["vm", "list", "-o", "tsv"], "vm-web-1\trg-demo\teastus\n");
        assert_eq!(out, "vm-web-1 rg-demo eastus");
    }

    #[test]
    fn az_login_text_table() {
        let raw = "A web browser has been opened at https://login.microsoftonline.com/organizations/oauth2/v2.0/authorize. Please continue the login in the web browser. If no web browser is available or if the web browser fails to open, use device code flow with `az login --use-device-code`.\n\nRetrieving tenants and subscriptions for the selection...\n\n[Tenant and subscription selection]\n\nNo     Subscription name    Subscription ID                       Tenant\n-----  -------------------  ------------------------------------  -----------------\n[1] *  Pay-As-You-Go        00000000-0000-0000-0000-000000000000  Default Directory\n[2]    Dev Sandbox          11111111-1111-1111-1111-111111111111  Contoso\n\nThe default is marked with an *; the default tenant is 'Default Directory' and subscription is 'Pay-As-You-Go' (00000000-0000-0000-0000-000000000000).\n\nSelect a subscription and tenant (Type a number or Enter for no changes): \n\nTenant: Default Directory\nSubscription: Pay-As-You-Go (00000000-0000-0000-0000-000000000000)\n\n[Announcements]\nWith the new Azure CLI login experience, you can select the subscription to use with 'az login'.\n";
        let out = filter_az(&["login"], raw);
        assert_eq!(out, "az login: ok (2 subscriptions)\n* Pay-As-You-Go 00000000-0000-0000-0000-000000000000 (Default Directory)\n  Dev Sandbox 11111111-1111-1111-1111-111111111111 (Contoso)\nTenant: Default Directory\nSubscription: Pay-As-You-Go (00000000-0000-0000-0000-000000000000)");
    }

    #[test]
    fn az_login_json() {
        let raw = "[\n  {\n    \"cloudName\": \"AzureCloud\",\n    \"homeTenantId\": \"aaaa\",\n    \"id\": \"00000000-0000-0000-0000-000000000000\",\n    \"isDefault\": true,\n    \"managedByTenants\": [],\n    \"name\": \"Pay-As-You-Go\",\n    \"state\": \"Enabled\",\n    \"tenantId\": \"aaaa\",\n    \"user\": { \"name\": \"me@example.com\", \"type\": \"user\" }\n  },\n  {\n    \"cloudName\": \"AzureCloud\",\n    \"homeTenantId\": \"bbbb\",\n    \"id\": \"11111111-1111-1111-1111-111111111111\",\n    \"isDefault\": false,\n    \"managedByTenants\": [],\n    \"name\": \"Dev\",\n    \"state\": \"Disabled\",\n    \"tenantId\": \"bbbb\",\n    \"user\": { \"name\": \"me@example.com\", \"type\": \"user\" }\n  }\n]\n";
        let out = filter_az(&["login"], raw);
        assert_eq!(out, "az login: ok (2 subscriptions)\n* Pay-As-You-Go 00000000-0000-0000-0000-000000000000 (aaaa)\n  Dev 11111111-1111-1111-1111-111111111111 (bbbb) [Disabled]");
    }

    #[test]
    fn az_login_device_code_kept() {
        let raw = "To sign in, use a web browser to open the page https://microsoft.com/devicelogin and enter the code ABCD1234E to authenticate.\n";
        let out = filter_az(&["login", "--use-device-code"], raw);
        assert_eq!(out, raw.trim());
    }

    #[test]
    fn az_warnings_collapsed_unless_important() {
        let json = "{\"name\": \"aks-1\", \"provisioningState\": \"Succeeded\"}";
        let two = format!("WARNING: The behavior of this command has been altered by the following extension: aks-preview\nWARNING: Command group 'aks' is in preview and under development.\n{}", json);
        let out = filter_az(&["aks", "show", "-n", "aks-1", "-g", "rg"], &two);
        assert_eq!(out, "[+2 more warnings]\nname: aks-1\nprovisioningState: Succeeded");
        let one = format!("WARNING: Command group 'aks' is in preview and under development.\n{}", json);
        let out1 = filter_az(&["aks", "show"], &one);
        assert!(out1.starts_with("WARNING: Command group 'aks' is in preview"), "{}", out1);
        let dep = format!("WARNING: This command has been deprecated and will be removed in a future release.\nWARNING: noise\nWARNING: more noise\n{}", json);
        let out2 = filter_az(&["aks", "show"], &dep);
        assert!(out2.starts_with("WARNING: This command has been deprecated"), "{}", out2);
        assert!(out2.contains("[+2 more warnings]"), "{}", out2);
    }

    #[test]
    fn az_error_with_examples_block() {
        let raw = "ERROR: (ResourceGroupNotFound) Resource group 'rg-missing' could not be found.\nCode: ResourceGroupNotFound\nMessage: Resource group 'rg-missing' could not be found.\n\nExamples from AI knowledge base:\naz vm list --resource-group MyResourceGroup\nList all VMs in a resource group\n\nhttps://docs.microsoft.com/en-us/cli/azure/vm#az_vm_list\nRead more about the command in reference docs\n";
        let out = filter_az(&["vm", "list", "-g", "rg-missing"], raw);
        assert_eq!(out, "ERROR: (ResourceGroupNotFound) Resource group 'rg-missing' could not be found.\nCode: ResourceGroupNotFound\nMessage: Resource group 'rg-missing' could not be found.\n[+5 more help lines]");
    }

    #[test]
    fn az_empty_and_unknown_text() {
        assert_eq!(filter_az(&["group", "list"], ""), "");
        assert_eq!(filter_az(&["login"], "\n"), "");
        let out = filter_az(&["aks", "get-credentials", "-n", "aks-1", "-g", "rg"], "Merged \"aks-1\" as current context in /home/me/.kube/config\n");
        assert_eq!(out, "Merged \"aks-1\" as current context in /home/me/.kube/config");
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    #[test]
    fn helpers_robust_on_odd_input() {
        assert_eq!(filter_aws(&["s3", "ls"], "PRE\n"), "PRE");
        assert_eq!(filter_aws(&["logs", "tail", "g"], "x\n"), "x");
        assert_eq!(filter_gcloud(&[], "héllo  wörld\n"), "héllo  wörld");
        assert_eq!(filter_az(&[], "WARNING: only a warning\n"), "WARNING: only a warning");
        assert!(filter_aws(&["ec2", "describe-instances", "--output", "table"], "---\n|a|\n---\n").ends_with("a:"));
        assert!(!is_table_header("ERROR: SOMETHING  BAD", None));
        assert!(is_table_header("NAME  ZONE  STATUS", None));
        // per-digit: run width kept so `9 items` and `10 items` stay distinct templates
        assert_eq!(template_of("2024-01-15 processed 10 items in 45ms"), "####-##-## processed ## items in ##ms");
        assert_eq!(template_of("processed 10 items"), template_of("processed 12 items"));
        assert_ne!(template_of("processed 9 items"), template_of("processed 10 items"));
        assert!(yaml_docs("just a sentence\nanother one").is_none());
        assert!(yaml_docs("ERROR: x\nfoo: bar").is_none());
    }
}
