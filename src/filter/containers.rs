//! Container & orchestration filters: `docker`/`podman`, compose, `kubectl`/`oc`,
//! `helm`, `stern`, `k9s`.
//!
//! Tables are sliced at their header's column offsets (so an empty PORTS cell can't
//! shift NAMES into it), logs are folded by digit-masked template, and `inspect` JSON
//! is pruned of empty fields before rendering.

use super::common::*;
use serde_json::Value;

// ─── fixed-width tables ───────────────────────────────────────────────────────

/// Byte offsets where each header column starts.
fn header_starts(line: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b' ' && (i == 0 || b[i - 1] == b' ') {
            // a single space inside a header name ("CONTAINER ID") is not a new column
            let two_spaces_before = i >= 2 && b[i - 1] == b' ' && b[i - 2] == b' ';
            if i == 0 || two_spaces_before {
                starts.push(i);
            }
        }
        i += 1;
    }
    starts
}

/// Slice `line` at `starts`, keeping empty cells in position.
fn row_cells(line: &str, starts: &[usize]) -> Vec<String> {
    let mut cells = Vec::with_capacity(starts.len());
    for (i, s) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(usize::MAX);
        let cell = if *s >= line.len() {
            ""
        } else {
            let e = end.min(line.len());
            // never split a UTF-8 char
            let s2 = floor_boundary(line, *s);
            let e2 = floor_boundary(line, e);
            &line[s2..e2.max(s2)]
        };
        cells.push(cell.trim().to_string());
    }
    cells
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    if i > s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Squeeze a whitespace-aligned table into `a|b|c` rows, header included.
pub(super) fn squeeze_table(output: &str, max: usize) -> String {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut out: Vec<String> = Vec::new();
    let mut dropped = 0usize;
    for l in &lines {
        if out.len() < max {
            out.push(squeeze_ws(l));
        } else {
            dropped += 1;
        }
    }
    if dropped > 0 {
        out.push(more(dropped, "rows"));
    }
    out.join("\n")
}

// ─── log folding ──────────────────────────────────────────────────────────────

/// Replace digit runs and long hex blobs with `#` so otherwise-identical log lines
/// group together.
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
    is_alert_line(line)
}

/// Fold repeated log templates, then keep the last `tail` lines (alerts pinned).
pub(super) fn fold_logs(output: &str, tail: usize) -> String {
    let mut groups: Vec<(String, String, usize)> = Vec::new(); // template, first line, count
    for line in output.lines() {
        let t = line.trim_end();
        if t.is_empty() {
            continue;
        }
        let key = log_template(t);
        match groups.last_mut() {
            Some((k, _, n)) if *k == key => *n += 1,
            _ => groups.push((key, t.to_string(), 1)),
        }
    }
    let rendered: Vec<String> = groups
        .iter()
        .map(|(_, first, n)| {
            if *n > 1 {
                format!("{}  (×{}, {} similar folded)", first, n, n - 1)
            } else {
                first.clone()
            }
        })
        .collect();
    if rendered.len() <= tail {
        return rendered.join("\n");
    }
    // keep the tail plus any earlier alert lines
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
    out.join("\n")
}

// ─── JSON pruning ─────────────────────────────────────────────────────────────

const JSON_NOISE_KEYS: [&str; 6] = [
    "GraphDriver",
    "managedFields",
    "kubectl.kubernetes.io/last-applied-configuration",
    "ResourceVersion",
    "SecretID",
    "Propagation",
];

fn is_empty_value(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        Value::Bool(b) => !*b,
        _ => false,
    }
}

/// Drop empty/false fields and known noise keys; returns how many were removed.
fn prune(v: &mut Value, dropped: &mut usize) {
    match v {
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for k in keys {
                if JSON_NOISE_KEYS.contains(&k.as_str()) {
                    map.remove(&k);
                    *dropped += 1;
                    continue;
                }
                if let Some(child) = map.get_mut(&k) {
                    prune(child, dropped);
                    if is_empty_value(child) {
                        map.remove(&k);
                        *dropped += 1;
                    }
                }
            }
        }
        Value::Array(items) => {
            for it in items.iter_mut() {
                prune(it, dropped);
            }
        }
        _ => {}
    }
}

pub(super) fn pruned_json(output: &str, fallback: impl Fn(&str) -> String) -> String {
    match parse_json(output) {
        Some(mut docs) => {
            let mut dropped = 0usize;
            for d in docs.iter_mut() {
                prune(d, &mut dropped);
            }
            let mut s = docs
                .iter()
                .map(compact_json)
                .collect::<Vec<_>>()
                .join("\n---\n");
            if dropped > 0 {
                s.push('\n');
                s.push_str(&more(dropped, "empty fields"));
            }
            s
        }
        None => fallback(output),
    }
}

