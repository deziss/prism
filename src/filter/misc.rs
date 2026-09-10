//! Network & system filters: `curl`, `wget`, `ping`, `ps`, `ss`, `df`, `du`, `free`,
//! `systemctl`, `journalctl`, `env`, `man`, plus the `act` and generic `lint` wrappers.
//!
//! `env`/`printenv` redact secret-looking values — see [`SECRET_KEYS`].

use super::common::*;

fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{} {}", n, noun)
    } else {
        format!("{} {}s", n, noun)
    }
}

/// Mask digit runs so otherwise-identical log lines group together.
fn log_template(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_num = false;
    for c in line.chars() {
        if c.is_ascii_digit() {
            if !in_num {
                out.push('#');
                in_num = true;
            }
        } else {
            in_num = false;
            out.push(c);
        }
    }
    out
}

fn is_alert(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    [
        "error", "fail", "fatal", "panic", "warn", "denied", "refused", "timeout",
    ]
    .iter()
    .any(|k| l.contains(k))
}

/// Fold repeated templates, then keep the last `tail` lines with alerts pinned.
fn fold_lines(lines: Vec<String>, tail: usize) -> Vec<String> {
    let mut groups: Vec<(String, String, usize)> = Vec::new();
    for line in lines {
        let key = log_template(&line);
        match groups.last_mut() {
            Some((k, _, n)) if *k == key => *n += 1,
            _ => groups.push((key, line, 1)),
        }
    }
    let rendered: Vec<String> = groups
        .into_iter()
        .map(|(_, first, n)| {
            if n > 1 {
                format!("{}  (×{})", first, n)
            } else {
                first
            }
        })
        .collect();
    if rendered.len() <= tail {
        return rendered;
    }
    let cut = rendered.len() - tail;
    let mut out: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    for (i, line) in rendered.iter().enumerate() {
        if i < cut && !is_alert(line) {
            skipped += 1;
            continue;
        }
        out.push(line.clone());
    }
    if skipped > 0 {
        out.insert(0, more(skipped, "lines"));
    }
    out
}

// ─── curl / wget / ping ───────────────────────────────────────────────────────

const KEEP_HEADERS: [&str; 10] = [
    "content-type",
    "content-length",
    "location",
    "cache-control",
    "etag",
    "retry-after",
    "www-authenticate",
    "content-encoding",
    "transfer-encoding",
    "x-request-id",
];

fn interesting_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    KEEP_HEADERS.contains(&n.as_str())
        || n.starts_with("x-ratelimit")
        || n.starts_with("set-cookie")
}

/// Reduce an HTML body to its title and visible text.
fn html_digest(body: &str, max_lines: usize) -> String {
    let mut title = String::new();
    if let Some(start) = body.to_ascii_lowercase().find("<title>") {
        if let Some(end) = body[start..].to_ascii_lowercase().find("</title>") {
            title = body[start + 7..start + end].trim().to_string();
        }
    }
    let mut text = String::new();
    let mut in_tag = false;
    let mut skip_block = false;
    let lower = body.to_ascii_lowercase();
    let mut i = 0;
    let bytes: Vec<char> = body.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();
    while i < bytes.len() {
        let c = bytes[i];
        if c == '<' {
            let rest: String = lower_chars[i..(i + 8).min(lower_chars.len())]
                .iter()
                .collect();
            if rest.starts_with("<script") || rest.starts_with("<style") {
                skip_block = true;
            }
            if rest.starts_with("</script") || rest.starts_with("</style") {
                skip_block = false;
            }
            in_tag = true;
            i += 1;
            continue;
        }
        if c == '>' {
            in_tag = false;
            text.push(' ');
            i += 1;
            continue;
        }
        if !in_tag && !skip_block {
            text.push(c);
        }
        i += 1;
    }
    let mut out = Vec::new();
    if !title.is_empty() {
        out.push(format!("title: {}", title));
    }
    let body_text = squeeze_ws(&text);
    if !body_text.is_empty() {
        out.push(truncate(&body_text, max_lines * 100));
    }
    out.push(format!("html: {} bytes", body.len()));
    out.join("\n")
}

