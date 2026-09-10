//! Search & filesystem filters: `grep`/`rg`, `find`/`fd`, `ls`/`eza`, `jq`, `tree`.
//!
//! These are the filters an agent hits most, so they compress *structure* rather than
//! deleting rows: matches and paths are grouped under one header per file/directory so
//! the shared prefix is paid for once, and every cut carries a [`more`] marker.

use super::common::*;

// ─── grep / rg ────────────────────────────────────────────────────────────────

/// Split `path:LINE:content` (or `path:LINE:COL:content`) on the first `:<digits>:`
/// so paths containing a colon still resolve. `sep` distinguishes matches (`:`) from
/// `-` context lines emitted by `-A/-B/-C`.
fn split_hit(line: &str, sep: char) -> Option<(&str, usize, &str)> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while let Some(off) = line[i..].find(sep) {
        let at = i + off;
        let rest = &line[at + 1..];
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 && rest.as_bytes().get(digits) == Some(&(sep as u8)) && at > 0 {
            let num: usize = rest[..digits].parse().ok()?;
            return Some((&line[..at], num, &rest[digits + 1..]));
        }
        i = at + 1;
        if i >= bytes.len() { break }
    }
    None
}

/// `path:count` from `grep -c`.
fn split_count(line: &str) -> Option<(&str, usize)> {
    let at = line.rfind(':')?;
    let n: usize = line[at + 1..].trim().parse().ok()?;
    if at == 0 { return None }
    Some((&line[..at], n))
}

fn tidy_match(content: &str, width: usize) -> String {
    truncate(&squeeze_ws(content), width)
}

/// Directory part of a path (`""` when the path has no separator).
fn dir_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => "",
    }
}

fn base_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// Group `path: a, b, c` lines for path-only output (`grep -l`, `fd`).
fn group_paths(paths: &[String], per_dir: usize, max_dirs: usize) -> String {
    let mut order: Vec<&str> = Vec::new();
    let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
    for p in paths {
        let d = dir_of(p);
        match groups.iter_mut().find(|(k, _)| *k == d) {
            Some((_, v)) => v.push(base_of(p)),
            None => {
                order.push(d);
                groups.push((d, vec![base_of(p)]));
            }
        }
    }
    let total_dirs = groups.len();
    let mut out: Vec<String> = Vec::new();
    let mut dropped_dirs = 0usize;
    let mut dropped_entries = 0usize;
    for (d, names) in groups {
        if out.len() >= max_dirs {
            dropped_dirs += 1;
            dropped_entries += names.len();
            continue;
        }
        let shown = names.len().min(per_dir);
        let head = if d.is_empty() { ".".to_string() } else { format!("{}/", d) };
        let mut line = format!("{} {}", head, names[..shown].join(" "));
        if names.len() > shown {
            line.push_str(&format!(" {}", more(names.len() - shown, "entries")));
        }
        out.push(line);
    }
    if dropped_dirs > 0 {
        out.push(format!(
            "[+{} more directories ({} entries)]",
            dropped_dirs, dropped_entries
        ));
    }
    let _ = total_dirs;
    out.join("\n")
}