// ─── docker ───────────────────────────────────────────────────────────────────

/// `0.0.0.0:5173->5173/tcp, [::]:5173->5173/tcp` → `5173`
fn compact_ports(field: &str) -> String {
    let mut seen: Vec<String> = Vec::new();
    for spec in field.split(',') {
        let spec = spec.trim();
        if spec.is_empty() {
            continue;
        }
        let p = match spec.split_once("->") {
            Some((host, cont)) => {
                let hp = host.rsplit(':').next().unwrap_or(host);
                let cp = cont.split('/').next().unwrap_or(cont);
                if hp == cp {
                    cp.to_string()
                } else {
                    format!("{}->{}", hp, cp)
                }
            }
            None => spec.split('/').next().unwrap_or(spec).to_string(),
        };
        if !p.is_empty() && !seen.contains(&p) {
            seen.push(p);
        }
    }
    seen.join(",")
}

pub(crate) fn filter_docker_ps(output: &str) -> String {
    let l = limits();
    let lines: Vec<&str> = output.lines().filter(|x| !x.trim().is_empty()).collect();
    let Some(header) = lines.first() else {
        return String::new();
    };
    if !header.contains("CONTAINER ID") && !header.contains("NAMES") && !header.contains("NAME") {
        return squeeze_table(output, l.list_max_lines); // --format output
    }
    let starts = header_starts(header);
    let idx = |name: &str| -> Option<usize> {
        starts
            .iter()
            .position(|s| header[floor_boundary(header, *s)..].starts_with(name))
    };
    let (i_id, i_img, i_status, i_ports, i_name) = (
        idx("CONTAINER ID"),
        idx("IMAGE"),
        idx("STATUS"),
        idx("PORTS"),
        idx("NAMES").or_else(|| idx("NAME")),
    );
    let mut running: Vec<String> = Vec::new();
    let mut stopped: Vec<String> = Vec::new();
    for line in &lines[1..] {
        let cells = row_cells(line, &starts);
        let get = |i: Option<usize>| i.and_then(|i| cells.get(i)).cloned().unwrap_or_default();
        let id = get(i_id);
        let name = get(i_name);
        let image = get(i_img);
        let status = get(i_status);
        let ports = compact_ports(&get(i_ports));
        let mut row = format!(
            "  {} {} ({}) {}",
            &id[..id.len().min(12)],
            name,
            image,
            status
        );
        if !ports.is_empty() {
            row.push_str(&format!(" [{}]", ports));
        }
        if status.starts_with("Up") {
            running.push(row);
        } else {
            stopped.push(row);
        }
    }
    if running.is_empty() && stopped.is_empty() {
        return "[docker] no containers".to_string();
    }
    let mut out = Vec::new();
    for (label, mut group) in [("running", running), ("stopped/exited", stopped)] {
        if group.is_empty() {
            continue;
        }
        out.push(format!("[docker] {} {}:", group.len(), label));
        let n = group.len();
        if n > l.list_max_lines {
            group.truncate(l.list_max_lines);
            group.push(format!("  {}", more(n - l.list_max_lines, "containers")));
        }
        out.extend(group);
    }
    out.join("\n")
}

pub(crate) fn filter_docker_images(output: &str) -> String {
    let l = limits();
    let lines: Vec<&str> = output.lines().filter(|x| !x.trim().is_empty()).collect();
    let Some(header) = lines.first() else {
        return String::new();
    };
    let up = header.to_ascii_uppercase();
    if !up.contains("IMAGE") && !up.contains("REPOSITORY") {
        return squeeze_table(output, l.list_max_lines);
    }
    let starts = header_starts(header);
    let idx = |name: &str| -> Option<usize> {
        starts
            .iter()
            .position(|s| header[floor_boundary(header, *s)..].starts_with(name))
    };
    let i_repo = idx("REPOSITORY").or_else(|| idx("IMAGE"));
    let i_tag = idx("TAG");
    let i_size = idx("SIZE").or_else(|| idx("DISK USAGE"));
    let mut rows: Vec<String> = Vec::new();
    let mut total = 0u64;
    let mut dangling = 0usize;
    let mut dangling_bytes = 0u64;
    for line in &lines[1..] {
        let cells = row_cells(line, &starts);
        let get = |i: Option<usize>| i.and_then(|i| cells.get(i)).cloned().unwrap_or_default();
        let repo = get(i_repo);
        let tag = get(i_tag);
        let size = get(i_size);
        let bytes = parse_size(&size).unwrap_or(0);
        total += bytes;
        let name = if tag.is_empty() || repo.contains(':') {
            repo.clone()
        } else {
            format!("{}:{}", repo, tag)
        };
        if name.starts_with("<none>") || name.ends_with("<none>") {
            dangling += 1;
            dangling_bytes += bytes;
            continue;
        }
        rows.push(format!("  {} {}", name, size));
    }
    let mut out = vec![format!(
        "[docker] {} images ({})",
        rows.len() + dangling,
        human_size(total)
    )];
    let n = rows.len();
    if n > l.list_max_lines {
        rows.truncate(l.list_max_lines);
        rows.push(format!("  {}", more(n - l.list_max_lines, "images")));
    }
    out.extend(rows);
    if dangling > 0 {
        out.push(format!(
            "  {} dangling <none> images {}",
            dangling,
            human_size(dangling_bytes)
        ));
    }
    out.join("\n")
}

