//! vcs filters: git, gh, glab, jj.
//!
//! Every filter is a real parser of the tool's output *shape* (detected from the
//! text itself, so `git show`, `log -p`, `stash show -p` and `jj diff --git` all
//! share one diff compactor and `log --stat` / `commit` / `pull` share one stat
//! compactor). Only hints, progress bars and decoration are dropped outright;
//! everything else that is cut is announced with `more()`.

use super::common::*;

// ─── shared text helpers ──────────────────────────────────────────────────────

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// First 7 chars of a hex sha; any other token unchanged.
fn sha7(s: &str) -> &str {
    if s.len() > 7 && is_hex(s) { &s[..7] } else { s }
}

fn month_num(m: &str) -> Option<u32> {
    Some(match &m.to_ascii_lowercase()[..m.len().min(3)] {
        "jan" => 1, "feb" => 2, "mar" => 3, "apr" => 4, "may" => 5, "jun" => 6,
        "jul" => 7, "aug" => 8, "sep" => 9, "oct" => 10, "nov" => 11, "dec" => 12,
        _ => return None,
    })
}

/// `YYYY-MM-DD…` prefix check (ASCII only, so slicing at 10 is safe).
fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 10
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit)
}

/// Any git date format → `YYYY-MM-DD`; relative dates ("4 days ago") kept.
fn short_date(s: &str) -> String {
    let t: Vec<&str> = s.split_whitespace().collect();
    // default: "Sat Sep 5 16:57:31 2026 +0530"
    if t.len() >= 5 {
        if let (Some(m), Ok(d), Ok(y)) = (month_num(t[1]), t[2].parse::<u32>(), t[4].parse::<u32>()) {
            return format!("{:04}-{:02}-{:02}", y, m, d);
        }
    }
    // rfc2822: "Sat, 5 Sep 2026 16:57:31 +0530"
    if t.len() >= 4 {
        if let (Some(m), Ok(d), Ok(y)) = (month_num(t[2]), t[1].parse::<u32>(), t[3].parse::<u32>()) {
            return format!("{:04}-{:02}-{:02}", y, m, d);
        }
    }
    // iso / iso-strict / short
    if let Some(f) = t.first() {
        if is_iso_date(f) { return f[..10].to_string() }
    }
    s.trim().to_string()
}

/// `2026-09-05T16:57:31Z` table cells → `2026-09-05`.
fn short_ts(cell: &str) -> String {
    if is_iso_date(cell) && cell.len() > 10 && matches!(cell.as_bytes()[10], b'T' | b' ') {
        cell[..10].to_string()
    } else {
        cell.to_string()
    }
}

/// `Name <email>` → `Name`.
fn name_only(s: &str) -> String {
    match s.find('<') {
        Some(i) => s[..i].trim().to_string(),
        None => s.trim().to_string(),
    }
}

/// Text between the first pair of single quotes.
fn quoted<'a>(s: &'a str) -> Option<&'a str> {
    let i = s.find('\'')?;
    let rest = &s[i + 1..];
    let j = rest.find('\'')?;
    Some(&rest[..j])
}

fn first_number(s: &str) -> Option<usize> {
    s.split(|c: char| !c.is_ascii_digit()).find(|w| !w.is_empty())?.parse().ok()
}

/// Blank-free, whitespace-squeezed list (indentation kept as one space), capped.
fn compact_list(output: &str, what: &str) -> String {
    let rows: Vec<String> = output
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let s = squeeze_ws(l);
            if l.starts_with(char::is_whitespace) { format!(" {}", s) } else { s }
        })
        .collect();
    cap_vec(dedupe_consecutive(rows), limits().list_max_lines, what).join("\n")
}

// ─── stat compaction (log --stat, diff --stat, commit, pull) ─────────────────

fn plus_minus(n: usize, plus: usize, minus: usize) -> String {
    if n == 0 { return "0".into() }
    let bar = plus + minus;
    let (a, d, approx) = if bar == n {
        (plus, minus, "")
    } else if bar == 0 {
        return format!("±{}", n); // no bar drawn: split unknown
    } else {
        // git scales the bar to the terminal width; rebuild the split proportionally
        let a = (n * plus + bar / 2) / bar;
        (a, n - a, "~")
    };
    let mut parts = Vec::new();
    if a > 0 { parts.push(format!("+{}", a)) }
    if d > 0 { parts.push(format!("-{}", d)) }
    format!("{}{}", approx, parts.join(" "))
}

/// ` path | 4 ++--` → `path +2 -2`; ` img.png | Bin 0 -> 1234 bytes` → `img.png bin 0→1234`.
fn stat_line(line: &str) -> Option<String> {
    let (left, right) = line.rsplit_once(" | ")?;
    let path = left.trim();
    if path.is_empty() || path.contains(' ') && !path.contains("=>") && !left.starts_with(' ') { return None }
    let mut it = right.split_whitespace();
    let first = it.next()?;
    if first == "Bin" {
        let rest: Vec<&str> = it.collect();
        return Some(match (rest.first(), rest.get(2)) {
            (Some(a), Some(b)) => format!("{} bin {}→{}", path, a, b),
            _ => format!("{} bin", path),
        });
    }
    let n: usize = first.parse().ok()?;
    let bar = it.next().unwrap_or("");
    if it.next().is_some() || bar.chars().any(|c| c != '+' && c != '-') { return None }
    let plus = bar.chars().filter(|&c| c == '+').count();
    let minus = bar.len() - plus;
    Some(format!("{} {}", path, plus_minus(n, plus, minus)))
}

/// ` 3 files changed, 85 insertions(+), 8 deletions(-)` → `3 files, +85 -8`.
fn stat_summary(line: &str) -> Option<String> {
    let t = line.trim();
    let (head, rest) = t.split_once(" changed")?;
    let mut hw = head.split_whitespace();
    let n = hw.next()?;
    n.parse::<usize>().ok()?;
    let noun = hw.next()?;
    if !noun.starts_with("file") || hw.next().is_some() { return None }
    let mut ch = Vec::new();
    for part in rest.split(',') {
        let p = part.trim();
        if let Some(v) = p.strip_suffix("insertions(+)").or_else(|| p.strip_suffix("insertion(+)")) {
            ch.push(format!("+{}", v.trim()));
        } else if let Some(v) = p.strip_suffix("deletions(-)").or_else(|| p.strip_suffix("deletion(-)")) {
            ch.push(format!("-{}", v.trim()));
        }
    }
    let mut s = format!("{} {}", n, noun);
    if !ch.is_empty() {
        s.push_str(", ");
        s.push_str(&ch.join(" "));
    }
    Some(s)
}

/// Pure `--stat` output (diff --stat / show --stat without -p).
fn compact_stat_block(lines: &[&str]) -> String {
    let l = limits();
    let mut files = Vec::new();
    let mut tail = Vec::new();
    for line in lines {
        if line.trim().is_empty() { continue }
        if let Some(s) = stat_summary(line) {
            tail.push(s);
        } else if let Some(s) = stat_line(line) {
            files.push(s);
        } else {
            tail.push(squeeze_ws(line));
        }
    }
    let mut out = tail;
    out.extend(cap_vec(files, l.list_max_lines, "files"));
    out.join("\n")
}

// ─── unified diff compaction ──────────────────────────────────────────────────

const GRAPH_CHARS: &str = "*|/\\_-. ";

#[derive(Default)]
struct FileHdr {
    a: String,
    b: String,
    from: Option<String>,
    to: Option<String>,
    old_mode: Option<String>,
    new_mode: Option<String>,
    new: bool,
    deleted: bool,
    binary: bool,
}

impl FileHdr {
    fn from_git(rest: &str) -> Self {
        // "a/X b/Y" (default prefixes) or "X Y" (--no-prefix)
        let (a, b) = if rest.starts_with("a/") {
            match rest.find(" b/") {
                Some(i) => (&rest[2..i], &rest[i + 3..]),
                None => (rest, ""),
            }
        } else {
            rest.split_once(' ').unwrap_or((rest, ""))
        };
        FileHdr { a: a.trim().to_string(), b: b.trim().to_string(), ..Default::default() }
    }