pub(crate) fn filter_curl(_args: &[&str], output: &str) -> String {
    let l = limits();
    let trimmed = output.trim_start();
    // JSON body
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return compact_json_output(output, generic);
    }
    let mut out: Vec<String> = Vec::new();
    let mut verbose_dropped = 0usize;
    let mut body: Vec<&str> = Vec::new();
    for line in output.lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if let Some(rest) = tt.strip_prefix("* ") {
            // verbose connection chatter, except TLS/DNS failures
            if is_alert(rest) {
                out.push(format!("* {}", rest));
            } else {
                verbose_dropped += 1;
            }
            continue;
        }
        if let Some(rest) = tt.strip_prefix("> ").or_else(|| tt.strip_prefix("< ")) {
            let is_status = rest.starts_with("HTTP/") || rest.contains(" HTTP/");
            if is_status {
                out.push(rest.to_string());
                continue;
            }
            match rest.split_once(':') {
                Some((name, val)) if interesting_header(name) => {
                    let val = if name.to_ascii_lowercase().starts_with("set-cookie") {
                        val.split('=').next().unwrap_or("").trim().to_string() + "=…"
                    } else {
                        val.trim().to_string()
                    };
                    out.push(format!("{}: {}", name, val));
                }
                _ => verbose_dropped += 1,
            }
            continue;
        }
        if tt.starts_with("HTTP/") {
            out.push(tt.to_string());
            continue;
        }
        if tt.starts_with("curl:") {
            out.push(tt.to_string());
            continue;
        }
        // response headers from -i / -I
        if let Some((name, val)) = tt.split_once(": ") {
            if !name.contains(' ')
                && name
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_alphabetic())
                    .unwrap_or(false)
            {
                if interesting_header(name) {
                    out.push(format!("{}: {}", name, val.trim()));
                } else {
                    verbose_dropped += 1;
                }
                continue;
            }
        }
        // progress meter rows
        if tt.starts_with('%') || tt.split_whitespace().count() > 8 && tt.contains("--:--:--") {
            verbose_dropped += 1;
            continue;
        }
        body.push(tt);
    }
    let body_text = body.join("\n");
    if body_text.trim_start().starts_with('<') {
        out.push(html_digest(&body_text, 5));
    } else if !body_text.is_empty() {
        out.push(cap_lines(body, l.passthrough_max_lines, "lines"));
    }
    if verbose_dropped > 0 {
        out.push(format!(
            "({} header/progress lines dropped)",
            verbose_dropped
        ));
    }
    out.join("\n")
}

pub(crate) fn filter_wget(output: &str) -> String {
    let l = limits();
    let mut status: Option<String> = None;
    let mut saved: Option<String> = None;
    let mut size: Option<String> = None;
    let mut errors: Vec<String> = Vec::new();
    let mut body: Vec<&str> = Vec::new();
    let mut noise = 0usize;
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with("--20")
            || t.starts_with("Resolving ")
            || t.starts_with("Connecting to ")
            || t.starts_with("Reusing existing connection")
            || t.contains("Loaded CA certificate")
        {
            noise += 1;
            continue;
        }
        if let Some(rest) = t.strip_prefix("HTTP request sent, awaiting response... ") {
            status = Some(rest.to_string());
            continue;
        }
        if let Some(rest) = t.strip_prefix("Length: ") {
            size = Some(rest.split('(').next().unwrap_or(rest).trim().to_string());
            continue;
        }
        if let Some(rest) = t.strip_prefix("Saving to: ") {
            saved = Some(rest.trim_matches('‘').trim_matches('’').trim().to_string());
            continue;
        }
        if t.contains("ERROR ") || t.starts_with("wget:") || t.contains("failed:") {
            errors.push(squeeze_ws(t));
            continue;
        }
        if t.contains('%') && (t.contains("K/s") || t.contains("M/s") || t.contains("=")) {
            noise += 1;
            continue;
        }
        if t.starts_with("Remote file exists") || t.contains("saved [") {
            saved = saved.or_else(|| Some(squeeze_ws(t)));
            continue;
        }
        body.push(t);
    }
    let mut out: Vec<String> = Vec::new();
    if !errors.is_empty() {
        out.extend(cap_vec(errors, l.max_diagnostics, "errors"));
    }
    if saved.is_some() || status.is_some() {
        let mut line = "wget:".to_string();
        if let Some(s) = &saved {
            line.push_str(&format!(" {}", s));
        }
        if let Some(sz) = &size {
            line.push_str(&format!(" ({})", sz));
        }
        if let Some(st) = &status {
            line.push_str(&format!(" [{}]", st));
        }
        out.push(line);
    }
    let body_text = body.join("\n");
    if !body_text.trim().is_empty() {
        if body_text.trim_start().starts_with('{') || body_text.trim_start().starts_with('[') {
            out.push(compact_json_output(&body_text, generic));
        } else if body_text.trim_start().starts_with('<') {
            out.push(html_digest(&body_text, 5));
        } else {
            out.push(cap_lines(body, l.passthrough_max_lines, "lines"));
        }
    }
    if noise > 0 {
        out.push(format!("({} transfer lines dropped)", noise));
    }
    out.join("\n")
}

pub(crate) fn filter_ping(output: &str) -> String {
    let mut host = String::new();
    let mut stats: Option<String> = None;
    let mut rtt: Option<String> = None;
    let mut errors: Vec<String> = Vec::new();
    let mut replies = 0usize;
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix("PING ") {
            host = rest.split_whitespace().next().unwrap_or("").to_string();
            continue;
        }
        if t.contains("bytes from") {
            replies += 1;
            continue;
        }
        if t.contains("packets transmitted") {
            stats = Some(squeeze_ws(t));
            continue;
        }
        if t.starts_with("rtt ") || t.starts_with("round-trip") {
            rtt = Some(squeeze_ws(t));
            continue;
        }
        if t.contains("Unreachable")
            || t.contains("unknown host")
            || t.contains("failure")
            || t.starts_with("ping:")
        {
            errors.push(squeeze_ws(t));
            continue;
        }
        if t.starts_with("---") {
            continue;
        }
    }
    let mut out: Vec<String> = Vec::new();
    if let Some(s) = stats {
        out.push(format!("ping {}: {}", host, s));
    } else if replies > 0 {
        out.push(format!("ping {}: {} replies", host, replies));
    }
    if let Some(r) = rtt {
        out.push(r);
    }
    out.extend(errors);
    if out.is_empty() {
        return generic(output);
    }
    out.join("\n")
}