/// `docker network ls` / `volume ls`: name-first rows, anonymous volumes folded.
fn docker_ls_resource(output: &str, kind: &str) -> String {
    let l = limits();
    let lines: Vec<&str> = output.lines().filter(|x| !x.trim().is_empty()).collect();
    let Some(header) = lines.first() else {
        return String::new();
    };
    let starts = header_starts(header);
    let up = header.to_ascii_uppercase();
    let i_name = starts
        .iter()
        .position(|s| {
            let h = up[floor_boundary(&up, *s)..].trim_start();
            h.starts_with("NAME") || h.starts_with("VOLUME NAME")
        })
        .unwrap_or(starts.len().saturating_sub(1));
    let i_driver = starts
        .iter()
        .position(|s| up[floor_boundary(&up, *s)..].starts_with("DRIVER"));
    let mut named: Vec<String> = Vec::new();
    let mut anon = 0usize;
    let mut builtin: Vec<String> = Vec::new();
    for line in &lines[1..] {
        let cells = row_cells(line, &starts);
        let name = cells.get(i_name).cloned().unwrap_or_default();
        let driver = i_driver
            .and_then(|i| cells.get(i))
            .cloned()
            .unwrap_or_default();
        if name.len() == 64 && name.chars().all(|c| c.is_ascii_hexdigit()) {
            anon += 1;
            continue;
        }
        if kind == "network" && matches!(name.as_str(), "bridge" | "host" | "none") {
            builtin.push(name);
            continue;
        }
        named.push(if driver.is_empty() {
            format!("  {}", name)
        } else {
            format!("  {} ({})", name, driver)
        });
    }
    let mut out = vec![format!(
        "[docker] {} {}s",
        named.len() + anon + builtin.len(),
        kind
    )];
    let n = named.len();
    if n > l.list_max_lines {
        named.truncate(l.list_max_lines);
        named.push(format!("  {}", more(n - l.list_max_lines, "entries")));
    }
    out.extend(named);
    if !builtin.is_empty() {
        out.push(format!("  builtin: {}", builtin.join(", ")));
    }
    if anon > 0 {
        out.push(format!("  {}", more(anon, "anonymous volumes")));
    }
    out.join("\n")
}

fn docker_version(output: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut section = String::new();
    let mut bits: Vec<String> = Vec::new();
    let flush = |section: &mut String, bits: &mut Vec<String>, out: &mut Vec<String>| {
        if !section.is_empty() && !bits.is_empty() {
            out.push(format!("{}: {}", section, bits.join(", ")));
        }
        bits.clear();
    };
    for line in output.lines() {
        let t = line.trim_end();
        if t.is_empty() {
            continue;
        }
        if !t.starts_with(' ') {
            flush(&mut section, &mut bits, &mut out);
            section = t.trim_end_matches(':').to_string();
            continue;
        }
        if let Some((k, v)) = t.split_once(':') {
            let k = k.trim();
            let v = v.trim();
            if matches!(
                k,
                "Version" | "API version" | "Go version" | "OS/Arch" | "Git commit"
            ) && !v.is_empty()
            {
                bits.push(format!(
                    "{} {}",
                    k.to_ascii_lowercase().replace(" version", ""),
                    v
                ));
            }
        }
    }
    flush(&mut section, &mut bits, &mut out);
    if out.is_empty() {
        return generic(output);
    }
    out.join("\n")
}

const INFO_KEYS: [&str; 13] = [
    "Server Version",
    "Storage Driver",
    "Cgroup Driver",
    "Cgroup Version",
    "Kernel Version",
    "Operating System",
    "OSType",
    "Architecture",
    "CPUs",
    "Total Memory",
    "Docker Root Dir",
    "Containers",
    "Images",
];