    fn line(&self) -> String {
        let strip = |p: &str| -> String {
            let p = p.split('\t').next().unwrap_or(p); // plain `diff -u` appends a timestamp
            p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")).unwrap_or(p).to_string()
        };
        let mut name = match (&self.from, &self.to) {
            (Some(f), Some(t)) => format!("{} → {}", f, t),
            _ => {
                let (a, b) = (strip(&self.a), strip(&self.b));
                if b.is_empty() || b == "/dev/null" { a }
                else if a == "/dev/null" || a == b { b }
                else { format!("{} → {}", a, b) }
            }
        };
        let mut tags: Vec<String> = Vec::new();
        if self.new { tags.push("new".into()) }
        if self.deleted { tags.push("deleted".into()) }
        if self.binary { tags.push("binary".into()) }
        if let (Some(o), Some(n)) = (&self.old_mode, &self.new_mode) { tags.push(format!("mode {}→{}", o, n)) }
        if !tags.is_empty() {
            name.push_str(&format!(" ({})", tags.join(", ")));
        }
        name
    }
}

struct Hunk<'a> {
    header: &'a str,
    lines: Vec<&'a str>,
    old: Option<usize>,
    new: Option<usize>,
}

impl<'a> Hunk<'a> {
    fn new(header: &'a str) -> Self {
        // "@@ -a,b +c,d @@ ctx" → remaining line budgets (missing ",n" means 1)
        let mut old = None;
        let mut new = None;
        if !header.starts_with("@@@") {
            for tok in header.split_whitespace().skip(1).take(2) {
                let count = tok[1..].split_once(',').map(|(_, n)| n).unwrap_or("1").parse::<usize>().ok();
                if tok.starts_with('-') { old = count } else if tok.starts_with('+') { new = count }
            }
        }
        Hunk { header, lines: Vec::new(), old, new }
    }

    fn exhausted(&self) -> bool {
        matches!((self.old, self.new), (Some(0), Some(0)))
    }

    /// Consume `line` as hunk content when the budget (or, if unknown, the prefix) says so.
    fn wants(&mut self, line: &str) -> bool {
        if line.starts_with('\\') { return true } // "\ No newline at end of file"
        let (Some(o), Some(n)) = (self.old, self.new) else {
            return line.is_empty() || line.starts_with([' ', '+', '-']) && !line.starts_with("--- ") && !line.starts_with("+++ ");
        };
        if o == 0 && n == 0 { return false }
        match line.chars().next() {
            Some(' ') | None => { self.old = Some(o.saturating_sub(1)); self.new = Some(n.saturating_sub(1)); true }
            Some('-') if o > 0 => { self.old = Some(o - 1); true }
            Some('+') if n > 0 => { self.new = Some(n - 1); true }
            _ => false,
        }
    }
}

/// Emit one hunk with context trimmed to `ctx` lines around each change run.
/// Gaps are marked with `…`; the dropped count is accumulated per file.
fn render_hunk(h: &Hunk, ctx: usize, dropped: &mut usize, prev_fn: &mut String, out: &mut Vec<String>) {
    let (nums, fnctx) = match h.header.strip_prefix("@@").and_then(|r| r.find("@@").map(|i| (i, r))) {
        Some((i, r)) => (&h.header[..i + 4], r[i + 2..].trim()),
        None => (h.header, ""),
    };
    let body: Vec<&str> = h.lines.iter().copied().filter(|l| !l.starts_with('\\')).collect();
    let first_ctx = body.iter().find(|l| l.starts_with(' ')).map(|l| l[1..].trim());
    let keep_fn = !fnctx.is_empty() && fnctx != prev_fn.as_str() && Some(fnctx) != first_ctx;
    out.push(if keep_fn { format!("{} {}", nums.trim_end(), fnctx) } else { nums.trim_end().to_string() });
    if !fnctx.is_empty() { *prev_fn = fnctx.to_string() }

    // split into maximal runs of context / change lines
    let mut runs: Vec<(bool, Vec<&str>)> = Vec::new();
    for l in body {
        let change = l.starts_with(['+', '-']);
        match runs.last_mut() {
            Some((c, v)) if *c == change => v.push(l),
            _ => runs.push((change, vec![l])),
        }
    }
    let n = runs.len();
    let has_change = runs.iter().any(|r| r.0);
    for (k, (change, ls)) in runs.into_iter().enumerate() {
        if change || !has_change {
            out.extend(ls.into_iter().map(str::to_string));
            continue;
        }
        let (head, tail) = if k == 0 { (0, ctx) } else if k == n - 1 { (ctx, 0) } else { (ctx, ctx) };
        if ls.len() <= head + tail {
            out.extend(ls.into_iter().map(str::to_string));
            continue;
        }
        *dropped += ls.len() - head - tail;
        out.extend(ls[..head].iter().map(|s| s.to_string()));
        out.push("…".to_string());
        out.extend(ls[ls.len() - tail..].iter().map(|s| s.to_string()));
    }
}

/// Unified diff → per file: one name line, hunks with trimmed context, one
/// `[+N more context lines]` marker per file. Non-diff lines pass through
/// (stat blocks before the diff are compacted).
fn compact_diff(lines: &[&str]) -> String {
    let ctx = limits().diff_context;
    let mut out: Vec<String> = Vec::new();
    let mut hdr: Option<FileHdr> = None;
    let mut emitted = false;
    let mut dropped = 0usize;
    let mut prev_fn = String::new();
    let mut hunk: Option<Hunk> = None;

    macro_rules! flush_hunk {
        () => {
            if let Some(h) = hunk.take() {
                if let (Some(fh), false) = (hdr.as_ref(), emitted) {
                    if !out.is_empty() { out.push(String::new()) }
                    out.push(fh.line());
                    emitted = true;
                }
                render_hunk(&h, ctx, &mut dropped, &mut prev_fn, &mut out);
            }
        };
    }
    macro_rules! flush_file {
        () => {
            flush_hunk!();
            if let Some(fh) = hdr.take() {
                if !emitted {
                    if !out.is_empty() { out.push(String::new()) }
                    out.push(fh.line());
                }
                if dropped > 0 { out.push(more(dropped, "context lines")) }
            }
            #[allow(unused_assignments)]
            {
                emitted = false;
                dropped = 0;
            }
            prev_fn.clear();
        };
    }

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_end_matches('\r');
        i += 1;
        if let Some(rest) = line.strip_prefix("diff --git ") {
            flush_file!();
            hdr = Some(FileHdr::from_git(rest));
            continue;
        }
        if let Some(rest) = line.strip_prefix("diff --cc ").or_else(|| line.strip_prefix("diff --combined ")) {
            flush_file!();
            hdr = Some(FileHdr { a: rest.to_string(), b: rest.to_string(), ..Default::default() });
            continue;
        }
        if let Some(h) = hunk.as_mut() {
            if h.wants(line) {
                h.lines.push(line);
                continue;
            }
        }
        let at_boundary = hunk.as_ref().map_or(true, Hunk::exhausted);
        if line.starts_with("@@") {
            flush_hunk!();
            hunk = Some(Hunk::new(line));
            continue;
        }
        // plain unified diff (no "diff --git"): "--- X" / "+++ Y" pair opens a file
        if at_boundary && line.starts_with("--- ") && lines.get(i).map_or(false, |n| n.starts_with("+++ ")) && (hdr.is_none() || emitted) {
            flush_file!();
            hdr = Some(FileHdr { a: line[4..].to_string(), b: lines[i][4..].trim_end_matches('\r').to_string(), ..Default::default() });
            i += 1;
            continue;
        }
        if let Some(h) = hdr.as_mut() {
            if !emitted {
                if line.starts_with("index ") || line.starts_with("similarity index") || line.starts_with("dissimilarity index")
                    || line.starts_with("--- ") || line.starts_with("+++ ") { continue }
                if line.starts_with("new file mode") { h.new = true; continue }
                if line.starts_with("deleted file mode") { h.deleted = true; continue }
                if let Some(m) = line.strip_prefix("old mode ") { h.old_mode = Some(m.to_string()); continue }
                if let Some(m) = line.strip_prefix("new mode ") { h.new_mode = Some(m.to_string()); continue }
                if let Some(f) = line.strip_prefix("rename from ").or_else(|| line.strip_prefix("copy from ")) { h.from = Some(f.to_string()); continue }
                if let Some(t) = line.strip_prefix("rename to ").or_else(|| line.strip_prefix("copy to ")) { h.to = Some(t.to_string()); continue }
                if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") { h.binary = true; continue }
            }
        }
        if line.trim().is_empty() { continue }
        // preamble / trailing text: stat blocks compact, anything else verbatim
        if hdr.is_none() {
            if let Some(s) = stat_summary(line).or_else(|| stat_line(line)) { out.push(s); continue }
        }
        flush_hunk!();
        out.push(line.to_string());
    }
    flush_file!();
    out.join("\n")
}