// ─── process / socket / disk / memory ─────────────────────────────────────────

pub(crate) fn filter_ps(_args: &[&str], output: &str) -> String {
    let l = limits();
    // (sort key, row) — biggest resident/CPU first, so the cap keeps what matters
    let mut rows: Vec<(u64, String)> = Vec::new();
    let mut kernel = 0usize;
    let mut header: Option<String> = None;
    let mut cols: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = line.trim_end();
        if t.trim().is_empty() {
            continue;
        }
        let up = t.trim_start();
        if header.is_none()
            && (up.starts_with("USER") || up.starts_with("UID") || up.starts_with("PID"))
        {
            cols = up
                .split_whitespace()
                .map(|c| c.to_ascii_uppercase())
                .collect();
            header = Some(squeeze_ws(t));
            continue;
        }
        // kernel threads carry no actionable information for an agent
        if t.contains('[') && t.trim_end().ends_with(']') {
            kernel += 1;
            continue;
        }
        let fields: Vec<&str> = t.split_whitespace().collect();
        let key = ["RSS", "%MEM", "%CPU", "VSZ"]
            .iter()
            .filter_map(|c| cols.iter().position(|h| h == c))
            .filter_map(|i| fields.get(i))
            .filter_map(|v| v.replace('.', "").parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        rows.push((key, truncate(&squeeze_ws(t), 160)));
    }
    if rows.is_empty() && header.is_none() {
        return generic(output);
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    let total = rows.len();
    let mut out: Vec<String> = header.into_iter().collect();
    let shown = total.min(l.ps_max_rows);
    out.extend(rows.into_iter().take(shown).map(|(_, r)| r));
    if total > shown {
        out.push(more(total - shown, "processes"));
    }
    if kernel > 0 {
        out.push(format!("({} kernel threads omitted)", kernel));
    }
    out.join("\n")
}

pub(crate) fn filter_ss(output: &str) -> String {
    let l = limits();
    let mut rows: Vec<(u32, String)> = Vec::new();
    let mut header: Option<String> = None;
    for line in output.lines() {
        let t = line.trim_end();
        if t.trim().is_empty() {
            continue;
        }
        if header.is_none()
            && (t.contains("Local Address") || t.starts_with("Netid") || t.starts_with("Proto"))
        {
            header = Some(squeeze_ws(t));
            continue;
        }
        let squeezed = squeeze_ws(t);
        // sort by local port so related sockets sit together
        // only `addr:port` fields count — a bare `0` in Recv-Q must not win
        let port = squeezed
            .split_whitespace()
            .filter(|f| f.contains(':'))
            .find_map(|f| f.rsplit(':').next().and_then(|p| p.parse::<u32>().ok()))
            .unwrap_or(0);
        rows.push((port, truncate(&squeezed, 200)));
    }
    rows.sort_by_key(|(p, _)| *p);
    let mut out: Vec<String> = header.into_iter().collect();
    out.extend(cap_vec(
        rows.into_iter().map(|(_, r)| r).collect(),
        l.ps_max_rows.max(l.list_max_lines / 2),
        "sockets",
    ));
    out.join("\n")
}

const PSEUDO_FS: [&str; 8] = [
    "tmpfs", "devtmpfs", "efivarfs", "squashfs", "overlay", "udev", "none", "shm",
];

pub(crate) fn filter_df(output: &str) -> String {
    let l = limits();
    let mut rows: Vec<String> = Vec::new();
    let mut pseudo = 0usize;
    let mut header: Option<String> = None;
    for line in output.lines() {
        let t = line.trim_end();
        if t.trim().is_empty() {
            continue;
        }
        if header.is_none() && (t.starts_with("Filesystem") || t.contains("Mounted on")) {
            header = Some(squeeze_ws(t));
            continue;
        }
        let fields: Vec<&str> = t.split_whitespace().collect();
        let fs = fields.first().copied().unwrap_or("");
        if PSEUDO_FS.iter().any(|p| fs.starts_with(p)) || fs.starts_with("/dev/loop") {
            pseudo += 1;
            continue;
        }
        rows.push(squeeze_ws(t));
    }
    let mut out: Vec<String> = header.into_iter().collect();
    out.extend(cap_vec(rows, l.list_max_lines, "filesystems"));
    if pseudo > 0 {
        out.push(format!("({} tmpfs/loop filesystems omitted)", pseudo));
    }
    out.join("\n")
}

pub(crate) fn filter_du(output: &str) -> String {
    let l = limits();
    let mut rows: Vec<(u64, String)> = Vec::new();
    for line in output.lines() {
        let t = line.trim_end();
        if t.trim().is_empty() {
            continue;
        }
        let mut it = t.split_whitespace();
        let size = it.next().unwrap_or("");
        let path = it.collect::<Vec<_>>().join(" ");
        let bytes = parse_size(size).unwrap_or(0);
        rows.push((bytes, format!("{} {}", size, path)));
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    cap_vec(
        rows.into_iter().map(|(_, r)| r).collect(),
        l.list_max_lines,
        "paths",
    )
    .join("\n")
}

pub(crate) fn filter_free(output: &str) -> String {
    let mut header: Vec<String> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let fields: Vec<&str> = t.split_whitespace().collect();
        if header.is_empty() && !t.contains(':') {
            header = fields.iter().map(|s| s.to_string()).collect();
            continue;
        }
        if let Some((label, rest)) = t.split_once(':') {
            let vals: Vec<&str> = rest.split_whitespace().collect();
            let pairs: Vec<String> = vals
                .iter()
                .enumerate()
                .map(|(i, v)| match header.get(i) {
                    Some(h) => format!("{} {}", h, v),
                    None => v.to_string(),
                })
                .collect();
            out.push(format!(
                "{}: {}",
                label.trim().to_ascii_lowercase(),
                pairs.join(", ")
            ));
        }
    }
    if out.is_empty() {
        return generic(output);
    }
    out.join("\n")
}

// ─── systemd ──────────────────────────────────────────────────────────────────

pub(crate) fn filter_systemctl(args: &[&str], output: &str) -> String {
    let l = limits();
    let sub = find_subcommand(args);
    if matches!(sub, Some("status") | Some("show")) {
        let mut out: Vec<String> = Vec::new();
        let mut journal: Vec<String> = Vec::new();
        let mut in_journal = false;
        for line in output.lines() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if in_journal {
                journal.push(truncate(&squeeze_ws(t), 200));
                continue;
            }
            if t.starts_with('●') || t.starts_with('×') || t.starts_with('○') {
                out.push(squeeze_ws(t));
                continue;
            }
            if let Some((k, v)) = t.split_once(':') {
                let key = k.trim();
                match key {
                    "Active" | "Main PID" | "Memory" | "CPU" | "Tasks" | "Since" => {
                        out.push(format!("{}: {}", key, squeeze_ws(v)));
                        continue;
                    }
                    "Loaded" => {
                        // `loaded (/lib/systemd/…; enabled; preset: enabled)` → state words only
                        let state = v
                            .split(&['(', ';'][..])
                            .map(|p| p.trim())
                            .filter(|p| {
                                matches!(
                                    *p,
                                    "enabled"
                                        | "disabled"
                                        | "static"
                                        | "masked"
                                        | "loaded"
                                        | "not-found"
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        out.push(format!(
                            "Loaded: {}",
                            if state.is_empty() {
                                squeeze_ws(v)
                            } else {
                                state
                            }
                        ));
                        continue;
                    }
                    "Docs" | "Process" | "CGroup" | "TriggeredBy" | "Triggers" => continue,
                    _ => {}
                }
            }
            // the journal tail starts at the first timestamped line
            if t.len() > 15 && t.contains(' ') && !t.contains(':') {
                continue;
            }
            in_journal = true;
            journal.push(truncate(&squeeze_ws(t), 200));
        }
        out.extend(fold_lines(journal, 10));
        if out.is_empty() {
            return generic(output);
        }
        return out.join("\n");
    }
    if matches!(
        sub,
        Some("list-units") | Some("list-unit-files") | Some("list-timers") | Some("list-sockets")
    ) {
        let mut rows: Vec<String> = Vec::new();
        let mut header: Option<String> = None;
        let mut legend = 0usize;
        for line in output.lines() {
            let t = line.trim_end();
            let tt = t.trim();
            if tt.is_empty() {
                continue;
            }
            if tt.starts_with("Legend:")
                || tt.starts_with("LOAD ")
                || tt.starts_with("ACTIVE ")
                || tt.starts_with("SUB ")
                || tt.contains("loaded units listed")
                || tt.starts_with("To show all")
                || tt.starts_with("unit files listed")
            {
                legend += 1;
                continue;
            }
            if header.is_none() && (tt.starts_with("UNIT") || tt.starts_with("NEXT")) {
                header = Some(squeeze_ws(tt));
                continue;
            }
            rows.push(truncate(&squeeze_ws(tt), 200));
        }
        let n = rows.len();
        let mut out: Vec<String> = header.into_iter().collect();
        out.extend(cap_vec(rows, l.list_max_lines, "units"));
        out.push(format!(
            "({} listed{})",
            n,
            if legend > 0 { ", legend dropped" } else { "" }
        ));
        return out.join("\n");
    }
    // is-active / is-enabled / enable / disable → already terse
    generic(output)
}

pub(crate) fn filter_journalctl(output: &str) -> String {
    let l = limits();
    let mut lines: Vec<String> = Vec::new();
    let mut headers = 0usize;
    for line in output.lines() {
        let t = line.trim_end();
        if t.trim().is_empty() {
            continue;
        }
        if t.starts_with("-- ") {
            headers += 1;
            continue;
        }
        // `Sep 09 08:14:12 hostname unit[1234]: message` → `08:14:12 unit: message`
        let fields: Vec<&str> = t.splitn(6, ' ').collect();
        let compacted = if fields.len() >= 6 && fields[2].contains(':') {
            let time = fields[2];
            let unit = fields[4]
                .split('[')
                .next()
                .unwrap_or(fields[4])
                .trim_end_matches(':');
            format!("{} {}: {}", time, unit, fields[5])
        } else {
            squeeze_ws(t)
        };
        lines.push(truncate(&compacted, 300));
    }
    let mut out = fold_lines(lines, l.log_tail);
    if headers > 0 {
        out.push(format!("({} boot markers dropped)", headers));
    }
    out.join("\n")
}

// ─── env / man ────────────────────────────────────────────────────────────────

const SECRET_KEYS: [&str; 9] = [
    "secret",
    "token",
    "password",
    "passwd",
    "api_key",
    "apikey",
    "private",
    "credential",
    "auth",
];

pub(crate) fn filter_env(output: &str) -> String {
    let l = limits();
    let mut rows: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = line.trim_end();
        if t.trim().is_empty() {
            continue;
        }
        let Some((k, v)) = t.split_once('=') else {
            rows.push(truncate(t, 200));
            continue;
        };
        if k == "LS_COLORS" {
            rows.push("LS_COLORS=[dropped]".to_string());
            continue;
        }
        let lk = k.to_ascii_lowercase();
        if SECRET_KEYS.iter().any(|s| lk.contains(s)) {
            rows.push(format!("{}=***", k));
            continue;
        }
        rows.push(format!("{}={}", k, truncate(v, 80)));
    }
    rows.sort();
    cap_vec(rows, l.list_max_lines, "vars").join("\n")
}

const MAN_DROP: [&str; 6] = [
    "AUTHOR",
    "REPORTING BUGS",
    "COPYRIGHT",
    "SEE ALSO",
    "COLOPHON",
    "BUGS",
];

pub(crate) fn filter_man(output: &str) -> String {
    let l = limits();
    let mut out: Vec<String> = Vec::new();
    let mut section = String::new();
    let mut dropped_sections: Vec<String> = Vec::new();
    let mut entries = 0usize;
    let mut dropped_entries = 0usize;
    for line in output.lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        // section headings are unindented and upper-case
        let is_heading = !t.starts_with(' ')
            && tt
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == ' ' || c == '-')
            && tt.len() > 2;
        if is_heading {
            section = tt.to_string();
            if MAN_DROP.iter().any(|d| section.starts_with(d)) {
                dropped_sections.push(section.clone());
            } else {
                out.push(section.clone());
            }
            entries = 0;
            continue;
        }
        if MAN_DROP.iter().any(|d| section.starts_with(d)) {
            continue;
        }
        // page header/footer, e.g. `LS(1)   User Commands   LS(1)`
        if !t.starts_with(' ')
            && tt.contains('(')
            && tt.contains(')')
            && tt.split_whitespace().count() <= 6
        {
            continue;
        }
        if section.starts_with("OPTIONS") {
            if entries >= l.list_max_lines {
                dropped_entries += 1;
                continue;
            }
            entries += 1;
        }
        out.push(squeeze_ws(t));
    }
    if dropped_entries > 0 {
        out.push(more(dropped_entries, "option lines"));
    }
    if !dropped_sections.is_empty() {
        out.push(format!(
            "[+{} more sections: {}]",
            dropped_sections.len(),
            dropped_sections.join(", ")
        ));
    }
    if out.is_empty() {
        return generic(output);
    }
    cap_lines(
        out.iter().map(|s| s.as_str()),
        l.passthrough_max_lines,
        "lines",
    )
}