fn docker_info(output: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with("WARNING") || t.starts_with("ERROR") {
            out.push(t.to_string());
            continue;
        }
        if let Some((k, v)) = t.split_once(':') {
            let k = k.trim();
            if INFO_KEYS.contains(&k) {
                out.push(format!("{}: {}", k, v.trim()));
            } else if matches!(k, "Running" | "Paused" | "Stopped") {
                out.push(format!("  {}: {}", k.to_ascii_lowercase(), v.trim()));
            }
        }
    }
    if out.is_empty() {
        return generic(output);
    }
    out.join("\n")
}

fn docker_build(output: &str) -> String {
    let l = limits();
    let mut steps = 0usize;
    let mut cached = 0usize;
    let mut keep: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if tt.contains("CACHED") {
            cached += 1;
            continue;
        }
        if tt.starts_with('#') || tt.starts_with("Step ") || tt.starts_with(" ---> ") {
            steps += 1;
            continue;
        }
        if tt.starts_with("Sending build context")
            || tt.starts_with("Removing intermediate container")
            || tt.ends_with("Preparing")
            || tt.ends_with("Pushed")
            || tt.ends_with("Waiting")
            || tt.ends_with("Already exists")
            || tt.ends_with("Download complete")
            || tt.ends_with("Pull complete")
            || tt.ends_with("Layer already exists")
        {
            steps += 1;
            continue;
        }
        keep.push(shorten_digests(tt));
    }
    let mut out = cap_vec(keep, l.max_diagnostics, "lines");
    out.push(format!("[docker] {} steps, {} cached", steps, cached));
    out.join("\n")
}

/// `sha256:abcd…64` → `sha256:abcd12345678`
fn shorten_digests(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(i) = rest.find("sha256:") {
        out.push_str(&rest[..i + 7]);
        let tail = &rest[i + 7..];
        let hex = tail.chars().take_while(|c| c.is_ascii_hexdigit()).count();
        if hex > 12 {
            out.push_str(&tail[..12]);
            out.push('…');
        } else {
            out.push_str(&tail[..hex]);
        }
        rest = &tail[hex..];
    }
    out.push_str(rest);
    out
}

pub(crate) fn filter_compose(args: &[&str], output: &str) -> String {
    let l = limits();
    match find_subcommand(args) {
        Some("ps") => filter_docker_ps(output),
        Some("logs") => fold_logs(output, l.log_tail),
        Some("config") => cap_lines(collapse_blank(output).lines(), l.list_max_lines, "lines"),
        Some("up") | Some("down") | Some("start") | Some("stop") | Some("restart") => {
            let mut acted = 0usize;
            let mut keep: Vec<String> = Vec::new();
            for line in output.lines() {
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                if t.starts_with('⠿')
                    || t.starts_with('✔')
                    || t.starts_with('-')
                    || t.contains(" Started")
                    || t.contains(" Created")
                    || t.contains(" Running")
                    || t.contains(" Stopped")
                    || t.contains(" Removed")
                    || t.contains(" Healthy")
                {
                    acted += 1;
                    continue;
                }
                keep.push(t.to_string());
            }
            let mut out = cap_vec(keep, l.max_diagnostics, "lines");
            out.push(format!("[compose] {} container steps", acted));
            out.join("\n")
        }
        Some("ls") => squeeze_table(output, l.list_max_lines),
        _ => generic(output),
    }
}

pub(crate) fn filter_docker(args: &[&str], output: &str) -> String {
    let l = limits();
    match find_subcommand(args) {
        Some("ps") => filter_docker_ps(output),
        Some("images") | Some("image") if !args.contains(&"history") => {
            filter_docker_images(output)
        }
        Some("compose") => {
            let pos = args.iter().position(|a| *a == "compose").unwrap_or(0);
            filter_compose(&args[pos + 1..], output)
        }
        Some("logs") | Some("service") => fold_logs(output, l.log_tail),
        Some("version") => docker_version(output),
        Some("info") => docker_info(output),
        Some("inspect") => pruned_json(output, generic),
        Some("build") | Some("buildx") | Some("pull") | Some("push") => docker_build(output),
        Some("network") | Some("volume") => {
            let kind = if find_subcommand(args) == Some("network") {
                "network"
            } else {
                "volume"
            };
            if args.contains(&"inspect") {
                pruned_json(output, generic)
            } else if args.contains(&"ls") {
                docker_ls_resource(output, kind)
            } else {
                generic(output)
            }
        }
        Some("stats") | Some("top") | Some("port") | Some("system") | Some("history") => {
            squeeze_table(output, l.list_max_lines)
        }
        Some("image") => squeeze_table(output, l.list_max_lines), // image history
        _ => generic(output),
    }
}