fn looks_like_diff(lines: &[&str]) -> bool {
    lines.iter().any(|l| l.starts_with("diff --git ") || l.starts_with("diff --cc ") || l.starts_with("@@ "))
}

// ─── git status ───────────────────────────────────────────────────────────────

/// "Your branch is ahead of 'origin/main' by 2 commits." → `origin/main +2`
/// (also up to date / behind / gone; the diverged second line is handled by the caller).
fn upstream_note(t: &str) -> Option<String> {
    let up = quoted(t)?;
    if t.starts_with("Your branch is up to date with") { return Some(up.to_string()) }
    if t.starts_with("Your branch is ahead of") { return Some(format!("{} +{}", up, first_number(&t[t.find(" by ")?..])?)) }
    if t.starts_with("Your branch is behind") { return Some(format!("{} -{}", up, first_number(&t[t.find(" by ")?..])?)) }
    if t.starts_with("Your branch is based on") && t.contains("gone") { return Some(format!("{} (gone)", up)) }
    if t.starts_with("Your branch and") && t.contains("diverged") { return Some(up.to_string()) }
    None
}

/// "modified:   path" → ('M', "path"); unmerged kinds map to their XY codes.
fn status_entry(t: &str) -> Option<(&'static str, &str)> {
    let (kind, path) = t.split_once(':')?;
    let path = path.trim();
    let code = match kind.trim() {
        "modified" => "M", "new file" => "A", "deleted" => "D", "renamed" => "R",
        "copied" => "C", "typechange" => "T", "unknown" => "X", "unmerged" => "U",
        "both modified" => "UU", "both added" => "AA", "both deleted" => "DD",
        "added by us" => "AU", "added by them" => "UA", "deleted by us" => "DU", "deleted by them" => "UD",
        _ => return None,
    };
    Some((code, path))
}

#[derive(Default)]
struct StatusModel {
    head: String,          // "main...origin/main +2"
    notes: Vec<String>,    // rebase/merge state lines
    tracked: Vec<(String, [char; 2])>,
    conflicts: Vec<String>,
    untracked: Vec<String>,
    ignored: Vec<String>,
    clean: bool,
}

impl StatusModel {
    fn mark(&mut self, path: &str, code: &str, staged: bool) {
        let c = code.chars().next().unwrap_or(' ');
        let entry = match self.tracked.iter_mut().find(|(p, _)| p == path) {
            Some(e) => e,
            None => { self.tracked.push((path.to_string(), [' ', ' '])); self.tracked.last_mut().unwrap() }
        };
        if staged { entry.1[0] = c } else { entry.1[1] = c }
    }

    fn render(self) -> String {
        let l = limits();
        let mut out: Vec<String> = Vec::new();
        if !self.head.is_empty() { out.push(format!("* {}", self.head)) }
        out.extend(self.notes);
        let empty = self.tracked.is_empty() && self.conflicts.is_empty() && self.untracked.is_empty() && self.ignored.is_empty();
        out.extend(cap_vec(self.conflicts, l.status_max_files, "conflicts"));
        let tracked: Vec<String> = self.tracked.into_iter().map(|(p, xy)| format!("{}{} {}", xy[0], xy[1], p)).collect();
        out.extend(cap_vec(tracked, l.status_max_files, "files"));
        out.extend(cap_vec(self.untracked.into_iter().map(|p| format!("?? {}", p)).collect(), l.status_max_files, "untracked files"));
        out.extend(cap_vec(self.ignored.into_iter().map(|p| format!("!! {}", p)).collect(), l.status_max_files, "ignored files"));
        if empty && (self.clean || out.len() <= 1) { out.push("clean".into()) }
        out.join("\n")
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Sec { None, Staged, Unstaged, Unmerged, Untracked, Ignored }

fn status_long(lines: &[&str]) -> String {
    let mut m = StatusModel::default();
    let mut sec = Sec::None;
    let mut branch = String::new();
    let mut upstream: Option<String> = None;
    let mut no_commits = false;
    for &line in lines {
        let t = line.trim();
        if t.is_empty() { continue }
        if let Some(b) = t.strip_prefix("On branch ") { branch = b.to_string(); continue }
        if t.starts_with("HEAD detached") || t.starts_with("Not currently on any branch") { branch = t.trim_end_matches('.').to_string(); continue }
        if t == "No commits yet" { no_commits = true; continue }
        if let Some(n) = upstream_note(t) { upstream = Some(n); continue }
        if t.starts_with("and have ") && t.contains("different commits") {
            // diverged: "and have 2 and 3 different commits each, respectively."
            let nums: Vec<usize> = t.split_whitespace().filter_map(|w| w.parse().ok()).collect();
            if let (Some(up), Some(a), Some(b)) = (upstream.as_mut(), nums.first(), nums.get(1)) {
                up.push_str(&format!(" +{} -{}", a, b));
            }
            continue;
        }
        if t.starts_with('(') { continue } // every '(use "git …")' hint
        if t.starts_with("nothing to commit") { m.clean = true; continue }
        if t.starts_with("nothing added to commit") || t.starts_with("no changes added to commit") { continue }
        match t {
            "Changes to be committed:" => { sec = Sec::Staged; continue }
            "Changes not staged for commit:" => { sec = Sec::Unstaged; continue }
            "Unmerged paths:" => { sec = Sec::Unmerged; continue }
            "Untracked files:" => { sec = Sec::Untracked; continue }
            "Ignored files:" => { sec = Sec::Ignored; continue }
            _ => {}
        }
        let indented = line.starts_with('\t') || line.starts_with("        ");
        if indented && sec != Sec::None {
            match sec {
                Sec::Staged | Sec::Unstaged => match status_entry(t) {
                    Some((code, path)) => m.mark(path, code, sec == Sec::Staged),
                    None => m.notes.push(squeeze_ws(t)),
                },
                Sec::Unmerged => match status_entry(t) {
                    Some((code, path)) => m.conflicts.push(format!("{} {}", code, path)),
                    None => m.notes.push(squeeze_ws(t)),
                },
                Sec::Untracked => m.untracked.push(t.to_string()),
                Sec::Ignored => m.ignored.push(t.to_string()),
                Sec::None => {}
            }
            continue;
        }
        m.notes.push(squeeze_ws(t)); // rebase/merge progress, "You have unmerged paths.", errors
    }
    let mut head = branch;
    if let Some(up) = upstream {
        if !head.is_empty() { head.push_str("..."); head.push_str(&up) }
    }
    if no_commits { head.push_str(" (no commits)") }
    m.head = head;
    m.render()
}

fn status_short(lines: &[&str]) -> String {
    let mut m = StatusModel::default();
    for &line in lines {
        if let Some(b) = line.strip_prefix("## ") {
            let b = b.trim();
            m.head = if let Some(rest) = b.strip_prefix("No commits yet on ") {
                format!("{} (no commits)", rest)
            } else if b == "HEAD (no branch)" {
                "HEAD (detached)".to_string()
            } else {
                // "main...origin/main [ahead 2, behind 1]" → "main...origin/main +2 -1"
                let (name, track) = b.split_once(" [").unwrap_or((b, ""));
                let mut h = name.to_string();
                for part in track.trim_end_matches(']').split(',') {
                    let p = part.trim();
                    if let Some(n) = p.strip_prefix("ahead ") { h.push_str(&format!(" +{}", n)) }
                    else if let Some(n) = p.strip_prefix("behind ") { h.push_str(&format!(" -{}", n)) }
                    else if p == "gone" { h.push_str(" (gone)") }
                }
                h
            };
            continue;
        }
        if line.len() < 3 { continue }
        let (code, path) = (&line[..2], line[3..].trim_end());
        match code {
            "??" => m.untracked.push(path.to_string()),
            "!!" => m.ignored.push(path.to_string()),
            "UU" | "AA" | "DD" | "AU" | "UA" | "DU" | "UD" => m.conflicts.push(format!("{} {}", code, path)),
            _ => {
                let mut cs = code.chars();
                let xy = [cs.next().unwrap_or(' '), cs.next().unwrap_or(' ')];
                m.tracked.push((path.to_string(), xy));
            }
        }
    }
    m.clean = true;
    m.render()
}

fn looks_porcelain(lines: &[&str]) -> bool {
    let mut any = false;
    for l in lines.iter().filter(|l| !l.trim().is_empty()) {
        if l.starts_with("## ") { any = true; continue }
        let b = l.as_bytes();
        if b.len() < 4 || b[2] != b' ' || !b" MADRCUT?!".contains(&b[0]) || !b" MADRCUT?!".contains(&b[1]) { return false }
        any = true;
    }
    any
}

pub(crate) fn filter_git_status(output: &str) -> String {
    let lines: Vec<&str> = output.lines().map(|l| l.trim_end_matches('\r')).collect();
    if lines.iter().all(|l| l.trim().is_empty()) { return "clean".into() }
    let first = lines.iter().find(|l| !l.trim().is_empty()).copied().unwrap_or("");
    if looks_porcelain(&lines) { return status_short(&lines) }
    if first.starts_with("On branch ") || first.starts_with("HEAD detached") || first.starts_with("Not currently on any branch")
        || first.starts_with("interactive rebase in progress") || first.starts_with("rebase in progress") {
        return status_long(&lines);
    }
    generic(output)
}

// ─── git diff / show / log ────────────────────────────────────────────────────

pub(crate) fn filter_git_diff(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.iter().all(|l| l.trim().is_empty()) { return String::new() }
    if looks_like_diff(&lines) { return compact_diff(&lines) }
    if lines.iter().any(|l| stat_line(l).is_some() || stat_summary(l).is_some()) { return compact_stat_block(&lines) }
    // --numstat / --name-status: tab columns → single space
    compact_list(output, "lines")
}

struct Commit<'a> {
    graph: String,
    sha: String,
    decor: String,
    merge: Vec<String>,
    author: String,
    date: String,
    msg: Vec<&'a str>,
    stats: Vec<String>,
    summary: Option<String>,
    extra: Vec<String>,
    diff: Vec<&'a str>,
}

/// `[graph] commit <sha> [(decorations)]` → (graph prefix, sha, decorations).
fn commit_header(line: &str) -> Option<(&str, &str, &str)> {
    let idx = line.find("commit ")?;
    let prefix = &line[..idx];
    // message bodies are indented by spaces only; a graph prefix always carries '*'
    if !(prefix.is_empty() || (prefix.contains('*') && prefix.chars().all(|c| GRAPH_CHARS.contains(c)))) { return None }
    let rest = &line[idx + 7..];
    let (sha, decor) = rest.split_once(' ').unwrap_or((rest, ""));
    if sha.len() < 7 || !is_hex(sha) { return None }
    Some((prefix, sha, decor.trim()))
}

fn strip_graph<'a>(line: &'a str, w: usize) -> &'a str {
    if w == 0 { return line }
    let ok = line.len() >= w && line.is_char_boundary(w) && line[..w].chars().all(|c| GRAPH_CHARS.contains(c));
    if ok { &line[w..] } else { line }
}