// ─── act / generic lint ───────────────────────────────────────────────────────

pub(crate) fn filter_act(output: &str) -> String {
    let l = limits();
    let mut jobs: Vec<(String, usize, Vec<String>)> = Vec::new();
    let mut failing: Option<String> = None;
    for line in output.lines() {
        let t = strip_ansi(line.trim_end());
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        // `[build/test] ⭐ Run Main step name`
        let job = tt
            .strip_prefix('[')
            .and_then(|r| r.split(']').next())
            .map(String::from);
        let Some(job) = job else {
            if let Some(f) = &failing {
                if let Some((_, _, lines)) = jobs.iter_mut().find(|(j, _, _)| j == f) {
                    if lines.len() < 10 {
                        lines.push(truncate(tt, 200));
                    }
                }
            }
            continue;
        };
        let body = tt.split(']').nth(1).unwrap_or("").trim().to_string();
        if jobs.iter().all(|(j, _, _)| *j != job) {
            jobs.push((job.clone(), 0, Vec::new()));
        }
        let entry = jobs.iter_mut().find(|(j, _, _)| *j == job).unwrap();
        if body.contains("Success") || body.starts_with('✅') {
            entry.1 += 1;
            failing = None;
            continue;
        }
        if body.contains("Failure") || body.starts_with('❌') || body.contains("failed") {
            entry.2.push(truncate(&body, 200));
            failing = Some(job);
            continue;
        }
        if body.contains("docker")
            || body.starts_with('🐳')
            || body.starts_with('⭐')
            || body.starts_with("⚙")
        {
            continue;
        }
    }
    let mut out: Vec<String> = Vec::new();
    for (job, ok, fails) in &jobs {
        let mut line = format!("{}: {} ok", job, ok);
        if !fails.is_empty() {
            line.push_str(&format!(", {} failed", fails.len()));
        }
        out.push(line);
        out.extend(fails.iter().map(|f| format!("  {}", f)));
    }
    if out.is_empty() {
        return generic(output);
    }
    cap_vec(out, l.max_diagnostics, "lines").join("\n")
}