// ─── kubernetes ───────────────────────────────────────────────────────────────

pub(super) fn kube_describe(output: &str) -> String {
    let l = limits();
    let mut out: Vec<String> = Vec::new();
    let mut dropped = 0usize;
    for line in output.lines() {
        let t = line.trim_end();
        if t.trim().is_empty() {
            continue;
        }
        // `Annotations:  <none>` and empty sections carry nothing
        if t.trim_end().ends_with("<none>") {
            continue;
        }
        if out.len() < l.list_max_lines {
            out.push(squeeze_ws(t));
        } else {
            dropped += 1;
        }
    }
    if dropped > 0 {
        out.push(more(dropped, "lines"));
    }
    out.join("\n")
}

/// glog lines (`E0910 00:01:25.957478  529622 memcache.go:265] …`) repeat the same
/// error once per API probe with a different timestamp and pid, so plain dedupe cannot
/// see them as equal. Fold them by template and return the rest untouched.
pub(super) fn split_glog(output: &str) -> (Vec<String>, String) {
    let is_glog = |t: &str| {
        let b = t.as_bytes();
        b.len() > 5
            && matches!(b[0], b'E' | b'W' | b'I' | b'F')
            && b[1..5].iter().all(|c| c.is_ascii_digit())
    };
    let (glog, rest): (Vec<&str>, Vec<&str>) =
        output.lines().partition(|l| is_glog(l.trim_start()));
    if glog.is_empty() {
        return (Vec::new(), output.to_string());
    }
    let folded = fold_logs(&glog.join("\n"), limits().max_diagnostics)
        .lines()
        .map(|l| truncate(l, 300))
        .collect();
    (folded, rest.join("\n"))
}

pub(crate) fn filter_kubectl(args: &[&str], output: &str) -> String {
    let l = limits();
    let (glog, output) = split_glog(output);
    let output: &str = &output;
    if !glog.is_empty() {
        let mut out = glog;
        let tail = filter_kubectl_body(args, output, l);
        if !tail.trim().is_empty() {
            out.push(tail);
        }
        return out.join("\n");
    }
    filter_kubectl_body(args, output, l)
}

fn filter_kubectl_body(args: &[&str], output: &str, l: &Limits) -> String {
    let json_out = flag_value(args, "-o").map(|v| v == "json").unwrap_or(false)
        || flag_value(args, "--output")
            .map(|v| v == "json")
            .unwrap_or(false);
    match find_subcommand(args) {
        Some("get") if json_out => pruned_json(output, |o| squeeze_table(o, l.list_max_lines)),
        Some("get") | Some("top") => squeeze_table(output, l.list_max_lines),
        Some("describe") => kube_describe(output),
        Some("logs") => fold_logs(output, l.log_tail),
        Some("apply") | Some("delete") | Some("create") | Some("patch") | Some("label") => {
            let (mut out, other) = group_by_verb(
                output,
                &[
                    "created",
                    "configured",
                    "unchanged",
                    "deleted",
                    "labeled",
                    "patched",
                ],
                l.list_max_lines,
            );
            out.extend(cap_vec(other, l.max_diagnostics, "lines"));
            out.join("\n")
        }
        Some("version") | Some("config") => squeeze_table(output, l.passthrough_max_lines),
        _ => generic(output),
    }
}

pub(crate) fn filter_helm(args: &[&str], output: &str) -> String {
    let l = limits();
    match find_subcommand(args) {
        Some("install") | Some("upgrade") | Some("status") => {
            let mut fields: Vec<String> = Vec::new();
            let mut notes = 0usize;
            let mut in_notes = false;
            let mut errors: Vec<String> = Vec::new();
            for line in output.lines() {
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                if t.starts_with("NOTES:") {
                    in_notes = true;
                    continue;
                }
                if in_notes {
                    notes += 1;
                    continue;
                }
                if t.starts_with("Error:") || t.starts_with("error") {
                    errors.push(t.to_string());
                    continue;
                }
                if let Some((k, v)) = t.split_once(':') {
                    if k.chars().all(|c| c.is_ascii_uppercase() || c == ' ') && !v.trim().is_empty()
                    {
                        fields.push(format!("{}={}", k.trim().to_ascii_lowercase(), v.trim()));
                    }
                }
            }
            let mut out = Vec::new();
            if !fields.is_empty() {
                out.push(fields.join(" "));
            }
            out.extend(errors);
            if notes > 0 {
                out.push(more(notes, "NOTES lines"));
            }
            if out.is_empty() {
                return generic(output);
            }
            out.join("\n")
        }
        Some("lint") => {
            let mut keep: Vec<String> = Vec::new();
            for line in output.lines() {
                let t = line.trim();
                if t.contains("[ERROR]") || t.contains("[WARNING]") || t.contains("chart(s) linted")
                {
                    keep.push(squeeze_ws(t));
                }
            }
            if keep.is_empty() {
                return "helm lint: ok".to_string();
            }
            cap_vec(keep, l.max_diagnostics, "lines").join("\n")
        }
        Some("template") | Some("diff") | Some("get") | Some("show") => {
            cap_lines(collapse_blank(output).lines(), l.list_max_lines, "lines")
        }
        Some("test") => {
            let keep: Vec<&str> = output
                .lines()
                .map(|l| l.trim())
                .filter(|t| {
                    t.contains("PASSED")
                        || t.contains("FAILED")
                        || t.starts_with("Phase")
                        || t.starts_with("TEST SUITE")
                })
                .collect();
            if keep.is_empty() {
                generic(output)
            } else {
                keep.join("\n")
            }
        }
        Some("list") | Some("ls") | Some("history") | Some("repo") | Some("search")
        | Some("env") | Some("version") => squeeze_table(output, l.list_max_lines),
        _ => generic(output),
    }
}