pub(crate) fn filter_grep(args: &[&str], output: &str) -> String {
    let l = limits();
    if output.trim().is_empty() {
        return String::new();
    }
    let count_mode = has_flag(args, Some('c'), Some("--count"));
    let list_mode = has_flag(args, Some('l'), Some("--files-with-matches"))
        || has_flag(args, Some('L'), Some("--files-without-match"));

    let mut errors: Vec<&str> = Vec::new();
    let mut binaries: Vec<&str> = Vec::new();
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut plain: Vec<&str> = Vec::new();
    // per-file hits, insertion ordered: (path, [(line, content, is_context)])
    let mut files: Vec<(String, Vec<(usize, String, bool)>)> = Vec::new();
    let mut heading: Option<String> = None; // rg heading mode: bare path line

    let push_hit = |path: &str, num: usize, content: &str, ctx: bool,
                        files: &mut Vec<(String, Vec<(usize, String, bool)>)>| {
        let entry = (num, tidy_match(content, l.grep_line_width), ctx);
        match files.iter_mut().find(|(p, _)| p == path) {
            Some((_, v)) => v.push(entry),
            None => files.push((path.to_string(), vec![entry])),
        }
    };

    for line in output.lines() {
        let t = line.trim_end();
        if t.is_empty() || t == "--" {
            continue;
        }
        if let Some(rest) = t.strip_prefix("grep: ").or_else(|| t.strip_prefix("rg: ")) {
            errors.push(rest);
            continue;
        }
        if let Some(rest) = t.strip_prefix("Binary file ") {
            binaries.push(rest.trim_end_matches(" matches"));
            continue;
        }
        if count_mode {
            if let Some((p, n)) = split_count(t) {
                counts.push((p.to_string(), n));
                continue;
            }
        }
        if list_mode {
            plain.push(t);
            continue;
        }
        if let Some((p, n, c)) = split_hit(t, ':') {
            push_hit(p, n, c, false, &mut files);
            continue;
        }
        if let Some((p, n, c)) = split_hit(t, '-') {
            push_hit(p, n, c, true, &mut files);
            continue;
        }
        // rg heading mode: `LINE:content` under a preceding bare path line
        let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 && t.as_bytes().get(digits) == Some(&b':') {
            if let (Some(h), Ok(n)) = (heading.clone(), t[..digits].parse::<usize>()) {
                push_hit(&h, n, &t[digits + 1..], false, &mut files);
                continue;
            }
        }
        if !t.starts_with(' ') && !t.contains(' ') && (t.contains('/') || t.contains('.')) {
            heading = Some(t.to_string());
            continue;
        }
        plain.push(t);
    }

    let mut out: Vec<String> = Vec::new();

    if count_mode && !counts.is_empty() {
        let zero = counts.iter().filter(|(_, n)| *n == 0).count();
        let mut hits: Vec<(String, usize)> = counts.into_iter().filter(|(_, n)| *n > 0).collect();
        hits.sort_by(|a, b| b.1.cmp(&a.1));
        let total: usize = hits.iter().map(|(_, n)| n).sum();
        out.push(format!("{} matches in {} files", total, hits.len()));
        let shown = hits.len().min(l.grep_max_results);
        for (p, n) in &hits[..shown] {
            out.push(format!("{} {}", p, n));
        }
        if hits.len() > shown {
            out.push(more(hits.len() - shown, "files"));
        }
        if zero > 0 {
            out.push(format!("{} files with 0 matches", zero));
        }
        return out.join("\n");
    }

    if list_mode && !plain.is_empty() {
        let paths: Vec<String> = plain.iter().map(|s| s.to_string()).collect();
        out.push(format!("{} files", paths.len()));
        out.push(group_paths(&paths, l.find_max_per_dir, l.find_max_dirs));
        return out.join("\n");
    }

    if !files.is_empty() {
        let total: usize = files.iter().map(|(_, v)| v.iter().filter(|h| !h.2).count()).sum();
        let multi = files.len() > 1;
        if multi || total > 1 {
            out.push(format!("{} matches in {} files", total, files.len()));
        }
        let mut budget = l.grep_max_results;
        let mut skipped_matches = 0usize;
        let mut skipped_files = 0usize;
        for (path, hits) in &files {
            let matches = hits.iter().filter(|h| !h.2).count();
            if budget == 0 {
                skipped_matches += matches;
                skipped_files += 1;
                continue;
            }
            out.push(path.clone());
            let mut shown = 0usize;
            let mut dropped = 0usize;
            for (num, content, ctx) in hits {
                if *ctx {
                    if shown > 0 && shown < l.grep_max_per_file {
                        out.push(format!("  -{}: {}", num, content));
                    }
                    continue;
                }
                if shown < l.grep_max_per_file && budget > 0 {
                    out.push(format!("  {}: {}", num, content));
                    shown += 1;
                    budget -= 1;
                } else {
                    dropped += 1;
                }
            }
            if dropped > 0 {
                out.push(format!("  {}", more(dropped, "matches")));
            }
        }
        if skipped_matches > 0 {
            out.push(format!(
                "[+{} more matches in {} files]",
                skipped_matches, skipped_files
            ));
        }
    }

    if !binaries.is_empty() {
        out.push(format!(
            "{} binary files match: {}",
            binaries.len(),
            cap_lines(binaries.iter().copied(), 5, "files").replace('\n', ", ")
        ));
    }
    if !errors.is_empty() {
        let shown = errors.len().min(3);
        out.push(format!(
            "grep: {} unreadable paths: {}",
            errors.len(),
            errors[..shown].join("; ")
        ));
    }
    if out.is_empty() {
        return cap_lines(plain.iter().copied(), l.passthrough_max_lines, "lines");
    }
    if out.len() == 1 {
        return out.remove(0);
    }
    out.join("\n")
}