fn parse_log(output: &str) -> Option<(Vec<String>, Vec<Commit<'_>>)> {
    let mut commits: Vec<Commit> = Vec::new();
    let mut preamble = Vec::new();
    let mut w = 0usize;
    let mut in_diff = false;
    for raw in output.lines() {
        let line = raw.trim_end_matches('\r');
        if let Some((prefix, sha, decor)) = commit_header(line) {
            commits.push(Commit {
                graph: prefix.to_string(), sha: sha.to_string(), decor: decor.to_string(),
                merge: Vec::new(), author: String::new(), date: String::new(), msg: Vec::new(),
                stats: Vec::new(), summary: None, extra: Vec::new(), diff: Vec::new(),
            });
            w = prefix.len();
            in_diff = false;
            continue;
        }
        let Some(c) = commits.last_mut() else {
            if !line.trim().is_empty() { preamble.push(squeeze_ws(line)) }
            continue;
        };
        let content = strip_graph(line, w);
        if in_diff { c.diff.push(content); continue }
        if content.trim().is_empty() { continue }
        if let Some(r) = content.strip_prefix("Merge: ") {
            c.merge = r.split_whitespace().map(|s| sha7(s).to_string()).collect();
        } else if let Some(r) = content.strip_prefix("Author: ") {
            c.author = name_only(r);
        } else if let Some(r) = content.strip_prefix("Date: ").or_else(|| content.strip_prefix("AuthorDate: ")) {
            c.date = short_date(r);
        } else if content.starts_with("Commit: ") || content.starts_with("CommitDate: ") {
            // committer identity: low-value duplicate of Author in practice
        } else if let Some(m) = content.strip_prefix("    ") {
            c.msg.push(m.trim_end());
        } else if content.starts_with("diff --git ") || content.starts_with("diff --cc ") {
            in_diff = true;
            c.diff.push(content);
        } else if let Some(s) = stat_summary(content) {
            c.summary = Some(s);
        } else if let Some(s) = stat_line(content) {
            c.stats.push(s);
        } else {
            c.extra.push(squeeze_ws(content)); // --name-only paths, --name-status, notes
        }
    }
    if commits.is_empty() { None } else { Some((preamble, commits)) }
}

fn render_log(preamble: Vec<String>, commits: Vec<Commit>) -> String {
    let l = limits();
    let mut out: Vec<String> = preamble;
    let same_author = commits.len() > 1 && !commits[0].author.is_empty() && commits.iter().all(|c| c.author == commits[0].author);
    if same_author { out.push(format!("author: {}", commits[0].author)) }
    let total = commits.len();
    for c in commits.into_iter().take(l.list_max_lines) {
        let mut h = format!("{}{}", c.graph, sha7(&c.sha));
        if !c.decor.is_empty() { h.push(' '); h.push_str(&c.decor) }
        if let Some(subject) = c.msg.iter().find(|m| !m.trim().is_empty()) { h.push(' '); h.push_str(subject.trim()) }
        if !c.date.is_empty() { h.push_str(&format!(" ({})", c.date)) }
        if !same_author && !c.author.is_empty() { h.push_str(&format!(" <{}>", c.author)) }
        if !c.merge.is_empty() { h.push_str(&format!(" (merge {})", c.merge.join(" "))) }
        if let Some(s) = &c.summary { h.push_str(&format!(" [{}]", s)) }
        let body: Vec<&str> = c.msg.iter().skip_while(|m| m.trim().is_empty()).skip(1).filter(|m| !m.trim().is_empty()).copied().collect();
        if body.len() > 1 { h.push(' '); h.push_str(&more(body.len(), "body lines")) }
        out.push(h);
        if body.len() == 1 { out.push(format!("  {}", body[0].trim())) }
        out.extend(c.stats.iter().map(|s| format!(" {}", s)));
        out.extend(c.extra.iter().map(|s| format!(" {}", s)));
        if !c.diff.is_empty() { out.push(compact_diff(&c.diff)) }
    }
    if total > l.list_max_lines { out.push(more(total - l.list_max_lines, "commits")) }
    out.join("\n")
}

/// `--oneline`-shaped output: every non-empty line is graph chars + sha + subject.
fn looks_oneline(lines: &[&str]) -> bool {
    let mut any = false;
    for l in lines.iter().filter(|l| !l.trim().is_empty()) {
        let rest = l.trim_start_matches(|c| GRAPH_CHARS.contains(c));
        if rest.is_empty() { continue } // pure graph connector
        let sha = rest.split_whitespace().next().unwrap_or("");
        if sha.len() < 7 || !is_hex(sha) { return false }
        any = true;
    }
    any
}