pub(crate) fn filter_lint(output: &str) -> String {
    let l = limits();
    let mut files: Vec<(String, Vec<String>)> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    let mut count = 0usize;
    for line in output.lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        let mut it = tt.splitn(4, ':');
        let path = it.next().unwrap_or("");
        let ln = it.next().unwrap_or("").trim();
        if !path.is_empty() && !ln.is_empty() && ln.chars().all(|c| c.is_ascii_digit()) {
            let third = it.next().unwrap_or("");
            let (pos, msg) =
                if third.trim().chars().all(|c| c.is_ascii_digit()) && !third.trim().is_empty() {
                    (
                        format!("{}:{}", ln, third.trim()),
                        it.next().unwrap_or("").trim().to_string(),
                    )
                } else {
                    (
                        ln.to_string(),
                        format!(
                            "{}{}",
                            third,
                            it.next().map(|r| format!(":{}", r)).unwrap_or_default()
                        ),
                    )
                };
            count += 1;
            let entry = format!("  {}  {}", pos, truncate(&squeeze_ws(&msg), 200));
            match files.iter_mut().find(|(p, _)| *p == path) {
                Some((_, v)) => v.push(entry),
                None => files.push((path.to_string(), vec![entry])),
            }
            continue;
        }
        other.push(squeeze_ws(tt));
    }
    if files.is_empty() {
        return cap_vec(other, l.passthrough_max_lines, "lines").join("\n");
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
        out.push(more(dropped, "diagnostics"));
    }
    out.extend(cap_vec(other, 10, "lines"));
    out.push(format!(
        "lint: {} in {}",
        plural(count, "issue"),
        plural(files.len(), "file")
    ));
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curl_json_body_is_compacted() {
        let out = filter_curl(
            &["-s", "http://x/health"],
            "{\"status\":\"ok\",\"mcp\":true,\"version\":\"2024-11-05\"}",
        );
        assert!(out.contains("status: ok"), "{}", out);
        assert!(out.contains("mcp: true"), "{}", out);
    }

    #[test]
    fn curl_headers_keep_status_and_useful_fields() {
        let out = filter_curl(
            &["-sI", "https://example.com"],
            "HTTP/2 200 \ncontent-type: text/html; charset=UTF-8\ndate: Tue, 09 Sep 2026 08:00:00 GMT\nserver: ECAcc\nx-ratelimit-remaining: 59\nalt-svc: h3=\":443\"\ncontent-length: 1256",
        );
        assert!(out.contains("HTTP/2 200"), "{}", out);
        assert!(
            out.contains("content-type: text/html; charset=UTF-8"),
            "{}",
            out
        );
        assert!(out.contains("x-ratelimit-remaining: 59"), "{}", out);
        assert!(out.contains("content-length: 1256"), "{}", out);
        assert!(!out.contains("alt-svc"), "{}", out);
        assert!(out.contains("header/progress lines dropped"), "{}", out);
    }

    #[test]
    fn curl_verbose_keeps_errors_and_html_is_digested() {
        let v = filter_curl(
            &["-sv", "http://127.0.0.1:1/x"],
            "*   Trying 127.0.0.1:1...\n* connect to 127.0.0.1 port 1 failed: Connection refused\n* Closing connection 0\ncurl: (7) Failed to connect to 127.0.0.1 port 1",
        );
        assert!(v.contains("Connection refused"), "{}", v);
        assert!(v.contains("curl: (7)"), "{}", v);
        let html = filter_curl(
            &["-s", "https://example.com"],
            "<html><head><title>Example Domain</title><style>a{}</style></head><body><h1>Example Domain</h1><p>Use in docs.</p><script>x()</script></body></html>",
        );
        assert!(html.contains("title: Example Domain"), "{}", html);
        assert!(html.contains("Example Domain Use in docs."), "{}", html);
        assert!(!html.contains("x()"), "{}", html);
    }

    #[test]
    fn wget_collapses_transfer_to_one_line() {
        let out = filter_wget(
            "--2026-09-09 08:00:00--  https://example.com/\nResolving example.com... 93.184.215.14\nConnecting to example.com|93.184.215.14|:443... connected.\nHTTP request sent, awaiting response... 200 OK\nLength: 1256 (1.2K) [text/html]\nSaving to: 'index.html'\n\nindex.html    100%[=================>]   1.23K  --.-KB/s    in 0s\n\n2026-09-09 08:00:00 (12.3 MB/s) - 'index.html' saved [1256/1256]",
        );
        assert!(out.contains("index.html"), "{}", out);
        assert!(out.contains("[200 OK]"), "{}", out);
        assert!(out.contains("(1256"), "{}", out);
        assert!(!out.contains("Resolving"), "{}", out);
        let err = filter_wget(
            "--2026-09-09--  https://x/404\nHTTP request sent, awaiting response... 404 Not Found\nERROR 404: Not Found.",
        );
        assert!(err.contains("ERROR 404: Not Found."), "{}", err);
    }

    #[test]
    fn ping_keeps_only_statistics() {
        let out = filter_ping(
            "PING 127.0.0.1 (127.0.0.1) 56(84) bytes of data.\n64 bytes from 127.0.0.1: icmp_seq=1 ttl=64 time=0.048 ms\n64 bytes from 127.0.0.1: icmp_seq=2 ttl=64 time=0.052 ms\n\n--- 127.0.0.1 ping statistics ---\n2 packets transmitted, 2 received, 0% packet loss, time 1002ms\nrtt min/avg/max/mdev = 0.048/0.050/0.052/0.002 ms",
        );
        assert!(
            out.starts_with("ping 127.0.0.1: 2 packets transmitted, 2 received, 0% packet loss"),
            "{}",
            out
        );
        assert!(out.contains("rtt min/avg/max/mdev"), "{}", out);
        assert!(!out.contains("icmp_seq"), "{}", out);
    }

    #[test]
    fn ps_drops_kernel_threads_and_announces_cap() {
        let mut raw =
            String::from("USER   PID  %CPU %MEM    VSZ   RSS TTY   STAT START   TIME COMMAND\n");
        raw.push_str("root     1   0.0  0.1 168000 12000 ?     Ss   08:00   0:01 /sbin/init\n");
        for i in 0..5 {
            raw.push_str(&format!(
                "root    {}   0.0  0.0      0     0 ?        I<   08:00   0:00 [kworker/{}:0H]\n",
                100 + i,
                i
            ));
        }
        for i in 0..300 {
            raw.push_str(&format!(
                "u      {}   0.1  0.2  50000  8000 ?        Sl   08:00   0:00 proc{}\n",
                1000 + i,
                i
            ));
        }
        let out = filter_ps(&["aux"], &raw);
        assert!(
            out.starts_with("USER PID %CPU %MEM VSZ RSS TTY STAT START TIME COMMAND"),
            "{}",
            out
        );
        assert!(out.contains("/sbin/init"), "{}", out);
        assert!(out.contains("(5 kernel threads omitted)"), "{}", out);
        assert!(has_truncation(&out), "{}", out);
    }

    #[test]
    fn ss_sorts_by_port_and_df_drops_pseudo_filesystems() {
        let ss = filter_ss(
            "Netid State  Recv-Q Send-Q Local Address:Port  Peer Address:Port Process\ntcp   LISTEN 0      4096   127.0.0.1:27182     0.0.0.0:*  users:((\"prism\"))\ntcp   LISTEN 0      511    0.0.0.0:80          0.0.0.0:*  users:((\"nginx\"))",
        );
        let lines: Vec<&str> = ss.lines().collect();
        assert!(lines[1].contains(":80"), "{}", ss);
        assert!(lines[2].contains(":27182"), "{}", ss);
        let df = filter_df(
            "Filesystem      Size  Used Avail Use% Mounted on\ntmpfs           1.6G  2.4M  1.6G   1% /run\n/dev/nvme0n1p2  468G  372G   72G  84% /\n/dev/loop12      64M   64M     0 100% /snap/core20/2318",
        );
        assert!(df.contains("/dev/nvme0n1p2 468G 372G 72G 84% /"), "{}", df);
        assert!(df.contains("(2 tmpfs/loop filesystems omitted)"), "{}", df);
    }

    #[test]
    fn du_sorts_desc_and_free_is_two_lines() {
        let du = filter_du("4.0K\tprism/.cargo\n11G\tprism/target\n2.1M\tprism/src\n11G\tprism");
        let first = du.lines().next().unwrap();
        assert!(first.starts_with("11G"), "{}", du);
        assert!(du.lines().last().unwrap().starts_with("4.0K"), "{}", du);
        let free = filter_free(
            "               total        used        free      shared  buff/cache   available\nMem:            15Gi        10Gi       1.0Gi       300Mi       4.0Gi       5.0Gi\nSwap:          4.0Gi       1.0Gi       3.0Gi",
        );
        assert!(free.contains("mem: total 15Gi, used 10Gi"), "{}", free);
        assert!(free.contains("swap: total 4.0Gi"), "{}", free);
    }

    #[test]
    fn systemctl_status_keeps_active_and_folds_journal() {
        let out = filter_systemctl(
            &["status", "docker"],
            "● docker.service - Docker Application Container Engine\n     Loaded: loaded (/lib/systemd/system/docker.service; enabled; preset: enabled)\n     Active: active (running) since Mon 2026-09-08 12:00:00 IST; 20h ago\n       Docs: https://docs.docker.com\n   Main PID: 1234 (dockerd)\n      Tasks: 42\n     Memory: 180.2M\n        CPU: 5min 2s\n     CGroup: /system.slice/docker.service\n             └─1234 /usr/bin/dockerd\n\nSep 09 08:00:01 host dockerd[1234]: level=info msg=\"starting\"\nSep 09 08:00:02 host dockerd[1234]: level=info msg=\"starting\"",
        );
        assert!(out.contains("docker.service"), "{}", out);
        assert!(out.contains("Active: active (running)"), "{}", out);
        assert!(out.contains("Main PID: 1234 (dockerd)"), "{}", out);
        assert!(out.contains("Loaded: loaded, enabled"), "{}", out);
        assert!(!out.contains("docs.docker.com"), "{}", out);
        assert!(!out.contains("CGroup"), "{}", out);
    }

    #[test]
    fn systemctl_list_units_drops_legend_and_counts() {
        let out = filter_systemctl(
            &["list-units", "--type=service"],
            "  UNIT              LOAD   ACTIVE SUB     DESCRIPTION\n  dbus.service      loaded active running D-Bus User Message Bus\n  dconf.service     loaded active running User preferences database\n\nLOAD   = Reflects whether the unit definition was properly loaded.\nACTIVE = The high-level unit activation state.\n\n2 loaded units listed.",
        );
        assert!(out.contains("UNIT LOAD ACTIVE SUB DESCRIPTION"), "{}", out);
        assert!(
            out.contains("dbus.service loaded active running"),
            "{}",
            out
        );
        assert!(out.contains("(2 listed, legend dropped)"), "{}", out);
        assert!(!out.contains("Reflects whether"), "{}", out);
    }

    #[test]
    fn journalctl_compacts_prefix_and_folds_repeats() {
        let mut raw = String::from("-- Logs begin at Mon 2026-09-08 12:00:00 IST. --\n");
        for _ in 0..50 {
            raw.push_str("Sep 09 08:14:12 myhost prism[1234]: heartbeat ok\n");
        }
        raw.push_str("Sep 09 08:15:00 myhost prism[1234]: ERROR upstream refused\n");
        let out = filter_journalctl(&raw);
        assert!(
            out.contains("08:14:12 prism: heartbeat ok  (×50)"),
            "{}",
            out
        );
        assert!(out.contains("ERROR upstream refused"), "{}", out);
        assert!(out.contains("(1 boot markers dropped)"), "{}", out);
        assert!(!out.contains("myhost"), "{}", out);
    }

    #[test]
    fn env_masks_secrets_sorts_and_drops_ls_colors() {
        let out = filter_env(
            "PATH=/usr/bin:/bin\nGITHUB_TOKEN=ghp_realsecretvalue\nAWS_SECRET_ACCESS_KEY=abcd\nLS_COLORS=rs=0:di=01;34:ln=01;36\nHOME=/home/u\nDB_PASSWORD=hunter2",
        );
        assert!(out.contains("GITHUB_TOKEN=***"), "{}", out);
        assert!(out.contains("AWS_SECRET_ACCESS_KEY=***"), "{}", out);
        assert!(out.contains("DB_PASSWORD=***"), "{}", out);
        assert!(!out.contains("ghp_realsecretvalue"), "{}", out);
        assert!(!out.contains("hunter2"), "{}", out);
        assert!(out.contains("LS_COLORS=[dropped]"), "{}", out);
        assert!(out.contains("PATH=/usr/bin:/bin"), "{}", out);
        let lines: Vec<&str> = out.lines().collect();
        assert!(
            lines.windows(2).all(|w| w[0] <= w[1]),
            "not sorted: {}",
            out
        );
    }

    #[test]
    fn man_keeps_synopsis_drops_trailing_sections() {
        let out = filter_man(
            "LS(1)                            User Commands                           LS(1)\n\nNAME\n       ls - list directory contents\n\nSYNOPSIS\n       ls [OPTION]... [FILE]...\n\nDESCRIPTION\n       List information about the FILEs.\n\nOPTIONS\n       -a, --all\n              do not ignore entries starting with .\n\nAUTHOR\n       Written by Richard M. Stallman.\n\nCOPYRIGHT\n       Copyright (C) 2024 Free Software Foundation\n\nSEE ALSO\n       dir(1)\n",
        );
        assert!(out.contains("NAME"), "{}", out);
        assert!(out.contains("ls - list directory contents"), "{}", out);
        assert!(out.contains("SYNOPSIS"), "{}", out);
        assert!(out.contains("-a, --all"), "{}", out);
        assert!(!out.contains("Richard M. Stallman"), "{}", out);
        assert!(!out.contains("Free Software Foundation"), "{}", out);
        assert!(
            out.contains("more sections: AUTHOR, COPYRIGHT, SEE ALSO"),
            "{}",
            out
        );
    }

    #[test]
    fn act_summarises_jobs_and_lint_groups_by_file() {
        let act = filter_act(
            "[build/test] 🚀  Start image=node:20\n[build/test]   ⭐ Run Main Set up\n[build/test]   ✅  Success - Main Set up\n[build/test]   ⭐ Run Main Tests\n[build/test]   ❌  Failure - Main Tests\n[build/test] exitcode '1': failure\n[build/lint] ✅  Success - Main Lint",
        );
        assert!(act.contains("build/test:"), "{}", act);
        assert!(act.contains("failed"), "{}", act);
        assert!(act.contains("build/lint: 1 ok"), "{}", act);
        let lint = filter_lint(
            "src/a.js:12:5: Unexpected console statement\nsrc/a.js:20:1: Missing semicolon\nsrc/b.js:3:9: Unused var",
        );
        assert!(
            lint.contains("src/a.js\n  12:5  Unexpected console statement"),
            "{}",
            lint
        );
        assert!(lint.contains("lint: 3 issues in 2 files"), "{}", lint);
    }

    #[test]
    fn empty_inputs_are_safe() {
        assert_eq!(filter_curl(&[], ""), "");
        assert_eq!(filter_wget(""), "");
        assert_eq!(filter_ps(&[], ""), "");
        assert_eq!(filter_ss(""), "");
        assert_eq!(filter_df(""), "");
        assert_eq!(filter_du(""), "");
        assert_eq!(filter_env(""), "");
        assert_eq!(filter_journalctl(""), "");
        assert_eq!(filter_lint(""), "");
    }
}