// ─── find / fd ────────────────────────────────────────────────────────────────

const NOISE_DIRS: [&str; 10] = [
    ".git", "node_modules", "target", "__pycache__", ".venv", "venv", "dist", ".next",
    ".turbo", ".cache",
];

/// Leading non-flag operands of `find` — its search roots.
fn find_roots<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut roots = Vec::new();
    for a in args {
        if a.starts_with('-') || a.starts_with('(') || a.starts_with('!') {
            break;
        }
        roots.push(*a);
    }
    roots
}

/// True when a search root already points inside `name`, in which case collapsing
/// that directory would hide exactly what was asked for.
fn root_targets(roots: &[&str], name: &str) -> bool {
    roots.iter().any(|r| r.split('/').any(|c| c == name))
}

pub(crate) fn filter_find(args: &[&str], output: &str) -> String {
    let l = limits();
    if output.trim().is_empty() {
        return String::new();
    }
    let roots = find_roots(args);
    let mut paths: Vec<&str> = Vec::new();
    let mut errors: Vec<&str> = Vec::new();
    for line in output.lines() {
        let t = line.trim_end();
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix("find: ").or_else(|| t.strip_prefix("fd: ")) {
            errors.push(rest);
            continue;
        }
        paths.push(t);
    }
    if paths.len() == 1 && errors.is_empty() {
        return paths[0].to_string();
    }

    // Collapse each noise subtree to one announced line, unless it was asked for.
    let mut collapsed: Vec<(String, usize)> = Vec::new();
    let mut kept: Vec<&str> = Vec::new();
    'outer: for p in &paths {
        let trimmed = p.strip_prefix("./").unwrap_or(p);
        let mut prefix_end = 0usize;
        for comp in trimmed.split('/') {
            let end = prefix_end + comp.len();
            if NOISE_DIRS.contains(&comp) && !root_targets(&roots, comp) {
                let prefix = &trimmed[..end];
                match collapsed.iter_mut().find(|(k, _)| k == prefix) {
                    Some((_, n)) => *n += 1,
                    None => collapsed.push((prefix.to_string(), 1)),
                }
                continue 'outer;
            }
            prefix_end = end + 1; // skip '/'
        }
        kept.push(trimmed);
    }

    let mut out: Vec<String> = Vec::new();
    // Group kept paths by parent directory.
    let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
    for p in &kept {
        let d = dir_of(p);
        match groups.iter_mut().find(|(k, _)| *k == d) {
            Some((_, v)) => v.push(base_of(p)),
            None => groups.push((d, vec![base_of(p)])),
        }
    }
    // A name is a directory when some other result lives under it.
    let is_dir = |dir: &str, name: &str| -> bool {
        let full = if dir.is_empty() { name.to_string() } else { format!("{}/{}", dir, name) };
        let probe = format!("{}/", full);
        kept.iter().any(|k| k.starts_with(&probe)) || collapsed.iter().any(|(c, _)| c.starts_with(&probe))
    };

    if groups.len() > 1 || !collapsed.is_empty() {
        out.push(format!(
            "{} paths in {} dirs",
            paths.len(),
            groups.len() + collapsed.len()
        ));
    }
    let mut dropped_dirs = 0usize;
    let mut dropped_entries = 0usize;
    let mut shown_dirs = 0usize;
    // Directory paths share long prefixes, and repeating `a/b/c/` on every line is the
    // single biggest cost in a deep listing. Sort them so parents precede children,
    // then print each one relative to the previous line's directory, indented by depth.
    groups.sort_by(|a, b| a.0.cmp(b.0));
    let mut ancestors: Vec<&str> = Vec::new();
    for (dir, names) in &groups {
        if shown_dirs >= l.find_max_dirs {
            dropped_dirs += 1;
            dropped_entries += names.len();
            continue;
        }
        shown_dirs += 1;
        let shown = names.len().min(l.find_max_per_dir);
        let entries: Vec<String> = names[..shown]
            .iter()
            .map(|n| {
                if is_dir(dir, n) {
                    format!("{}/", n)
                } else {
                    n.to_string()
                }
            })
            .collect();
        // drop ancestors this directory does not descend from, then print the suffix
        // relative to the deepest one that is still open
        while ancestors
            .last()
            .map(|a| !(dir.len() > a.len() && dir.starts_with(*a) && dir.as_bytes()[a.len()] == b'/'))
            .unwrap_or(false)
        {
            ancestors.pop();
        }
        let head = if dir.is_empty() {
            ".".to_string()
        } else {
            match ancestors.last() {
                Some(a) => format!("{}{}/", " ".repeat(ancestors.len()), &dir[a.len() + 1..]),
                None => format!("{}/", dir),
            }
        };
        // `dir/ a.rs b.rs` — space separated, since ", " costs an extra token per entry
        let mut line = format!("{} {}", head, entries.join(" "));
        if names.len() > shown {
            line.push_str(&format!(" {}", more(names.len() - shown, "entries")));
        }
        out.push(line);
        ancestors.push(dir);
    }
    for (prefix, n) in &collapsed {
        out.push(format!("{}/ ({} entries, collapsed)", prefix, n));
    }
    if dropped_dirs > 0 {
        out.push(format!(
            "[+{} more directories ({} entries)]",
            dropped_dirs, dropped_entries
        ));
    }
    if !errors.is_empty() {
        let shown = errors.len().min(3);
        out.push(format!(
            "find: {} unreadable paths: {}",
            errors.len(),
            errors[..shown].join("; ")
        ));
    }
    out.join("\n")
}