pub(crate) fn filter_git_log(output: &str) -> String {
    if let Some((pre, commits)) = parse_log(output) { return render_log(pre, commits) }
    let lines: Vec<&str> = output.lines().collect();
    if looks_oneline(&lines) {
        return cap_lines(lines.iter().map(|l| l.trim_end()).filter(|l| !l.is_empty()), limits().list_max_lines, "lines");
    }
    generic(output)
}

// ─── git branch / remote / blame / reflog ─────────────────────────────────────

pub(crate) fn filter_git_branch(output: &str) -> String {
    let l = limits();
    let mut current: Vec<String> = Vec::new();
    let mut locals: Vec<String> = Vec::new();
    let mut remotes: Vec<(String, Vec<String>)> = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() { continue }
        let (cur, name) = match t.strip_prefix("* ") { Some(r) => (true, r), None => (false, t.strip_prefix("+ ").unwrap_or(t)) };
        let name = squeeze_ws(name);
        if let Some((remote, rest)) = name.strip_prefix("remotes/").and_then(|r| r.split_once('/')) {
            match remotes.iter_mut().find(|(r, _)| r == remote) {
                Some((_, v)) => v.push(rest.to_string()),
                None => remotes.push((remote.to_string(), vec![rest.to_string()])),
            }
        } else if cur {
            current.push(format!("* {}", name));
        } else {
            locals.push(name);
        }
    }
    let mut out = current;
    out.extend(cap_vec(locals, l.list_max_lines, "branches"));
    for (remote, names) in remotes {
        out.push(format!("remotes/{}: {}", remote, cap_vec(names, l.list_max_lines, "branches").join(", ")));
    }
    out.join("\n")
}

pub(crate) fn filter_git_remote(output: &str) -> String {
    let mut entries: Vec<(String, String, Vec<&str>)> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    for line in output.lines() {
        let p: Vec<&str> = line.split_whitespace().collect();
        if p.len() == 3 && matches!(p[2], "(fetch)" | "(push)") {
            match entries.iter_mut().find(|(n, u, _)| n == p[0] && u == p[1]) {
                Some((_, _, k)) => k.push(p[2]),
                None => entries.push((p[0].to_string(), p[1].to_string(), vec![p[2]])),
            }
        } else if !p.is_empty() {
            other.push(if line.starts_with(char::is_whitespace) { format!(" {}", p.join(" ")) } else { p.join(" ") });
        }
    }
    let mut out: Vec<String> = entries
        .into_iter()
        .map(|(n, u, k)| if k.len() == 2 { format!("{} {}", n, u) } else { format!("{} {} {}", n, u, k.join(" ")) })
        .collect();
    out.extend(other);
    cap_vec(out, limits().list_max_lines, "lines").join("\n")
}

/// `sha (Author 2026-09-05 16:57:31 +0530 12) content` → runs grouped by commit:
/// `sha7 Author 2026-09-05` header, then `  12| content` lines.
fn filter_git_blame(output: &str) -> String {
    let l = limits();
    let mut runs: Vec<(String, Vec<(String, String)>)> = Vec::new(); // (header, [(lineno, content)])
    for line in output.lines() {
        let Some((sha, rest)) = line.split_once(' ') else { return generic(output) };
        if !is_hex(sha.trim_start_matches('^')) { return generic(output) }
        let Some(lp) = rest.find('(') else { return generic(output) };
        let Some(rp) = rest[lp..].find(')').map(|i| i + lp) else { return generic(output) };
        let meta: Vec<&str> = rest[lp + 1..rp].split_whitespace().collect();
        let content = rest.get(rp + 1..).map(|c| c.strip_prefix(' ').unwrap_or(c)).unwrap_or("");
        let Some((lineno, meta)) = meta.split_last() else { return generic(output) };
        // author = tokens before the date; date only (time/tz dropped)
        let mut header = if sha.len() > 8 { format!("{}{}", if sha.starts_with('^') { "^" } else { "" }, sha7(sha.trim_start_matches('^'))) } else { sha.to_string() };
        match meta.iter().position(|t| is_iso_date(t)) {
            Some(di) => {
                let author = meta[..di].join(" ");
                if !author.is_empty() { header.push(' '); header.push_str(&author) }
                header.push(' ');
                header.push_str(&short_date(meta[di]));
            }
            None if !meta.is_empty() => { header.push(' '); header.push_str(&meta.join(" ")) }
            None => {}
        }
        match runs.last_mut() {
            Some((h, v)) if *h == header => v.push((lineno.to_string(), content.to_string())),
            _ => runs.push((header, vec![(lineno.to_string(), content.to_string())])),
        }
    }
    let width = runs.iter().flat_map(|(_, v)| v.iter()).map(|(n, _)| n.len()).max().unwrap_or(1);
    let mut out: Vec<String> = Vec::new();
    let mut shown = 0usize;
    let mut cut = 0usize;
    for (h, v) in runs {
        if shown >= l.passthrough_max_lines { cut += v.len(); continue }
        out.push(h);
        for (n, c) in v {
            if shown >= l.passthrough_max_lines { cut += 1; continue }
            out.push(format!("{:>w$}| {}", n, c, w = width));
            shown += 1;
        }
    }
    if cut > 0 { out.push(more(cut, "lines")) }
    out.join("\n")
}

fn filter_git_reflog(output: &str) -> String {
    let rows: Vec<String> = output
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| squeeze_ws(&l.replacen("HEAD@{", "@{", 1)))
        .collect();
    cap_vec(rows, limits().list_max_lines, "entries").join("\n")
}

// ─── git fetch / pull / push / clone ──────────────────────────────────────────

const PROGRESS: &[&str] = &[
    "Enumerating objects", "Counting objects", "Compressing objects", "Writing objects",
    "Receiving objects", "Resolving deltas", "Unpacking objects", "Total ", "Delta compression",
    "Auto packing", "Checking connectivity", "Updating files:", "Filtering content:", "Rebasing (",
    "Checking out files:", "remote: Enumerating", "remote: Counting", "remote: Compressing",
    "remote: Total", "remote: Resolving", "remote: Finding", "Fetching ", "Everything up-to-date",
];

const DETACHED_ADVICE: &[&str] = &[
    "detached HEAD' state", "You can look around, make experimental", "changes and commit them, and you can discard",
    "state without impacting any branches", "If you want to create a new branch to retain",
    "do so (now or later) by using", "git switch -c <new-branch-name>", "git checkout -b <new-branch-name>",
    "Or undo this operation with:", "Turn off this advice by setting config variable",
];

fn is_progress(t: &str) -> bool {
    if t == "Everything up-to-date" { return false }
    PROGRESS.iter().any(|p| t.starts_with(p)) && !t.starts_with("Total ") || t.starts_with("Total ") && t.contains("delta")
}

