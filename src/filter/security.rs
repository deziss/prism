//! Security scanner filters: `semgrep`, `trivy`, `hadolint`.
//!
//! Findings are the payload: every rule id, CVE and location survives (or is counted).
//! Box borders, code excerpts and doc URLs are decoration and go away.

use super::common::*;
use serde_json::Value;

fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{} {}", n, noun)
    } else {
        format!("{} {}s", n, noun)
    }
}

fn is_border(t: &str) -> bool {
    !t.is_empty()
        && t.chars().all(|c| {
            matches!(
                c,
                '─' | '│'
                    | '┌'
                    | '┐'
                    | '└'
                    | '┘'
                    | '├'
                    | '┤'
                    | '┬'
                    | '┴'
                    | '┼'
                    | '━'
                    | '┏'
                    | '┓'
                    | '┗'
                    | '┛'
                    | '='
                    | '-'
                    | '+'
                    | ' '
            )
        })
}

/// Split a `│ a │ b │` row into cells.
fn box_cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches(|c| c == '│' || c == '|')
        .split(['│', '|'])
        .map(|c| c.trim().to_string())
        .collect()
}

// ─── semgrep ──────────────────────────────────────────────────────────────────

pub(crate) fn filter_semgrep(output: &str) -> String {
    let l = limits();
    // JSON mode
    if let Some(docs) = parse_json(output) {
        let mut findings: Vec<(String, usize, String, String, String)> = Vec::new();
        let mut scanned = 0usize;
        for d in &docs {
            if let Some(results) = d.get("results").and_then(|r| r.as_array()) {
                for r in results {
                    let path = r
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                        .to_string();
                    let line = r
                        .get("start")
                        .and_then(|s| s.get("line"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as usize;
                    let rule = r
                        .get("check_id")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                        .to_string();
                    let extra = r.get("extra");
                    let sev = extra
                        .and_then(|e| e.get("severity"))
                        .and_then(Value::as_str)
                        .unwrap_or("INFO")
                        .to_string();
                    let msg = extra
                        .and_then(|e| e.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    findings.push((path, line, sev, rule, squeeze_ws(&msg)));
                }
            }
            if let Some(paths) = d
                .get("paths")
                .and_then(|p| p.get("scanned"))
                .and_then(|s| s.as_array())
            {
                scanned = paths.len();
            }
        }
        return render_semgrep(findings, scanned, l);
    }

    // text mode
    let mut findings: Vec<(String, usize, String, String, String)> = Vec::new();
    let mut file = String::new();
    let mut rule = String::new();
    let mut scanned = 0usize;
    let mut summary: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() || is_border(t) || t.starts_with("Details:") {
            continue;
        }
        if let Some(rest) = t.strip_prefix("❯❱ ").or_else(|| t.strip_prefix("❯ ")) {
            rule = rest.trim().to_string();
            continue;
        }
        if t.contains("Ran ") && t.contains("rules on") {
            summary.push(squeeze_ws(t));
            scanned = t
                .split_whitespace()
                .nth(4)
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            continue;
        }
        // `  12┆ code` excerpt
        if t.chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
            && t.contains('┆')
        {
            continue;
        }
        // a bare path line starts a new file section
        if !t.starts_with(' ') && (t.contains('/') || t.contains('.')) && !t.contains(' ') {
            file = t.to_string();
            continue;
        }
        if !rule.is_empty() && !file.is_empty() {
            let sev = if rule.contains("security") {
                "WARNING"
            } else {
                "INFO"
            };
            findings.push((
                file.clone(),
                0,
                sev.to_string(),
                rule.clone(),
                squeeze_ws(t),
            ));
            rule.clear();
        }
    }
    let mut out = render_semgrep(findings, scanned, l);
    if !summary.is_empty() {
        out.push('\n');
        out.push_str(&summary.join("\n"));
    }
    out
}

fn render_semgrep(
    findings: Vec<(String, usize, String, String, String)>,
    scanned: usize,
    l: &Limits,
) -> String {
    if findings.is_empty() {
        return format!("semgrep: 0 findings ({} files scanned)", scanned);
    }
    let mut by_sev: Vec<(String, usize)> = Vec::new();
    for (_, _, sev, _, _) in &findings {
        match by_sev.iter_mut().find(|(s, _)| s == sev) {
            Some((_, n)) => *n += 1,
            None => by_sev.push((sev.clone(), 1)),
        }
    }
    let mut files: Vec<(String, Vec<String>)> = Vec::new();
    for (path, line, sev, rule, msg) in &findings {
        let entry = format!("  {}  {}  {}: {}", line, sev, rule, truncate(msg, 200));
        match files.iter_mut().find(|(p, _)| p == path) {
            Some((_, v)) => v.push(entry),
            None => files.push((path.clone(), vec![entry])),
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut shown = 0usize;
    let mut dropped = 0usize;
    for (path, entries) in &files {
        if shown >= l.max_diagnostics {
            dropped += entries.len();
            continue;
        }
        out.push(path.clone());
        for e in entries {
            if shown < l.max_diagnostics {
                out.push(e.clone());
                shown += 1;
            } else {
                dropped += 1;
            }
        }
    }
    if dropped > 0 {
        out.push(more(dropped, "findings"));
    }
    let sev_summary: Vec<String> = by_sev
        .iter()
        .map(|(s, n)| format!("{} {}", n, s.to_ascii_lowercase()))
        .collect();
    out.push(format!(
        "semgrep: {} ({}) in {}",
        plural(findings.len(), "finding"),
        sev_summary.join(", "),
        plural(files.len(), "file")
    ));
    out.join("\n")
}

// ─── trivy ────────────────────────────────────────────────────────────────────

fn sev_rank(s: &str) -> u8 {
    match s.to_ascii_uppercase().as_str() {
        "CRITICAL" => 0,
        "HIGH" => 1,
        "MEDIUM" => 2,
        "LOW" => 3,
        _ => 4,
    }
}

pub(crate) fn filter_trivy(output: &str) -> String {
    let l = limits();
    // JSON mode
    if let Some(docs) = parse_json(output) {
        let mut rows: Vec<(String, String, String, String, String, String)> = Vec::new();
        for d in &docs {
            if let Some(results) = d.get("Results").and_then(|r| r.as_array()) {
                for r in results {
                    let target = r
                        .get("Target")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                        .to_string();
                    if let Some(vulns) = r.get("Vulnerabilities").and_then(|v| v.as_array()) {
                        for v in vulns {
                            rows.push((
                                target.clone(),
                                v.get("Severity")
                                    .and_then(Value::as_str)
                                    .unwrap_or("UNKNOWN")
                                    .to_string(),
                                v.get("VulnerabilityID")
                                    .and_then(Value::as_str)
                                    .unwrap_or("?")
                                    .to_string(),
                                v.get("PkgName")
                                    .and_then(Value::as_str)
                                    .unwrap_or("?")
                                    .to_string(),
                                format!(
                                    "{}→{}",
                                    v.get("InstalledVersion")
                                        .and_then(Value::as_str)
                                        .unwrap_or("?"),
                                    v.get("FixedVersion")
                                        .and_then(Value::as_str)
                                        .unwrap_or("none")
                                ),
                                v.get("Title")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string(),
                            ));
                        }
                    }
                }
            }
        }
        return render_trivy(rows, l);
    }

    // table mode: `│ pkg │ CVE-… │ HIGH │ fixed │ 1.0 │ 1.1 │ title │`
    let mut rows: Vec<(String, String, String, String, String, String)> = Vec::new();
    let mut target = String::new();
    let mut totals: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() || is_border(tt) {
            continue;
        }
        if tt.starts_with("INFO") || tt.starts_with("DEBUG") {
            continue;
        }
        if tt.starts_with("WARN") || tt.starts_with("FATAL") {
            totals.push(squeeze_ws(tt));
            continue;
        }
        if let Some(rest) = tt.strip_prefix("Total: ") {
            totals.push(format!(
                "{}: Total {}",
                if target.is_empty() { "scan" } else { &target },
                rest
            ));
            continue;
        }
        if !tt.starts_with('│') && !tt.starts_with('|') {
            // `path (gobinary)` / `image (debian 12)` target header
            if tt.contains('(') && tt.ends_with(')') {
                target = tt.to_string();
            }
            continue;
        }
        let cells = box_cells(tt);
        if cells.len() < 4 {
            continue;
        }
        if cells.iter().any(|c| c.eq_ignore_ascii_case("Severity")) {
            continue; // header row
        }
        // continuation rows repeat empty leading cells — append their text to the title
        let sev = cells.iter().find(|c| sev_rank(c) < 4).cloned();
        match sev {
            Some(sev) => rows.push((
                target.clone(),
                sev,
                cells.get(1).cloned().unwrap_or_default(),
                cells.first().cloned().unwrap_or_default(),
                format!("{}→{}", cells.get(4).cloned().unwrap_or_default(), {
                    let f = cells.get(5).cloned().unwrap_or_default();
                    if f.is_empty() { "none".to_string() } else { f }
                }),
                cells.last().cloned().unwrap_or_default(),
            )),
            None => {
                if let Some(last) = rows.last_mut() {
                    let extra = cells.last().cloned().unwrap_or_default();
                    if !extra.is_empty() {
                        last.5.push(' ');
                        last.5.push_str(&extra);
                    }
                }
            }
        }
    }
    let mut out = render_trivy(rows, l);
    if !totals.is_empty() {
        out.push('\n');
        out.push_str(&totals.join("\n"));
    }
    out
}

fn render_trivy(
    mut rows: Vec<(String, String, String, String, String, String)>,
    l: &Limits,
) -> String {
    if rows.is_empty() {
        return "trivy: 0 vulnerabilities".to_string();
    }
    rows.sort_by_key(|r| sev_rank(&r.1));
    let total = rows.len();
    let mut counts = [0usize; 5];
    for r in &rows {
        counts[sev_rank(&r.1) as usize] += 1;
    }
    let mut out = vec![format!(
        "trivy: {} vulns ({} crit, {} high, {} med, {} low)",
        total, counts[0], counts[1], counts[2], counts[3]
    )];
    // the target only earns a per-row prefix when the scan covered several
    let multi_target = rows.iter().any(|r| r.0 != rows[0].0);
    if !multi_target && !rows[0].0.is_empty() {
        out.push(rows[0].0.clone());
    }
    let shown = total.min(l.max_diagnostics);
    for (target, sev, id, pkg, ver, title) in &rows[..shown] {
        let head = if multi_target && !target.is_empty() {
            format!("{} ", target)
        } else {
            String::new()
        };
        out.push(format!(
            "  {}{}  {}  {} {}: {}",
            head,
            sev,
            id,
            pkg,
            ver,
            truncate(&squeeze_ws(title), 160)
        ));
    }
    if total > shown {
        out.push(more(total - shown, "vulnerabilities"));
    }
    out.join("\n")
}

// ─── hadolint ─────────────────────────────────────────────────────────────────

pub(crate) fn filter_hadolint(output: &str) -> String {
    let l = limits();
    // JSON mode
    if let Some(docs) = parse_json(output) {
        let mut rows: Vec<(String, usize, String, String, String)> = Vec::new();
        for d in &docs {
            let arr = d.as_array().cloned().unwrap_or_default();
            for it in arr {
                rows.push((
                    it.get("file")
                        .and_then(Value::as_str)
                        .unwrap_or("Dockerfile")
                        .to_string(),
                    it.get("line").and_then(Value::as_u64).unwrap_or(0) as usize,
                    it.get("code")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                        .to_string(),
                    it.get("level")
                        .and_then(Value::as_str)
                        .unwrap_or("info")
                        .to_string(),
                    it.get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                ));
            }
        }
        return render_hadolint(rows, l);
    }
    let mut rows: Vec<(String, usize, String, String, String)> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        // `Dockerfile:12 DL3008 warning: Pin versions in apt get install`
        let mut it = t.splitn(2, ':');
        let file = it.next().unwrap_or("");
        let rest = it.next().unwrap_or("");
        let mut words = rest.split_whitespace();
        let line_no = words.next().and_then(|n| n.parse::<usize>().ok());
        let code = words.next().unwrap_or("");
        match (line_no, code.starts_with("DL") || code.starts_with("SC")) {
            (Some(n), true) => {
                let level = words
                    .next()
                    .unwrap_or("info")
                    .trim_end_matches(':')
                    .to_string();
                let msg: Vec<&str> = words.collect();
                rows.push((file.to_string(), n, code.to_string(), level, msg.join(" ")));
            }
            _ => other.push(squeeze_ws(t)),
        }
    }
    let mut out = render_hadolint(rows, l);
    if !other.is_empty() {
        out.push('\n');
        out.push_str(&cap_vec(other, 10, "lines").join("\n"));
    }
    out
}

fn render_hadolint(rows: Vec<(String, usize, String, String, String)>, l: &Limits) -> String {
    if rows.is_empty() {
        return "hadolint: ok".to_string();
    }
    let multi_file = rows.iter().any(|r| r.0 != rows[0].0);
    let mut codes: Vec<(String, usize)> = Vec::new();
    for r in &rows {
        match codes.iter_mut().find(|(c, _)| *c == r.2) {
            Some((_, n)) => *n += 1,
            None => codes.push((r.2.clone(), 1)),
        }
    }
    let mut out: Vec<String> = Vec::new();
    let shown = rows.len().min(l.max_diagnostics);
    for (file, line, code, level, msg) in &rows[..shown] {
        let head = if multi_file {
            format!("{}:", file)
        } else {
            String::new()
        };
        out.push(format!(
            "  {}{}  {}  {}  {}",
            head,
            line,
            code,
            level,
            truncate(&squeeze_ws(msg), 200)
        ));
    }
    if rows.len() > shown {
        out.push(more(rows.len() - shown, "issues"));
    }
    let code_summary: Vec<String> = codes.iter().map(|(c, n)| format!("{} ×{}", c, n)).collect();
    out.insert(
        0,
        format!(
            "hadolint: {} ({})",
            plural(rows.len(), "issue"),
            code_summary.join(", ")
        ),
    );
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semgrep_text_groups_findings_and_drops_decoration() {
        let raw = "\
┌─────────────┐
│ Code Findings │
└─────────────┘

    app/main.py
      ❯❱ python.lang.security.audit.eval-detected
         Detected the use of eval(). This can lead to RCE.
         Details: https://sg.run/abc

       12┆ eval(user_input)

    app/db.py
      ❯❱ python.sqlalchemy.security.sqli
         Possible SQL injection.

       44┆ query(raw)

Ran 812 rules on 24 files: 2 findings.";
        let out = filter_semgrep(raw);
        assert!(out.contains("app/main.py"), "{}", out);
        assert!(
            out.contains("python.lang.security.audit.eval-detected"),
            "{}",
            out
        );
        assert!(out.contains("Detected the use of eval()"), "{}", out);
        assert!(out.contains("app/db.py"), "{}", out);
        assert!(out.contains("python.sqlalchemy.security.sqli"), "{}", out);
        assert!(out.contains("semgrep: 2 findings"), "{}", out);
        assert!(!out.contains("┌"), "{}", out);
        assert!(!out.contains("Details: https"), "{}", out);
        assert!(!out.contains("eval(user_input)"), "{}", out);
    }

    #[test]
    fn semgrep_json_and_clean_scan() {
        let json = "{\"results\":[{\"path\":\"a.py\",\"start\":{\"line\":12},\"check_id\":\"rule.x\",\"extra\":{\"severity\":\"ERROR\",\"message\":\"bad thing\"}}],\"paths\":{\"scanned\":[\"a.py\",\"b.py\"]}}";
        let out = filter_semgrep(json);
        assert!(
            out.contains("a.py\n  12  ERROR  rule.x: bad thing"),
            "{}",
            out
        );
        assert!(
            out.contains("semgrep: 1 finding (1 error) in 1 file"),
            "{}",
            out
        );
        let clean = filter_semgrep("{\"results\":[],\"paths\":{\"scanned\":[\"a.py\"]}}");
        assert_eq!(clean, "semgrep: 0 findings (1 files scanned)");
    }

    #[test]
    fn semgrep_finding_cap_announced() {
        let mut findings = Vec::new();
        for i in 0..80 {
            findings.push(format!("{{\"path\":\"f{}.py\",\"start\":{{\"line\":1}},\"check_id\":\"r{}\",\"extra\":{{\"severity\":\"WARNING\",\"message\":\"m\"}}}}", i, i));
        }
        let json = format!("{{\"results\":[{}]}}", findings.join(","));
        let out = filter_semgrep(&json);
        assert!(has_truncation(&out), "{}", out);
        assert!(out.contains("semgrep: 80 findings"), "{}", out);
    }

    #[test]
    fn trivy_table_sorts_by_severity_and_joins_wrapped_cells() {
        let raw = "\
2026-09-09T10:00:00Z\tINFO\tVulnerability scanning is enabled
app/server (gobinary)
==================
Total: 3 (LOW: 1, MEDIUM: 0, HIGH: 1, CRITICAL: 1)

┌──────────┬────────────────┬──────────┬────────┬───────────────────┬───────────────┬─────────────────────┐
│ Library  │ Vulnerability  │ Severity │ Status │ Installed Version │ Fixed Version │        Title        │
├──────────┼────────────────┼──────────┼────────┼───────────────────┼───────────────┼─────────────────────┤
│ stdlib   │ CVE-2026-1111  │ CRITICAL │ fixed  │ 1.21.0            │ 1.21.5        │ crypto/tls: bad     │
│          │                │          │        │                   │               │ handshake handling  │
│ golang.x │ CVE-2026-2222  │ HIGH     │ fixed  │ 0.17.0            │ 0.18.0        │ net/http: overflow  │
│ zlib     │ CVE-2026-3333  │ LOW      │ fixed  │ 1.2.11            │ 1.2.13        │ minor issue         │
└──────────┴────────────────┴──────────┴────────┴───────────────────┴───────────────┴─────────────────────┘";
        let out = filter_trivy(raw);
        assert!(
            out.starts_with("trivy: 3 vulns (1 crit, 1 high, 0 med, 1 low)"),
            "{}",
            out
        );
        let crit = out.lines().nth(2).unwrap(); // line 1 is the scan target
        assert!(
            crit.contains("CRITICAL") && crit.contains("CVE-2026-1111"),
            "{}",
            out
        );
        assert!(crit.contains("stdlib 1.21.0→1.21.5"), "{}", out);
        assert!(
            crit.contains("handshake handling"),
            "wrapped cell lost: {}",
            out
        );
        // every CVE survives
        for cve in ["CVE-2026-1111", "CVE-2026-2222", "CVE-2026-3333"] {
            assert!(out.contains(cve), "missing {}\n{}", cve, out);
        }
        assert!(out.contains("Total 3"), "{}", out);
        assert!(!out.contains('┌'), "{}", out);
    }

    #[test]
    fn trivy_json_and_clean() {
        let json = "{\"Results\":[{\"Target\":\"img (alpine 3.19)\",\"Vulnerabilities\":[{\"VulnerabilityID\":\"CVE-1\",\"PkgName\":\"ssl\",\"Severity\":\"HIGH\",\"InstalledVersion\":\"1.0\",\"FixedVersion\":\"1.1\",\"Title\":\"boom\"}]}]}";
        let out = filter_trivy(json);
        assert!(out.contains("CVE-1"), "{}", out);
        assert!(out.contains("ssl 1.0→1.1: boom"), "{}", out);
        assert_eq!(filter_trivy("{\"Results\":[]}"), "trivy: 0 vulnerabilities");
    }

    #[test]
    fn hadolint_counts_codes_and_keeps_lines() {
        let out = filter_hadolint(
            "Dockerfile:3 DL3008 warning: Pin versions in apt get install\nDockerfile:9 DL3008 warning: Pin versions in apt get install\nDockerfile:12 SC2086 info: Double quote to prevent globbing",
        );
        assert!(
            out.starts_with("hadolint: 3 issues (DL3008 ×2, SC2086 ×1)"),
            "{}",
            out
        );
        assert!(
            out.contains("  3  DL3008  warning  Pin versions"),
            "{}",
            out
        );
        assert!(out.contains("  12  SC2086  info  Double quote"), "{}", out);
        assert_eq!(filter_hadolint(""), "hadolint: ok");
    }

    #[test]
    fn hadolint_json_and_multifile() {
        let out = filter_hadolint(
            "[{\"file\":\"a/Dockerfile\",\"line\":3,\"code\":\"DL3008\",\"level\":\"warning\",\"message\":\"Pin versions\"},{\"file\":\"b/Dockerfile\",\"line\":1,\"code\":\"DL3006\",\"level\":\"warning\",\"message\":\"Tag the version\"}]",
        );
        assert!(out.contains("a/Dockerfile:3"), "{}", out);
        assert!(out.contains("b/Dockerfile:1"), "{}", out);
    }

    #[test]
    fn empty_inputs_are_safe() {
        assert_eq!(filter_semgrep(""), "semgrep: 0 findings (0 files scanned)");
        assert_eq!(filter_trivy(""), "trivy: 0 vulnerabilities");
        assert_eq!(filter_hadolint(""), "hadolint: ok");
    }
}