// ─── ls / eza ─────────────────────────────────────────────────────────────────

/// `drwxr-xr-x` → `755`. Returns `None` when the field isn't a permission string.
fn octal_perms(field: &str) -> Option<String> {
    let f = field.trim_end_matches(['+', '.', '@']);
    let b: Vec<char> = f.chars().collect();
    if b.len() < 10 || !matches!(b[0], '-' | 'd' | 'l' | 'b' | 'c' | 'p' | 's') {
        return None;
    }
    let mut digits = String::new();
    for chunk in 0..3 {
        let base = 1 + chunk * 3;
        let mut v = 0;
        if b[base] == 'r' { v += 4 }
        if b[base + 1] == 'w' { v += 2 }
        if matches!(b[base + 2], 'x' | 's' | 't') { v += 1 }
        digits.push_str(&v.to_string());
    }
    Some(digits)
}

/// First `n` whitespace-separated fields plus the untouched remainder (so names
/// containing spaces survive).
fn split_fields(line: &str, n: usize) -> Option<(Vec<&str>, &str)> {
    let mut fields = Vec::with_capacity(n);
    let mut rest = line;
    for _ in 0..n {
        let start = rest.find(|c: char| !c.is_whitespace())?;
        rest = &rest[start..];
        let end = rest.find(char::is_whitespace)?;
        fields.push(&rest[..end]);
        rest = &rest[end..];
    }
    let start = rest.find(|c: char| !c.is_whitespace())?;
    Some((fields, &rest[start..]))
}