/// Shared tail for result-style commands: hints collapse to first line + marker,
/// stat blocks compact, `[branch sha] subject` folds with its summary.
fn compact_result(output: &str) -> String {
    let l = limits();
    let mut out: Vec<String> = Vec::new();
    let mut hints: Vec<String> = Vec::new();
    let mut created: Vec<String> = Vec::new();
    let mut deleted: Vec<String> = Vec::new();
    let mut renamed: Vec<String> = Vec::new();
    let mut crlf: Vec<String> = Vec::new();
    let mut pending_commit: Option<usize> = None; // index of "[main abc] subject" line awaiting its summary
    let mut diverged = false;
    for raw in output.split(['\n', '\r']) {
        let line = raw.trim_end();
        let t = line.trim();
        if t.is_empty() || is_progress(t) || t.starts_with('(') && t.contains("git ") { continue }
        if DETACHED_ADVICE.iter().any(|a| t.contains(a)) { continue }
        if let Some(h) = t.strip_prefix("hint: ") { if !h.trim().is_empty() { hints.push(h.to_string()) } continue }
        if let Some(r) = t.strip_prefix("remote: ") { if !r.trim().is_empty() { out.push(r.trim_end().to_string()) } continue }
        if t.starts_with("Your branch and") && t.contains("diverged") { diverged = true; }
        if let Some(n) = upstream_note(t) {
            match out.last_mut() {
                Some(prev) if prev.starts_with("Switched") || prev.starts_with("Already on") => prev.push_str(&format!(" [{}]", n)),
                _ => out.push(format!("...{}", n)),
            }
            continue;
        }
        if diverged && t.starts_with("and have ") {
            let nums: Vec<usize> = t.split_whitespace().filter_map(|w| w.parse().ok()).collect();
            if let (Some(prev), Some(a), Some(b)) = (out.last_mut(), nums.first(), nums.get(1)) { prev.push_str(&format!(" +{} -{}", a, b)) }
            continue;
        }
        if t.starts_with('[') && t.contains("] ") && line.starts_with('[') {
            // "[main abc1234] subject" → "main abc1234 subject"
            let (head, subject) = t[1..].split_once("] ").unwrap_or((&t[1..], ""));
            out.push(format!("{} {}", head, subject));
            pending_commit = Some(out.len() - 1);
            continue;
        }
        if let Some(s) = stat_summary(t) {
            match pending_commit.take().and_then(|i| out.get_mut(i)) {
                Some(c) => c.push_str(&format!("; {}", s)),
                None => out.push(s),
            }
            continue;
        }
        if let Some(s) = stat_line(line) { out.push(format!(" {}", s)); continue }
        if let Some(p) = t.strip_prefix("create mode ") { created.push(p.split_once(' ').map_or(p, |x| x.1).to_string()); continue }
        if let Some(p) = t.strip_prefix("delete mode ") { deleted.push(p.split_once(' ').map_or(p, |x| x.1).to_string()); continue }
        if let Some(p) = t.strip_prefix("rename ").or_else(|| t.strip_prefix("copy ")) { renamed.push(p.replace(" => ", " → ")); continue }
        if t.starts_with("warning: in the working copy of '") && t.contains("will be replaced") {
            if let Some(p) = quoted(t) { crlf.push(p.to_string()) }
            continue;
        }
        if t.starts_with("warning: LF will be replaced") || t.starts_with("warning: CRLF will be replaced") { crlf.push(t.trim_start_matches("warning: ").to_string()); continue }
        if t.starts_with("The file will have its original line endings") { continue }
        out.push(squeeze_ws(line));
    }
    if !created.is_empty() { out.push(format!("created: {}", cap_vec(created, l.status_max_files, "files").join(", "))) }
    if !deleted.is_empty() { out.push(format!("deleted: {}", cap_vec(deleted, l.status_max_files, "files").join(", "))) }
    if !renamed.is_empty() { out.push(format!("renamed: {}", cap_vec(renamed, l.status_max_files, "files").join(", "))) }
    if !crlf.is_empty() { out.push(format!("warning: line endings will be replaced: {}", cap_vec(crlf, l.status_max_files, "files").join(", "))) }
    if let Some(first) = hints.first() {
        let mut h = format!("hint: {}", first);
        if hints.len() > 1 { h.push(' '); h.push_str(&more(hints.len() - 1, "hint lines")) }
        out.push(h);
    }
    cap_vec(dedupe_consecutive(out), l.passthrough_max_lines, "lines").join("\n")
}

fn filter_git_result(output: &str) -> String {
    if output.lines().any(|l| l.starts_with("On branch ") || l.starts_with("HEAD detached")) {
        // commit with nothing staged / stash pop print a full status block
        return filter_git_status(output);
    }
    compact_result(output)
}

// ─── git dispatch ─────────────────────────────────────────────────────────────

pub(crate) fn filter_git(args: &[&str], output: &str) -> String {
    let sub = find_subcommand(args);
    let second = sub.and_then(|s| {
        let i = args.iter().position(|a| *a == s)?;
        args[i + 1..].iter().find(|a| !a.starts_with('-')).copied()
    });
    match sub {
        Some("status" | "st") => filter_git_status(output),
        Some("diff" | "difftool" | "range-diff" | "format-patch" | "apply") => filter_git_diff(output),
        Some("log" | "show" | "whatchanged") => filter_git_log(output),
        Some("branch") => filter_git_branch(output),
        Some("remote") => filter_git_remote(output),
        Some("stash") => match second {
            Some("show") => filter_git_diff(output),
            Some("list") | None => compact_list(output, "stashes"),
            _ => filter_git_result(output),
        },
        Some("blame" | "annotate") => filter_git_blame(output),
        Some("reflog") => filter_git_reflog(output),
        Some("fetch" | "pull" | "push" | "clone" | "submodule") => compact_result(output),
        Some("add" | "commit" | "checkout" | "switch" | "merge" | "rebase" | "cherry-pick" | "revert"
            | "reset" | "restore" | "rm" | "mv" | "init" | "am" | "bisect" | "clean" | "gc" | "prune") => filter_git_result(output),
        Some("tag") => compact_list(output, "tags"),
        Some("worktree") => compact_list(output, "worktrees"),
        Some("shortlog") => compact_list(output, "lines"),
        Some("ls-files" | "ls-tree" | "ls-remote" | "rev-list" | "rev-parse" | "for-each-ref" | "config"
            | "describe" | "grep" | "show-ref" | "cat-file" | "count-objects" | "name-rev") => compact_list(output, "lines"),
        _ => generic(output),
    }
}

// ─── GH / GLAB tables ─────────────────────────────────────────────────────────

fn split_cols(line: &str) -> Vec<String> {
    if line.contains('\t') {
        line.split('\t').map(|c| c.trim().to_string()).collect()
    } else {
        line.split("  ").map(str::trim).filter(|c| !c.is_empty()).map(str::to_string).collect()
    }
}

fn is_header_row(cells: &[String]) -> bool {
    cells.len() > 1 && cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_uppercase() || ch == ' ' || ch == '_' || ch == '#'))
}

/// gh/glab tab-separated tables → `a  b  c` rows, one header at most, dates shortened.
/// `checks` drops the job URL for passing checks (still reachable via `--json`).
fn table_rows(output: &str, what: &str, checks: bool) -> String {
    let mut rows: Vec<String> = Vec::new();
    let mut header = false;
    for line in output.lines() {
        if line.trim().is_empty() { continue }
        let mut cells = split_cols(line.trim_end());
        if is_header_row(&cells) {
            if header { continue }
            header = true;
        }
        for c in cells.iter_mut() { *c = short_ts(c) }
        if checks && cells.get(1).map_or(false, |s| matches!(s.as_str(), "pass" | "success" | "skipping")) {
            cells.retain(|c| !c.starts_with("http"));
        }
        cells.retain(|c| !c.is_empty());
        rows.push(cells.join("  "));
    }
    cap_vec(dedupe_consecutive(rows), limits().list_max_lines, what).join("\n")
}

/// `gh pr view` / `gh issue view` (non-TTY: `key:\tvalue` block, `--`, body).
/// `gh … view`: the metadata block is the payload; the `--`-separated body is a
/// README / PR description that an agent can fetch on demand, so it is summarised.
fn kv_view(output: &str) -> String {
    let l = limits();
    const BODY_LINES: usize = 10;
    let mut out: Vec<String> = Vec::new();
    let mut body: Vec<&str> = Vec::new();
    let mut in_body = false;
    for line in output.lines() {
        if !in_body {
            if line.trim() == "--" {
                in_body = true;
                continue;
            }
            if let Some((k, v)) = line.split_once(['\t', ':']) {
                let v = v.trim();
                if v.is_empty() { continue }
                out.push(format!("{}: {}", k.trim().trim_end_matches(':'), short_ts(v)));
                continue;
            }
        }
        if in_body {
            body.push(line.trim_end());
        } else {
            out.push(line.trim_end().to_string());
        }
    }
    if !body.is_empty() {
        let kept: Vec<&str> = body.iter().copied().filter(|l| !l.trim().is_empty()).collect();
        let shown = kept.len().min(BODY_LINES);
        out.push("--".to_string());
        out.extend(kept[..shown].iter().map(|l| truncate(l, 200)));
        if kept.len() > shown {
            out.push(more(kept.len() - shown, "body lines"));
        }
    }
    cap_lines(collapse_blank(&out.join("\n")).lines(), l.passthrough_max_lines, "lines")
}