pub(crate) fn filter_stern(output: &str) -> String {
    fold_logs(output, limits().log_tail)
}

pub(crate) fn filter_k9s(output: &str) -> String {
    cap_lines(
        collapse_blank(output).lines(),
        limits().list_max_lines,
        "lines",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = "\
CONTAINER ID   IMAGE                  COMMAND                CREATED          STATUS                    PORTS                                         NAMES
ef93185539c3   conman-server:latest   \"/app/conman-server\"   39 minutes ago   Up 39 minutes (healthy)   0.0.0.0:5173->5173/tcp, [::]:5173->5173/tcp   conman-server
62d495a72692   conman-agent:latest    \"/conman-agent\"        5 days ago       Up 4 days                 5073/tcp                                      conman-local-agent
bef31c060979   apache/kafka:4.3.0     \"/__cacert_entrypoin…\" 9 days ago       Exited (143) 6 days ago                                                 local_kafka";

    #[test]
    fn ps_groups_running_and_keeps_ids_names_ports() {
        let out = filter_docker_ps(PS);
        assert!(out.starts_with("[docker] 2 running:"), "{}", out);
        assert!(
            out.contains(
                "ef93185539c3 conman-server (conman-server:latest) Up 39 minutes (healthy) [5173]"
            ),
            "{}",
            out
        );
        assert!(
            out.contains("62d495a72692 conman-local-agent (conman-agent:latest) Up 4 days [5073]"),
            "{}",
            out
        );
        assert!(out.contains("[docker] 1 stopped/exited:"), "{}", out);
        assert!(
            out.contains("bef31c060979 local_kafka (apache/kafka:4.3.0) Exited (143) 6 days ago"),
            "{}",
            out
        );
        // COMMAND and CREATED are dropped
        assert!(!out.contains("cacert_entrypoin"), "{}", out);
        assert!(!out.contains("9 days ago"), "{}", out);
    }

    #[test]
    fn ps_empty_header_only_and_custom_format() {
        assert_eq!(filter_docker_ps(""), "");
        let hdr = "CONTAINER ID   IMAGE   COMMAND   CREATED   STATUS   PORTS   NAMES";
        assert_eq!(filter_docker_ps(hdr), "[docker] no containers");
        let fmt = filter_docker_ps("abc123 nginx up\ndef456 redis up");
        assert!(fmt.contains("abc123 nginx up"), "{}", fmt);
    }

    #[test]
    fn images_totals_and_dangling_fold() {
        let raw = "\
IMAGE                        ID             DISK USAGE   CONTENT SIZE   EXTRA
ai-tester/api:latest         31266f8f94be       3.14GB             0B
alpine:latest                d529dd0c6e55       8.42MB             0B
<none>:<none>                aaaaaaaaaaaa        100MB             0B";
        let out = filter_docker_images(raw);
        assert!(out.starts_with("[docker] 3 images ("), "{}", out);
        assert!(out.contains("  ai-tester/api:latest 3.14GB"), "{}", out);
        assert!(out.contains("  alpine:latest 8.42MB"), "{}", out);
        assert!(out.contains("1 dangling <none> images"), "{}", out);
        assert!(!out.contains("31266f8f94be"), "{}", out);
    }

    #[test]
    fn images_fidelity_cap_announced() {
        let mut raw = String::from("REPOSITORY   TAG   IMAGE ID   CREATED   SIZE\n");
        for i in 0..300 {
            raw.push_str(&format!(
                "repo{}   latest   abc{}   2 days ago   10MB\n",
                i, i
            ));
        }
        let out = filter_docker_images(&raw);
        let shown = out.lines().filter(|l| l.starts_with("  repo")).count();
        let announced: usize = out
            .lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix("[+")?
                    .split_whitespace()
                    .next()?
                    .parse()
                    .ok()
            })
            .unwrap_or(0);
        assert_eq!(shown + announced, 300, "{}", out);
        assert!(has_truncation(&out));
    }

    #[test]
    fn volumes_fold_anonymous_and_networks_fold_builtin() {
        let vols = format!(
            "DRIVER    VOLUME NAME\nlocal     {}\nlocal     {}\nlocal     pgdata",
            "a".repeat(64),
            "b".repeat(64)
        );
        let out = docker_ls_resource(&vols, "volume");
        assert!(out.starts_with("[docker] 3 volumes"), "{}", out);
        assert!(out.contains("pgdata (local)"), "{}", out);
        assert!(out.contains("[+2 more anonymous volumes]"), "{}", out);
        let nets = docker_ls_resource(
            "NETWORK ID     NAME      DRIVER    SCOPE\nabc            bridge    bridge    local\ndef            host      host      local\nghi            mynet     bridge    local",
            "network",
        );
        assert!(nets.contains("builtin: bridge, host"), "{}", nets);
        assert!(nets.contains("mynet (bridge)"), "{}", nets);
    }

    #[test]
    fn logs_fold_repeated_templates_and_keep_errors() {
        let mut raw = String::new();
        for i in 0..200 {
            raw.push_str(&format!(
                "2026/09/08 12:45:0{} \"POST http://x/api/v1/events HTTP/1.1\" from 172.19.0.2:5046{} - 202 22B in 1.39ms\n",
                i % 10, i % 10
            ));
        }
        raw.push_str("2026/09/08 12:50:00 ERROR upstream unavailable\n");
        let out = fold_logs(&raw, 100);
        assert!(out.contains("(×200, 199 similar folded)"), "{}", out);
        assert!(out.contains("ERROR upstream unavailable"), "{}", out);
        assert!(out.lines().count() < 5, "{}", out);
    }

    #[test]
    fn logs_keep_errors_from_the_dropped_head() {
        let mut raw = String::from("early FATAL boom\n");
        for i in 0..50 {
            raw.push_str(&format!(
                "line variant {} alpha\n",
                (b'a' + (i % 26) as u8) as char
            ));
        }
        let out = fold_logs(&raw, 5);
        assert!(out.contains("early FATAL boom"), "{}", out);
        assert!(has_truncation(&out), "{}", out);
    }

    #[test]
    fn inspect_prunes_empties_and_announces() {
        let raw = "[{\"Id\":\"ef93\",\"Name\":\"/conman\",\"State\":{\"Running\":true,\"Paused\":false,\"Error\":\"\"},\"GraphDriver\":{\"Data\":{\"x\":\"y\"}},\"Mounts\":[]}]";
        let out = pruned_json(raw, generic);
        assert!(out.contains("Id: ef93"), "{}", out);
        assert!(out.contains("Running: true"), "{}", out);
        assert!(!out.contains("Paused"), "{}", out);
        assert!(!out.contains("GraphDriver"), "{}", out);
        assert!(has_truncation(&out), "{}", out);
    }

    #[test]
    fn version_and_info_keep_key_facts() {
        let v = docker_version(
            "Client: Docker Engine - Community\n Version:           29.8.0\n API version:       1.56\n Go version:        go1.26.8\n Git commit:        88096ef\n\nServer: Docker Engine - Community\n Engine:\n  Version:          29.8.0\n  API version:      1.56 (minimum version 1.24)",
        );
        assert!(v.contains("29.8.0"), "{}", v);
        assert!(v.contains("api 1.56"), "{}", v);
        let i = docker_info(
            "Client: Docker Engine\n Version: 29.8.0\n Containers: 12\n  Running: 2\n  Paused: 0\n  Stopped: 10\n Images: 92\n Server Version: 29.8.0\n Storage Driver: overlay2\n Plugins:\n  buildx: Docker Buildx\nWARNING: No swap limit support",
        );
        assert!(i.contains("Containers: 12"), "{}", i);
        assert!(i.contains("running: 2"), "{}", i);
        assert!(i.contains("Server Version: 29.8.0"), "{}", i);
        assert!(i.contains("WARNING: No swap limit support"), "{}", i);
        assert!(!i.contains("buildx"), "{}", i);
    }

    #[test]
    fn kubectl_folds_repeated_glog_probe_errors() {
        // kubectl retries the API probe five times and logs the same error each time
        // with a fresh timestamp and pid — one line carries all the information.
        let mut raw = String::new();
        for i in 0..5 {
            raw.push_str(&format!(
                "E0910 00:01:25.95747{}  52962{} memcache.go:265] \"Unhandled Error\" err=\"couldn't get current server API group list: dial tcp 127.0.0.1:8080: connect: connection refused\"\n",
                i, i
            ));
        }
        raw.push_str("The connection to the server localhost:8080 was refused - did you specify the right host or port?\n");
        let out = filter_kubectl(&["get", "pods", "-A"], &raw);
        assert!(out.contains("connection refused"), "{}", out);
        assert!(out.contains("(×5"), "repeats not folded: {}", out);
        assert!(
            out.contains("The connection to the server localhost:8080 was refused"),
            "{}",
            out
        );
        assert!(out.lines().count() <= 3, "still verbose: {}", out);
    }

    #[test]
    fn kubectl_get_squeezes_and_errors_survive() {
        let out = filter_kubectl(
            &["get", "pods", "-A"],
            "NAMESPACE     NAME                       READY   STATUS    RESTARTS   AGE\nkube-system   coredns-abc                1/1     Running   0          5d",
        );
        assert!(
            out.contains("NAMESPACE NAME READY STATUS RESTARTS AGE"),
            "{}",
            out
        );
        assert!(
            out.contains("kube-system coredns-abc 1/1 Running 0 5d"),
            "{}",
            out
        );
        let err = filter_kubectl(&["get", "pods"], "error: current-context is not set");
        assert_eq!(err, "error: current-context is not set");
    }

    #[test]
    fn kubectl_apply_groups_by_verb() {
        let out = filter_kubectl(
            &["apply", "-f", "."],
            "deployment.apps/api created\nservice/api created\nconfigmap/env unchanged",
        );
        assert!(
            out.contains("created (2): deployment.apps/api, service/api"),
            "{}",
            out
        );
        assert!(out.contains("unchanged (1): configmap/env"), "{}", out);
    }

    #[test]
    fn helm_status_folds_notes_and_lint_is_ok() {
        let out = filter_helm(
            &["upgrade", "api", "./chart"],
            "Release \"api\" has been upgraded.\nNAME: api\nLAST DEPLOYED: Mon Sep  8\nNAMESPACE: default\nSTATUS: deployed\nREVISION: 3\nNOTES:\n1. Get the URL:\n  export POD=...\n  echo http://...",
        );
        assert!(out.contains("name=api"), "{}", out);
        assert!(out.contains("status=deployed"), "{}", out);
        assert!(out.contains("revision=3"), "{}", out);
        assert!(has_truncation(&out), "{}", out);
        assert_eq!(
            filter_helm(&["lint", "./chart"], "==> Linting ./chart\n"),
            "helm lint: ok"
        );
    }

    #[test]
    fn compose_up_counts_steps_and_ps_reuses_docker_shape() {
        let up = filter_compose(
            &["up", "-d"],
            " ⠿ Container api-1  Started\n ⠿ Container db-1   Healthy\nError response from daemon: port is allocated",
        );
        assert!(up.contains("Error response from daemon"), "{}", up);
        assert!(up.contains("[compose] 2 container steps"), "{}", up);
        let ps = filter_compose(
            &["ps", "-a"],
            "NAME    IMAGE     COMMAND   SERVICE   CREATED   STATUS    PORTS\napi-1   api:dev   \"run\"     api       2d ago    Up 2 days  0.0.0.0:8080->80/tcp",
        );
        assert!(ps.contains("api-1"), "{}", ps);
        assert!(ps.contains("8080->80"), "{}", ps);
    }

    #[test]
    fn docker_routes_compose_and_unknown_subcommands() {
        let logs = filter_docker(&["logs", "--tail", "100", "x"], "a\na\nb");
        assert!(logs.contains("a  (×2"), "{}", logs);
        let unknown = filter_docker(&["events"], "\n\nx\n\n\ny\n");
        assert_eq!(unknown, "x\n\ny");
    }

    #[test]
    fn digest_shortening_and_build_summary() {
        let out = docker_build(
            "#1 [internal] load build definition\n#5 CACHED\nabc123: Preparing\nsuccessfully pushed sha256:0123456789abcdef0123456789abcdef01234567\n#8 DONE 1.2s",
        );
        assert!(out.contains("sha256:0123456789ab…"), "{}", out);
        assert!(out.contains("[docker] 3 steps, 1 cached"), "{}", out);
    }
}