/// One long-format `ls` row → `755  name/  12.3K`.
/// `Some("")` means the row parsed but is dropped by design (`.`, `..`); `None` means
/// the row wasn't a long-format entry at all, so the caller keeps it verbatim.
fn ls_long_entry(line: &str) -> Option<String> {
    let (fields, name) = split_fields(line, 8)?;
    let perms = octal_perms(fields[0])?;
    let kind = fields[0].chars().next().unwrap_or('-');
    let (name, link) = match name.split_once(" -> ") {
        Some((n, t)) => (n, Some(t)),
        None => (name, None),
    };
    if name == "." || name == ".." {
        return Some(String::new());
    }
    let mut out = format!("{}  {}", perms, name);
    if kind == 'd' {
        out.push('/');
    }
    if let Some(t) = link {
        out.push_str(&format!(" -> {}", t));
    } else if kind != 'd' {
        // fields[4] is the size column for both `-l` and `-lh`
        if let Some(bytes) = parse_size(fields[4]) {
            out.push_str(&format!("  {}", human_size(bytes)));
        }
    }
    Some(out)
}

pub(crate) fn filter_ls(args: &[&str], output: &str) -> String {
    let l = limits();
    if output.trim().is_empty() {
        return String::new();
    }
    let long = has_flag(args, Some('l'), Some("--long"))
        || has_flag(args, Some('o'), None)
        || output.starts_with("total ")
        || output.lines().next().map(|f| f.starts_with("total ")).unwrap_or(false);

    let mut out: Vec<String> = Vec::new();
    let mut errors: Vec<&str> = Vec::new();
    let mut entries: Vec<String> = Vec::new(); // current section
    let mut plain: Vec<&str> = Vec::new();
    let mut dropped = 0usize;

    let flush = |entries: &mut Vec<String>, dropped: &mut usize, out: &mut Vec<String>| {
        if entries.is_empty() {
            return;
        }
        out.append(entries);
        if *dropped > 0 {
            out.push(more(*dropped, "entries"));
            *dropped = 0;
        }
    };

    for line in output.lines() {
        let t = line.trim_end();
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix("ls: ") {
            errors.push(rest);
            continue;
        }
        if t.starts_with("total ") {
            continue;
        }
        // `ls -R` section header
        if t.ends_with(':') && !t.contains(char::is_whitespace) {
            flush(&mut entries, &mut dropped, &mut out);
            out.push(format!("{}/:", t.trim_end_matches(':')));
            continue;
        }
        if long {
            if let Some(e) = ls_long_entry(t) {
                if e.is_empty() {
                    continue; // `.` / `..`
                }
                if entries.len() < l.ls_max_entries {
                    entries.push(e);
                } else {
                    dropped += 1;
                }
                continue;
            }
        }
        plain.push(t);
    }
    flush(&mut entries, &mut dropped, &mut out);

    if !plain.is_empty() {
        if long {
            // rows the parser could not read — keep them verbatim rather than drop
            out.extend(cap_lines(plain.iter().copied(), l.ls_max_entries, "lines").lines().map(String::from));
        } else {
            // piped `ls` prints one name per line; pack them into ~100-char rows
            let names: Vec<&str> = plain
                .iter()
                .copied()
                .filter(|n| *n != "." && *n != "..")
                .collect();
            let shown = names.len().min(l.ls_max_entries);
            let mut row = String::new();
            for n in &names[..shown] {
                if !row.is_empty() && row.len() + n.len() + 2 > 100 {
                    out.push(std::mem::take(&mut row));
                }
                if !row.is_empty() {
                    row.push_str("  ");
                }
                row.push_str(n);
            }
            if !row.is_empty() {
                out.push(row);
            }
            if names.len() > shown {
                out.push(more(names.len() - shown, "entries"));
            }
        }
    }
    if !errors.is_empty() {
        let shown = errors.len().min(3);
        out.push(format!("ls: {} errors: {}", errors.len(), errors[..shown].join("; ")));
    }
    out.join("\n")
}

// ─── jq ───────────────────────────────────────────────────────────────────────