pub(crate) fn filter_gh(args: &[&str], output: &str) -> String {
    compact_json_output(output, |o| {
        match (args.first().copied(), args.get(1).copied()) {
            (Some("api"), _) => generic(o),
            (Some("pr"), Some("checks")) => table_rows(o, "checks", true),
            (Some("pr" | "issue" | "run" | "release" | "repo" | "gist" | "workflow" | "label" | "search"), Some("list" | "ls" | "repos" | "issues" | "prs")) => table_rows(o, "rows", false),
            (Some("pr" | "issue" | "release" | "run" | "repo" | "gist" | "workflow"), Some("view")) => kv_view(o),
            (Some("repo"), _) => kv_view(o),
            (Some("pr" | "issue"), Some("status")) => compact_list(o, "lines"),
            _ => generic(o),
        }
    })
}

pub(crate) fn filter_glab(args: &[&str], output: &str) -> String {
    compact_json_output(output, |o| {
        match (args.first().copied(), args.get(1).copied()) {
            (Some("api"), _) => generic(o),
            (Some("mr" | "issue" | "ci" | "pipeline" | "release" | "repo"), Some("list" | "ls")) => table_rows(o, "rows", false),
            (Some("mr" | "issue" | "release"), Some("view")) => kv_view(o),
            (Some("ci" | "pipeline"), Some("status")) => compact_list(o, "lines"),
            _ => generic(o),
        }
    })
}

// ─── JJ (Jujutsu) ─────────────────────────────────────────────────────────────

const JJ_GRAPH: &str = "@○◆×│├┤─╮╯╭╰┐┘┌└~| ";

/// jj log: header (`change-id author date time commit-id`) + description line
/// pairs fold to one line; timestamps keep the date only; connectors dropped.
fn filter_jj_log(output: &str) -> String {
    let mut rows: Vec<String> = Vec::new();
    let mut open_header = false;
    for line in output.lines() {
        let content = line.trim_start_matches(|c| JJ_GRAPH.contains(c)).trim_end();
        if content.is_empty() { continue }
        let sym = line.chars().next().filter(|c| "@○◆×".contains(*c));
        let toks: Vec<&str> = content.split_whitespace().collect();
        let is_header = toks.iter().enumerate().any(|(i, t)| is_iso_date(t) && toks.get(i + 1).map_or(false, |n| n.contains(':')));
        if is_header {
            let mut h: Vec<String> = Vec::new();
            let mut skip_time = false;
            for t in &toks {
                if skip_time { skip_time = false; continue }
                if is_iso_date(t) { h.push(short_date(t)); skip_time = true } else { h.push(t.to_string()) }
            }
            rows.push(format!("{}{}", sym.map(|s| format!("{} ", s)).unwrap_or_default(), h.join(" ")));
            open_header = true;
        } else if open_header {
            if let Some(last) = rows.last_mut() { last.push_str(": "); last.push_str(&squeeze_ws(content)) }
            open_header = false;
        } else {
            rows.push(squeeze_ws(content));
        }
    }
    cap_vec(rows, limits().list_max_lines, "changes").join("\n")
}