pub(crate) fn filter_jq(output: &str) -> String {
    compact_json_output(output, |o| {
        // `jq -r` emits raw strings, not JSON
        cap_lines(collapse_blank(o).lines(), limits().list_max_lines, "lines")
    })
}

// ─── tree ─────────────────────────────────────────────────────────────────────

pub(crate) fn filter_tree(output: &str) -> String {
    let l = limits();
    let mut out: Vec<String> = Vec::new();
    let mut trailer: Option<String> = None;
    let mut dropped = 0usize;
    for line in output.lines() {
        let t = line.trim_end();
        if t.is_empty() {
            continue;
        }
        if t.contains(" director") && (t.contains(" file") || t.ends_with("directories")) {
            trailer = Some(squeeze_ws(t));
            continue;
        }
        if out.len() < l.list_max_lines {
            out.push(flatten_tree(t));
        } else {
            dropped += 1;
        }
    }
    if dropped > 0 {
        out.push(more(dropped, "lines"));
    }
    if let Some(t) = trailer {
        out.push(t);
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const GREP: &str = "\
src/vector.rs:20:    pub fn new() -> Self {
src/vector.rs:30:    pub fn add(&mut self, id: &str, embedding: &[f32]) {
src/main.rs:14:pub fn main() {
Binary file target/debug/prism matches
grep: /root/secret: Permission denied";

    #[test]
    fn grep_groups_by_file_and_keeps_every_match() {
        let out = filter_grep(&["-rn", "pub fn", "src/"], GREP);
        assert!(out.starts_with("3 matches in 2 files"), "{}", out);
        assert!(out.contains("src/vector.rs\n  20: pub fn new() -> Self {"), "{}", out);
        assert!(out.contains("  30: pub fn add(&mut self, id: &str, embedding: &[f32]) {"));
        assert!(out.contains("src/main.rs\n  14: pub fn main() {"));
        assert!(out.contains("1 binary files match: target/debug/prism"));
        assert!(out.contains("grep: 1 unreadable paths: /root/secret: Permission denied"));
    }

    #[test]
    fn grep_fidelity_every_hit_present_or_counted() {
        let mut fixture = String::new();
        for i in 1..=60 {
            fixture.push_str(&format!("src/a.rs:{}:hit {}\n", i, i));
        }
        let out = filter_grep(&["-rn", "hit"], &fixture);
        let shown = out.lines().filter(|l| l.starts_with("  ") && l.contains(':')).count();
        let announced: usize = out
            .lines()
            .find_map(|l| l.trim().strip_prefix("[+")?.split_whitespace().next()?.parse().ok())
            .unwrap_or(0);
        assert_eq!(shown + announced, 60, "{}", out);
        assert!(has_truncation(&out));
    }

    #[test]
    fn grep_paths_with_colons_and_context_lines() {
        let out = filter_grep(
            &["-C", "1", "x"],
            "a:b.rs:12:match here\na:b.rs-11-context above\n--\nc.rs:3:other",
        );
        assert!(out.contains("a:b.rs\n  12: match here"), "{}", out);
        assert!(out.contains("  -11: context above"), "{}", out);
        assert!(!out.contains("--\n"));
    }

    #[test]
    fn grep_rg_heading_mode_and_count_mode() {
        let head = filter_grep(&["-n", "tokio"], "src/proxy.rs\n12:use tokio;\n14:tokio::spawn();");
        assert!(head.contains("src/proxy.rs\n  12: use tokio;"), "{}", head);
        let cnt = filter_grep(&["-c", "fn"], "src/a.rs:12\nsrc/b.rs:0\nsrc/c.rs:3");
        assert!(cnt.starts_with("15 matches in 2 files"), "{}", cnt);
        assert!(cnt.contains("src/a.rs 12\nsrc/c.rs 3"), "{}", cnt);
        assert!(cnt.contains("1 files with 0 matches"));
    }

    #[test]
    fn grep_list_mode_groups_by_dir() {
        let out = filter_grep(&["-rl", "fn"], "src/a.rs\nsrc/b.rs\nREADME.md");
        assert!(out.starts_with("3 files"), "{}", out);
        assert!(out.contains("src/ a.rs b.rs"), "{}", out);
        assert!(out.contains("README.md"));
    }

    #[test]
    fn grep_empty_and_single_line() {
        assert_eq!(filter_grep(&[], ""), "");
        assert_eq!(filter_grep(&[], "src/a.rs:1:only"), "src/a.rs\n  1: only");
    }

    #[test]
    fn find_groups_and_collapses_noise_only_when_not_requested() {
        let out = filter_find(
            &[".", "-name", "*.rs"],
            "./build.rs\n./src/main.rs\n./src/filter/mod.rs\n./target/debug/x.rs\n./target/debug/y.rs",
        );
        assert!(out.contains("5 paths in"), "{}", out);
        assert!(out.contains("src/ main.rs"), "{}", out);
        // a child directory prints its suffix only, indented by depth
        assert!(out.contains("\n filter/ mod.rs"), "{}", out);
        assert!(out.contains("target/ (2 entries, collapsed)"), "{}", out);

        // asking for target/ explicitly must list it
        let inside = filter_find(
            &["target/debug", "-maxdepth", "1", "-type", "f"],
            "target/debug/prism\ntarget/debug/libprism.rlib",
        );
        assert!(inside.contains("prism"), "{}", inside);
        assert!(inside.contains("libprism.rlib"));
        assert!(!inside.contains("collapsed"));
    }

    #[test]
    fn find_fidelity_and_dir_markers() {
        let mut fixture = String::new();
        for i in 0..100 {
            fixture.push_str(&format!("src/f{}.rs\n", i));
        }
        let out = filter_find(&["src"], &fixture);
        let announced: usize = out
            .lines()
            .find_map(|l| l.split("[+").nth(1)?.split_whitespace().next()?.parse().ok())
            .unwrap_or(0);
        let shown = out.lines().last().unwrap().split_whitespace().count() - 1;
        assert!(shown + announced >= 99, "{} / {}\n{}", shown, announced, out);
        assert!(has_truncation(&out));
        // a path that is also a prefix of others is marked as a directory
        let dirs = filter_find(&["."], "prism-hub\nprism-hub/frontend\nprism-hub/frontend/src");
        assert!(dirs.contains("prism-hub/"), "{}", dirs);
    }

    #[test]
    fn find_factors_shared_directory_prefixes() {
        // A deep listing must not repeat `a/b/c/` on every line.
        let out = filter_find(
            &["hub"],
            "hub/frontend\nhub/frontend/src\nhub/frontend/src/app\nhub/backend\nhub/backend/src",
        );
        assert!(out.contains("hub/"), "{}", out);
        assert!(out.contains("\n frontend/"), "{}", out);
        assert!(out.contains("\n  src/"), "{}", out);
        // the full nested path appears once, not once per level
        assert_eq!(out.matches("hub/frontend/src").count(), 0, "{}", out);
    }

    #[test]
    fn filter_output_never_costs_more_than_the_raw_output() {
        use super::super::filter_output;
        // grouping/headers can outweigh a tiny output — the raw text wins then
        for (cmd, args, raw) in [
            ("git", vec!["branch".to_string(), "-a".to_string()], "* main\n  remotes/origin/main\n"),
            ("find", vec![".".to_string()], "./a.toml\n./b.toml\n"),
            ("jq", vec![".".to_string()], "{\"a\":1,\"b\":2}"),
            ("ls", vec![], "a\nb\n"),
        ] {
            let out = filter_output(raw, cmd, &args);
            assert!(
                out.len() <= raw.len(),
                "{} inflated {} -> {} bytes:\n{}",
                cmd,
                raw.len(),
                out.len(),
                out
            );
        }
    }

    #[test]
    fn find_single_path_errors_and_empty() {
        assert_eq!(filter_find(&["."], "./only/one"), "./only/one");
        assert_eq!(filter_find(&["."], ""), "");
        let err = filter_find(&["/"], "/a\n/b\nfind: '/root': Permission denied");
        assert!(err.contains("find: 1 unreadable paths: '/root': Permission denied"), "{}", err);
    }

    const LS: &str = "\
total 312
drwxrwxr-x  4 anshukushwaha anshukushwaha  4096 Sep  8 18:44 .
drwxrwxr-x 11 anshukushwaha anshukushwaha  4096 Sep  9 08:17 ..
-rw-------  1 anshukushwaha anshukushwaha 18759 Sep  8 12:03 cli.rs
drwxrwxr-x  2 anshukushwaha anshukushwaha  4096 Jun 15 22:17 knowledge
lrwxrwxrwx  1 root          root              7 Apr  2  2025 python -> python3
-rw-rw-r--  1 anshukushwaha anshukushwaha   958 Sep  1 10:00 my file.rs";

    #[test]
    fn ls_long_keeps_perms_size_and_symlink_target() {
        let out = filter_ls(&["-la", "src"], LS);
        assert!(out.contains("600  cli.rs  18.3K"), "{}", out);
        assert!(out.contains("775  knowledge/"), "{}", out);
        assert!(out.contains("777  python -> python3"), "{}", out);
        assert!(out.contains("664  my file.rs  958B"), "{}", out);
        assert!(!out.contains("total "));
        assert!(!out.lines().any(|l| l.ends_with("  .") || l.ends_with("  ..")));
        assert!(!out.contains("anshukushwaha"));
    }

    #[test]
    fn ls_fidelity_and_recursive_sections() {
        let mut fixture = String::from("total 8\n");
        for i in 0..250 {
            fixture.push_str(&format!("-rw-r--r-- 1 u g 10 Sep 1 10:00 f{}.rs\n", i));
        }
        let out = filter_ls(&["-l"], &fixture);
        let shown = out.lines().filter(|l| l.starts_with("644  ")).count();
        let announced: usize = out
            .lines()
            .find_map(|l| l.strip_prefix("[+")?.split_whitespace().next()?.parse().ok())
            .unwrap_or(0);
        assert_eq!(shown + announced, 250, "{}", out);

        let rec = filter_ls(&["-laR", "src"], "src:\ntotal 4\n-rw-r--r-- 1 u g 10 Sep 1 10:00 a.rs\n\nsrc/knowledge:\ntotal 4\n-rw-r--r-- 1 u g 20 Sep 1 10:00 b.rs");
        assert!(rec.contains("src/:"), "{}", rec);
        assert!(rec.contains("src/knowledge/:"), "{}", rec);
        assert!(rec.contains("644  a.rs  10B") && rec.contains("644  b.rs  20B"));
    }

    #[test]
    fn ls_short_form_and_errors() {
        let out = filter_ls(&[], "a.rs\nb.rs\nc.rs");
        assert_eq!(out, "a.rs  b.rs  c.rs");
        assert_eq!(filter_ls(&["-la"], ""), "");
        let err = filter_ls(&["-la"], "ls: cannot access 'x': No such file or directory");
        assert!(err.contains("ls: 1 errors: cannot access 'x'"), "{}", err);
    }

    #[test]
    fn jq_compacts_json_and_passes_raw_output() {
        let out = filter_jq("{\n  \"name\": \"prism\",\n  \"deps\": {\n    \"serde\": \"^1\"\n  }\n}");
        assert!(out.contains("name: prism"), "{}", out);
        assert!(out.contains("deps:\n serde: ^1"), "{}", out);
        let raw = filter_jq("git status\ncargo check\n");
        assert_eq!(raw, "git status\ncargo check");
        assert_eq!(filter_jq(""), "");
    }

    #[test]
    fn tree_flattens_box_drawing_and_keeps_trailer() {
        let out = filter_tree("src\n├── filter\n│   ├── mod.rs\n│   └── common.rs\n└── main.rs\n\n2 directories, 3 files");
        assert!(out.contains("\n filter"), "{}", out);
        assert!(out.contains("\n  mod.rs"), "{}", out);
        assert!(out.ends_with("2 directories, 3 files"), "{}", out);
        assert_eq!(filter_tree(""), "");
    }
}