pub(crate) fn filter_jj(args: &[&str], output: &str) -> String {
    match find_subcommand(args) {
        Some("log" | "evolog" | "op") => filter_jj_log(output),
        Some("diff" | "show" | "interdiff") => {
            let lines: Vec<&str> = output.lines().collect();
            if looks_like_diff(&lines) { compact_diff(&lines) } else { compact_list(output, "lines") }
        }
        Some("status" | "st" | "bookmark" | "branch" | "git" | "workspace" | "files") => compact_list(output, "lines"),
        _ => generic(output),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_long_form_becomes_porcelain_like() {
        let out = filter_git(
            &["status"],
            "On branch main\nYour branch is up to date with 'origin/main'.\n\nChanges not staged for commit:\n  (use \"git add <file>...\" to update what will be committed)\n  (use \"git restore <file>...\" to discard changes in working directory)\n\tmodified:   src/cli.rs\n\tdeleted:    src/filter.rs\n\nUntracked files:\n  (use \"git add <file>...\" to include in what will be committed)\n\tbench/\n\nno changes added to commit (use \"git add\" and/or \"git commit -a\")",
        );
        assert!(out.contains("main"), "{}", out);
        assert!(out.contains("src/cli.rs"), "{}", out);
        assert!(out.contains("src/filter.rs"), "{}", out);
        assert!(out.contains("bench/"), "{}", out);
        // hint lines carry nothing for an agent
        assert!(!out.contains("use \"git add"), "{}", out);
        assert!(!out.contains("no changes added"), "{}", out);
    }

    #[test]
    fn status_clean_and_porcelain_pass_through() {
        let clean = filter_git(&["status"], "On branch main\nYour branch is up to date with 'origin/main'.\n\nnothing to commit, working tree clean");
        assert!(clean.contains("main"), "{}", clean);
        assert!(clean.to_lowercase().contains("clean"), "{}", clean);
        let porcelain = filter_git(&["status", "--short", "--branch"], "## main...origin/main\n M src/cli.rs\n?? bench/");
        assert!(porcelain.contains("main...origin/main"), "{}", porcelain);
        assert!(porcelain.contains("M src/cli.rs"), "{}", porcelain);
        assert!(porcelain.contains("?? bench/"), "{}", porcelain);
    }

    #[test]
    fn status_file_cap_is_announced() {
        let mut raw = String::from("On branch main\n\nChanges not staged for commit:\n");
        for i in 0..80 {
            raw.push_str(&format!("\tmodified:   src/f{}.rs\n", i));
        }
        let out = filter_git(&["status"], &raw);
        let shown = out.lines().filter(|l| l.contains("src/f")).count();
        assert!(shown < 80, "nothing was capped: {}", out);
        assert!(has_truncation(&out), "{}", out);
    }

    #[test]
    fn log_default_format_becomes_one_line_per_commit() {
        let out = filter_git(
            &["log", "-2"],
            "commit 5b641e3240792aa59ddaa6f831c1dec86706093b\nAuthor: Anshu Kushwaha <kushawahaanshu8858@gmail.com>\nDate:   Sat Sep 5 16:57:31 2026 +0530\n\n    fix(stability): add proxy loopback guard\n\n    Body line one.\n    Body line two.\n\ncommit 365096f0000000000000000000000000000000aa\nAuthor: Anshu Kushwaha <kushawahaanshu8858@gmail.com>\nDate:   Fri Sep 4 10:00:00 2026 +0530\n\n    feat(ports): migrate default ports\n",
        );
        assert!(out.contains("5b641e3"), "{}", out);
        assert!(out.contains("fix(stability): add proxy loopback guard"), "{}", out);
        assert!(out.contains("365096f"), "{}", out);
        assert!(out.contains("feat(ports): migrate default ports"), "{}", out);
        // the 40-char sha, the e-mail and the Date: label are all redundant
        assert!(!out.contains("5b641e3240792aa59ddaa6f831c1dec86706093b"), "{}", out);
        assert!(!out.contains("kushawahaanshu8858@gmail.com"), "{}", out);
        assert_eq!(out.lines().filter(|l| l.starts_with("commit ")).count(), 0, "{}", out);
    }

    #[test]
    fn log_body_lines_are_counted_not_dropped_silently() {
        let out = filter_git(
            &["log", "-1"],
            "commit abc1234000000000000000000000000000000000\nAuthor: A <a@b.c>\nDate:   Sat Sep 5 16:57:31 2026 +0530\n\n    subject here\n\n    body one\n    body two\n    body three\n",
        );
        assert!(out.contains("subject here"), "{}", out);
        assert!(has_truncation(&out) || out.contains("body"), "body lines vanished: {}", out);
    }

    #[test]
    fn log_stat_compacts_bar_charts() {
        let out = filter_git(
            &["log", "--stat", "-1"],
            "commit abc1234000000000000000000000000000000000\nAuthor: A <a@b.c>\nDate:   Sat Sep 5 16:57:31 2026 +0530\n\n    subject\n\n CHANGELOG.md     |  4 ++++\n src/analytics.rs | 18 +++++++++++---\n 3 files changed, 85 insertions(+), 8 deletions(-)\n",
        );
        assert!(out.contains("CHANGELOG.md"), "{}", out);
        assert!(out.contains("src/analytics.rs"), "{}", out);
        assert!(out.contains("85") && out.contains('8'), "{}", out);
        assert!(!out.contains("++++"), "bars kept: {}", out);
    }

    #[test]
    fn diff_collapses_headers_keeps_every_change_line() {
        let raw = "diff --git a/src/a.rs b/src/a.rs\nindex 1111111..2222222 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,6 +1,6 @@ fn main() {\n context one\n context two\n context three\n-removed line\n+added line\n context four\n context five\n context six\ndiff --git a/new.txt b/new.txt\nnew file mode 100644\nindex 0000000..3333333\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hello\ndiff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\nindex 4444444..0000000\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n";
        let out = filter_git(&["diff"], raw);
        assert!(out.contains("src/a.rs"), "{}", out);
        assert!(out.contains("-removed line"), "{}", out);
        assert!(out.contains("+added line"), "{}", out);
        assert!(out.contains("+hello"), "{}", out);
        assert!(out.contains("-bye"), "{}", out);
        assert!(out.contains("new"), "new-file marker missing: {}", out);
        assert!(out.contains("delet"), "deleted marker missing: {}", out);
        // the 4-line git header per file is pure decoration
        assert!(!out.contains("diff --git"), "{}", out);
        assert!(!out.contains("index 1111111"), "{}", out);
        assert!(!out.contains("+++ b/"), "{}", out);
    }

    #[test]
    fn diff_context_trimming_is_announced() {
        let mut raw = String::from("diff --git a/x.rs b/x.rs\nindex 1..2 100644\n--- a/x.rs\n+++ b/x.rs\n@@ -1,40 +1,40 @@\n");
        for i in 0..30 {
            raw.push_str(&format!(" context {}\n", i));
        }
        raw.push_str("-old\n+new\n");
        for i in 0..30 {
            raw.push_str(&format!(" tail context {}\n", i));
        }
        let out = filter_git(&["diff"], &raw);
        assert!(out.contains("-old") && out.contains("+new"), "{}", out);
        assert!(out.lines().count() < 40, "context not trimmed: {} lines", out.lines().count());
        assert!(has_truncation(&out), "trimming not announced: {}", out);
    }

    #[test]
    fn diff_stat_and_binary_and_rename() {
        let stat = filter_git(&["diff", "--stat", "HEAD~1"], " src/a.rs | 4 ++--\n src/b.rs | 2 +-\n 2 files changed, 3 insertions(+), 3 deletions(-)\n");
        assert!(stat.contains("src/a.rs"), "{}", stat);
        assert!(stat.contains("src/b.rs"), "{}", stat);
        assert!(!stat.contains("++--"), "{}", stat);
        let bin = filter_git(&["diff"], "diff --git a/img.png b/img.png\nindex 1..2 100644\nBinary files a/img.png and b/img.png differ\n");
        assert!(bin.contains("img.png"), "{}", bin);
        let ren = filter_git(&["diff"], "diff --git a/old.rs b/new.rs\nsimilarity index 95%\nrename from old.rs\nrename to new.rs\n");
        assert!(ren.contains("old.rs") && ren.contains("new.rs"), "{}", ren);
    }

    #[test]
    fn show_keeps_commit_header_then_diff() {
        let out = filter_git(
            &["show", "HEAD"],
            "commit abc1234000000000000000000000000000000000\nAuthor: A <a@b.c>\nDate:   Sat Sep 5 16:57:31 2026 +0530\n\n    the subject\n\ndiff --git a/x.rs b/x.rs\nindex 1..2 100644\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-a\n+b\n",
        );
        assert!(out.contains("abc1234"), "{}", out);
        assert!(out.contains("the subject"), "{}", out);
        assert!(out.contains("-a") && out.contains("+b"), "{}", out);
        assert!(!out.contains("diff --git"), "{}", out);
    }

    #[test]
    fn branch_groups_remotes_and_marks_current() {
        let out = filter_git(&["branch", "-a"], "* main\n  develop\n  remotes/origin/main\n  remotes/origin/develop\n  remotes/upstream/main\n");
        assert!(out.starts_with("* main"), "{}", out);
        assert!(out.contains("develop"), "{}", out);
        // remotes are still reachable, just factored
        assert!(out.contains("origin"), "{}", out);
        assert!(out.contains("upstream"), "{}", out);
        assert!(out.lines().count() <= 4, "not compacted: {}", out);
    }

    #[test]
    fn remote_merges_fetch_and_push_urls() {
        let out = filter_git(&["remote", "-v"], "origin\thttps://github.com/x/y.git (fetch)\norigin\thttps://github.com/x/y.git (push)\n");
        assert!(out.contains("origin"), "{}", out);
        assert!(out.contains("https://github.com/x/y.git"), "{}", out);
        assert_eq!(out.lines().count(), 1, "not merged: {}", out);
    }

    #[test]
    fn progress_lines_are_dropped_from_transfers() {
        let out = filter_git(
            &["push"],
            "Enumerating objects: 21, done.\nCounting objects: 100% (21/21), done.\nDelta compression using up to 12 threads\nCompressing objects: 100% (11/11), done.\nWriting objects: 100% (12/12), 1.61 KiB | 1.61 MiB/s, done.\nTotal 12 (delta 9), reused 0 (delta 0), pack-reused 0\nremote: Resolving deltas: 100% (9/9), completed with 9 local objects.\nTo https://github.com/x/y.git\n   5b641e3..abc1234  main -> main",
        );
        assert!(out.contains("5b641e3..abc1234"), "{}", out);
        assert!(out.contains("main -> main"), "{}", out);
        assert!(!out.contains("Compressing objects"), "{}", out);
        assert!(!out.contains("Delta compression"), "{}", out);
    }

    #[test]
    fn gh_and_glab_and_jj_are_handled() {
        let pr = filter_gh(&["pr", "list"], "12\tAdd feature\tfeat/x\tOPEN\n11\tFix bug\tfix/y\tMERGED\n");
        assert!(pr.contains("12") && pr.contains("Add feature"), "{}", pr);
        let api = filter_gh(&["api", "repos/x/y"], "{\"name\":\"y\",\"stargazers_count\":42,\"private\":false}");
        assert!(api.contains("name: y"), "{}", api);
        assert!(api.contains("42"), "{}", api);
        let mr = filter_glab(&["mr", "list"], "!5\tTitle\tbranch\topen\n");
        assert!(mr.contains("Title"), "{}", mr);
        let jj = filter_jj(&["log"], "@  qpvuntsm user@host 2026-09-09 12:00:00 abc1234\n│  subject line\n");
        assert!(jj.contains("subject line") || jj.contains("abc1234"), "{}", jj);
    }

    #[test]
    fn empty_and_unknown_subcommands_are_safe() {
        assert_eq!(filter_git(&["status"], ""), "clean"); // an empty status means a clean tree
        assert_eq!(filter_git(&["log"], ""), "");
        assert_eq!(filter_git(&["diff"], ""), "");
        assert_eq!(filter_git(&["branch"], ""), "");
        // unknown subcommand: compacted, never raw-dumped
        let bisect = filter_git(&["bisect", "log"], "\n\ngit bisect start\n\n\ngit bisect good abc\n");
        assert_eq!(bisect, "git bisect start\ngit bisect good abc");
    }

    #[test]
    fn no_panic_on_malformed_input() {
        for raw in [
            "commit\n",
            "diff --git\n",
            "@@ broken hunk header\n",
            "On branch\n",
            "\t\t\n",
            "  remotes/\n",
            "commit abc\nAuthor:\nDate:\n",
            "── unicode ── ✓ ✗ 🎉\n",
            "a\r\nb\r\n",
        ] {
            for sub in ["status", "log", "diff", "show", "branch", "remote", "stash"] {
                let _ = filter_git(&[sub], raw);
            }
        }
    }
}
