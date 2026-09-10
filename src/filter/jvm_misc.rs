//! Ruby, .NET, JVM, PHP, Scala and `make` filters.
//!
//! Every one of these tools prints a long progress narration around a short result;
//! the narration is counted and dropped, failures are kept with their locations.

use super::common::*;

fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{} {}", n, noun)
    } else {
        format!("{} {}s", n, noun)
    }
}

/// Group `path:line[:col]: msg` diagnostics under one header per file.
fn group_diags(tool: &str, diags: Vec<(String, String, String)>, extra: Vec<String>) -> String {
    let l = limits();
    let mut files: Vec<(String, Vec<String>)> = Vec::new();
    for (path, pos, msg) in &diags {
        let entry = format!("  {}  {}", pos, truncate(&squeeze_ws(msg), 200));
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
        out.push(more(dropped, "diagnostics"));
    }
    out.extend(cap_vec(extra, l.max_diagnostics, "lines"));
    if out.is_empty() {
        out.push(format!("{}: ok", tool));
    } else if !diags.is_empty() {
        out.push(format!(
            "{}: {} in {}",
            tool,
            plural(diags.len(), "issue"),
            plural(files.len(), "file")
        ));
    }
    out.join("\n")
}

/// `path:line:col: msg` / `path(line,col): msg` → (path, pos, msg)
fn parse_diag(t: &str) -> Option<(String, String, String)> {
    // MSBuild style: `Program.cs(12,5): error CS1002: ; expected [proj.csproj]`
    if let Some((head, rest)) = t.split_once("): ") {
        if let Some((path, pos)) = head.split_once('(') {
            if pos.chars().all(|c| c.is_ascii_digit() || c == ',') && !pos.is_empty() {
                let msg = rest.split(" [").next().unwrap_or(rest);
                return Some((
                    path.trim().to_string(),
                    pos.replace(',', ":"),
                    msg.trim().to_string(),
                ));
            }
        }
    }
    let mut it = t.splitn(4, ':');
    let path = it.next()?;
    let ln = it.next()?.trim();
    if path.is_empty() || ln.is_empty() || !ln.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let third = it.next().unwrap_or("");
    if third.trim().chars().all(|c| c.is_ascii_digit()) && !third.trim().is_empty() {
        Some((
            path.to_string(),
            format!("{}:{}", ln, third.trim()),
            it.next().unwrap_or("").trim().to_string(),
        ))
    } else {
        let rest = match it.next() {
            Some(r) => format!("{}:{}", third, r),
            None => third.to_string(),
        };
        Some((path.to_string(), ln.to_string(), rest.trim().to_string()))
    }
}

// ─── Ruby ─────────────────────────────────────────────────────────────────────

pub(crate) fn filter_rspec(output: &str) -> String {
    let l = limits();
    let mut fails: Vec<Vec<String>> = Vec::new();
    let mut summary: Option<String> = None;
    let mut duration: Option<String> = None;
    let mut section = "";
    for line in output.lines() {
        let t = strip_ansi(line.trim_end());
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if tt == "Failures:" {
            section = "fail";
            continue;
        }
        if tt.starts_with("Finished in ") {
            duration = Some(tt.to_string());
            section = "";
            continue;
        }
        if tt.contains(" example") && (tt.contains("failure") || tt.contains("failures")) {
            summary = Some(squeeze_ws(tt));
            section = "";
            continue;
        }
        if tt.starts_with("Failed examples:") || tt.starts_with("rspec ./") {
            section = "rerun"; // redundant with the failure list
            continue;
        }
        if section == "fail" {
            // `1) Group does thing`
            if tt
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
                && tt.contains(')')
            {
                fails.push(vec![squeeze_ws(tt)]);
                continue;
            }
            if let Some(cur) = fails.last_mut() {
                let keep = tt.starts_with("Failure/Error")
                    || tt.starts_with("expected")
                    || tt.starts_with("got")
                    || tt.starts_with('#')
                    || tt.contains("Error:");
                if keep && cur.len() < 6 {
                    cur.push(format!("  {}", truncate(&squeeze_ws(tt), 200)));
                }
            }
            continue;
        }
    }
    let mut out: Vec<String> = Vec::new();
    let head = match (&summary, &duration) {
        (Some(s), Some(d)) => format!("rspec: {} ({})", s, d.trim_start_matches("Finished in ")),
        (Some(s), None) => format!("rspec: {}", s),
        _ => format!("rspec: {} failures", fails.len()),
    };
    out.push(head);
    let shown = fails.len().min(l.test_max_failures);
    for f in &fails[..shown] {
        out.extend(f.clone());
    }
    if fails.len() > shown {
        out.push(more(fails.len() - shown, "failures"));
    }
    out.join("\n")
}

pub(crate) fn filter_rubocop(output: &str) -> String {
    let mut diags: Vec<(String, String, String)> = Vec::new();
    let mut extra: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = strip_ansi(line.trim_end());
        let tt = t.trim();
        if tt.is_empty()
            || tt.starts_with("Inspecting ")
            || tt.chars().all(|c| c == '.' || c == 'C' || c == 'W')
        {
            continue;
        }
        if tt.contains("files inspected") {
            extra.push(squeeze_ws(tt));
            continue;
        }
        match parse_diag(tt) {
            Some(d) => diags.push(d),
            None => extra.push(squeeze_ws(tt)),
        }
    }
    group_diags("rubocop", diags, extra)
}

pub(crate) fn filter_rake(output: &str) -> String {
    let l = limits();
    if output.contains("examples,") || output.contains("Failures:") {
        return filter_rspec(output);
    }
    let mut keep: Vec<String> = Vec::new();
    let mut frames = 0usize;
    let mut in_trace = false;
    for line in output.lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if tt == "rake aborted!" {
            in_trace = true;
            keep.push(tt.to_string());
            continue;
        }
        if in_trace {
            frames += 1;
            if frames <= 3 {
                keep.push(format!("  {}", truncate(&squeeze_ws(tt), 200)));
            }
            continue;
        }
        // minitest summary
        if tt.contains(" runs, ") && tt.contains(" assertions") {
            keep.push(squeeze_ws(tt));
            continue;
        }
        keep.push(truncate(&squeeze_ws(tt), 200));
    }
    let mut out = cap_vec(keep, l.max_diagnostics, "lines");
    if frames > 3 {
        out.push(more(frames - 3, "backtrace frames"));
    }
    out.join("\n")
}

// ─── .NET ─────────────────────────────────────────────────────────────────────

pub(crate) fn filter_dotnet_build(output: &str) -> String {
    let mut diags: Vec<(String, String, String)> = Vec::new();
    let mut summary: Vec<String> = Vec::new();
    let mut steps = 0usize;
    for line in output.lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if tt.starts_with("Determining projects to restore")
            || tt.starts_with("Restored ")
            || tt.contains(" -> ")
            || tt.starts_with("MSBuild version")
        {
            steps += 1;
            continue;
        }
        if tt.starts_with("Build succeeded")
            || tt.starts_with("Build FAILED")
            || tt.contains("Time Elapsed")
            || tt.contains("Warning(s)")
            || tt.contains("Error(s)")
        {
            summary.push(squeeze_ws(tt));
            continue;
        }
        if let Some(d) = parse_diag(tt) {
            diags.push(d)
        }
    }
    let mut out = group_diags("dotnet build", diags, Vec::new());
    if !summary.is_empty() {
        out.push('\n');
        out.push_str(&summary.join("; "));
    }
    if steps > 0 {
        out.push_str(&format!("\n({} restore/link steps)", steps));
    }
    out
}

pub(crate) fn filter_dotnet_test(output: &str) -> String {
    let l = limits();
    let mut summary: Option<String> = None;
    let mut fails: Vec<Vec<String>> = Vec::new();
    for line in output.lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if tt.starts_with("Passed!") || tt.starts_with("Failed!") || tt.contains("Total tests:") {
            summary = Some(squeeze_ws(tt));
            continue;
        }
        if let Some(rest) = tt.strip_prefix("Failed ") {
            fails.push(vec![format!("FAIL {}", squeeze_ws(rest))]);
            continue;
        }
        if let Some(cur) = fails.last_mut() {
            let keep = tt.starts_with("Error Message:")
                || tt.starts_with("Assert.")
                || tt.starts_with("Expected:")
                || tt.starts_with("Actual:")
                || tt.starts_with("at ");
            if keep && cur.len() < 6 {
                cur.push(format!("  {}", truncate(&squeeze_ws(tt), 200)));
            }
        }
    }
    let mut out = vec![summary.unwrap_or_else(|| format!("dotnet test: {} failed", fails.len()))];
    let shown = fails.len().min(l.test_max_failures);
    for f in &fails[..shown] {
        out.extend(f.clone());
    }
    if fails.len() > shown {
        out.push(more(fails.len() - shown, "failures"));
    }
    out.join("\n")
}

pub(crate) fn filter_dotnet(args: &[&str], output: &str) -> String {
    match find_subcommand(args) {
        Some("test") | Some("vstest") => filter_dotnet_test(output),
        Some("build") | Some("publish") | Some("pack") | Some("restore") => {
            filter_dotnet_build(output)
        }
        Some("run") => cap_lines(
            collapse_blank(output).lines(),
            limits().passthrough_max_lines,
            "lines",
        ),
        _ => generic(output),
    }
}

// ─── JVM ──────────────────────────────────────────────────────────────────────

pub(crate) fn filter_gradle(args: &[&str], output: &str) -> String {
    let l = limits();
    let mut tasks = 0usize;
    let mut trailer: Option<String> = None;
    let mut duration = String::new();
    let mut fails: Vec<String> = Vec::new();
    let mut diags: Vec<(String, String, String)> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut section = "";
    let is_test = matches!(find_subcommand(args), Some("test") | Some("check"));

    for line in output.lines() {
        let t = strip_ansi(line.trim_end());
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if tt.starts_with("> Task ") {
            tasks += 1;
            continue;
        }
        if tt.starts_with("BUILD SUCCESSFUL") || tt.starts_with("BUILD FAILED") {
            trailer = Some(tt.split(" in ").next().unwrap_or(tt).to_string());
            duration = tt.rsplit(" in ").next().unwrap_or("").to_string();
            continue;
        }
        if tt.contains("actionable task") {
            continue;
        }
        if tt.starts_with("* What went wrong:") {
            section = "why";
            continue;
        }
        if tt.starts_with("* Try:")
            || tt.starts_with("* Get more help")
            || tt.starts_with("* Exception is:")
        {
            section = "";
            continue;
        }
        if section == "why" {
            fails.push(truncate(&squeeze_ws(tt), 200));
            continue;
        }
        if tt.contains(" FAILED") && is_test {
            fails.push(squeeze_ws(tt));
            continue;
        }
        if tt.contains("tests completed") {
            fails.push(squeeze_ws(tt));
            continue;
        }
        if tt.starts_with("warning:") || tt.contains("has been deprecated") {
            warnings.push(squeeze_ws(tt));
            continue;
        }
        if let Some(d) = parse_diag(tt) {
            diags.push(d);
        }
    }
    let mut out: Vec<String> = Vec::new();
    if !diags.is_empty() {
        out.push(group_diags("gradle", diags, Vec::new()));
    }
    out.extend(cap_vec(fails, l.max_diagnostics, "lines"));
    if !warnings.is_empty() {
        out.push(format!("({} deprecation/warning lines)", warnings.len()));
    }
    let head = format!(
        "gradle: {} ({}{} tasks)",
        trailer.unwrap_or_else(|| "done".into()),
        if duration.is_empty() {
            String::new()
        } else {
            format!("{}, ", duration)
        },
        tasks
    );
    out.push(head);
    out.join("\n")
}

pub(crate) fn filter_mvn(output: &str) -> String {
    let l = limits();
    let mut errors: Vec<String> = Vec::new();
    let mut warnings = 0usize;
    let mut tests: Option<String> = None;
    let mut trailer: Vec<String> = Vec::new();
    let mut progress = 0usize;
    for line in output.lines() {
        let t = strip_ansi(line.trim_end());
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if let Some(rest) = tt.strip_prefix("[INFO] ") {
            if rest.starts_with("---")
                || rest.starts_with("Building")
                || rest.starts_with("Downloading")
                || rest.starts_with("Downloaded")
                || rest.starts_with("Installing")
                || rest.chars().all(|c| c == '-')
                || rest.starts_with("Total time")
                || rest.starts_with("Finished at")
                || rest.starts_with("BUILD")
            {
                if rest.starts_with("BUILD") || rest.starts_with("Total time") {
                    trailer.push(rest.to_string());
                } else {
                    progress += 1;
                }
                continue;
            }
            if rest.starts_with("Tests run:") {
                tests = Some(rest.to_string());
                continue;
            }
            progress += 1;
            continue;
        }
        if let Some(rest) = tt.strip_prefix("[ERROR] ") {
            errors.push(truncate(&squeeze_ws(rest), 200));
            continue;
        }
        if tt.starts_with("[WARNING]") {
            warnings += 1;
            continue;
        }
        if tt.starts_with("Tests run:") {
            tests = Some(tt.to_string());
            continue;
        }
        if tt.starts_with("BUILD SUCCESS") || tt.starts_with("BUILD FAILURE") {
            trailer.push(tt.to_string());
        }
    }
    let mut out: Vec<String> = Vec::new();
    out.extend(cap_vec(
        dedupe_consecutive(errors),
        l.max_diagnostics,
        "error lines",
    ));
    if let Some(t) = tests {
        out.push(t);
    }
    if warnings > 0 {
        out.push(format!("({} warnings)", warnings));
    }
    let status = trailer
        .iter()
        .find(|t| t.starts_with("BUILD"))
        .cloned()
        .unwrap_or_else(|| "done".into());
    let time = trailer
        .iter()
        .find(|t| t.starts_with("Total time"))
        .map(|t| t.trim_start_matches("Total time:").trim().to_string())
        .unwrap_or_default();
    out.push(format!(
        "mvn: {}{} ({} steps)",
        status,
        if time.is_empty() {
            String::new()
        } else {
            format!(" in {}", time)
        },
        progress
    ));
    out.join("\n")
}

// ─── make ─────────────────────────────────────────────────────────────────────

const COMPILERS: [&str; 12] = [
    "gcc", "g++", "cc", "c++", "clang", "clang++", "rustc", "cargo", "go", "javac", "ld", "ar",
];

pub(crate) fn filter_make(output: &str) -> String {
    let l = limits();
    let mut diags: Vec<(String, String, String)> = Vec::new();
    let mut keep: Vec<String> = Vec::new();
    let mut steps = 0usize;
    let mut dirs = 0usize;
    for line in output.lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if tt.starts_with("make[")
            && (tt.contains("Entering directory") || tt.contains("Leaving directory"))
        {
            dirs += 1;
            continue;
        }
        if tt.starts_with("make:") && tt.contains("Nothing to be done") {
            keep.push(squeeze_ws(tt));
            continue;
        }
        if tt.contains("Error ") && tt.starts_with("make") {
            keep.push(squeeze_ws(tt));
            continue;
        }
        if let Some(d) = parse_diag(tt) {
            if d.2.starts_with("error") || d.2.starts_with("warning") || d.2.starts_with("note") {
                diags.push(d);
                continue;
            }
        }
        // echoed compile commands are noise unless they carry a diagnostic
        let first = tt.split_whitespace().next().unwrap_or("");
        if COMPILERS
            .iter()
            .any(|c| first == *c || first.ends_with(&format!("/{}", c)))
        {
            steps += 1;
            continue;
        }
        keep.push(truncate(&squeeze_ws(tt), 200));
    }
    let mut out: Vec<String> = Vec::new();
    if !diags.is_empty() {
        out.push(group_diags("make", diags, Vec::new()));
    }
    out.extend(cap_vec(keep, l.max_diagnostics, "lines"));
    let mut note = Vec::new();
    if steps > 0 {
        note.push(format!("{} compile steps", steps));
    }
    if dirs > 0 {
        note.push(format!("{} directory changes", dirs));
    }
    if !note.is_empty() {
        out.push(format!("({})", note.join(", ")));
    }
    if out.is_empty() {
        out.push("make: ok".to_string());
    }
    out.join("\n")
}

// ─── PHP ──────────────────────────────────────────────────────────────────────

pub(crate) fn filter_phpunit(output: &str) -> String {
    let l = limits();
    let mut summary: Option<String> = None;
    let mut fails: Vec<Vec<String>> = Vec::new();
    let mut tests: Vec<String> = Vec::new();
    let mut section = "";
    for line in output.lines() {
        let t = strip_ansi(line.trim_end());
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if tt.starts_with("PHPUnit ")
            || tt.starts_with("Runtime:")
            || tt.starts_with("Configuration:")
        {
            continue;
        }
        if tt.starts_with("OK (") || tt.starts_with("Tests: ") || tt.starts_with("OK, but") {
            summary = Some(squeeze_ws(tt));
            continue;
        }
        if tt == "FAILURES!" || tt == "ERRORS!" || tt.starts_with("There w") {
            section = "fail";
            continue;
        }
        // `--list-tests` output
        if let Some(name) = tt.strip_prefix(" - ").or_else(|| tt.strip_prefix("- ")) {
            tests.push(name.to_string());
            continue;
        }
        if section == "fail" {
            if tt
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
                && tt.contains(')')
            {
                fails.push(vec![squeeze_ws(tt)]);
                continue;
            }
            if let Some(cur) = fails.last_mut() {
                if cur.len() < 6 {
                    cur.push(format!("  {}", truncate(&squeeze_ws(tt), 200)));
                }
            }
            continue;
        }
        // progress dots
        if tt.chars().all(|c| ".FSEIRW%() 0123456789/".contains(c)) {
            continue;
        }
    }
    if !tests.is_empty() {
        // group `Class::method` listings by class
        let mut groups: Vec<(String, Vec<String>)> = Vec::new();
        for name in &tests {
            let (class, method) = match name.rfind("::") {
                Some(i) => (name[..i].to_string(), name[i + 2..].to_string()),
                None => (String::new(), name.clone()),
            };
            match groups.iter_mut().find(|(c, _)| *c == class) {
                Some((_, v)) => v.push(method),
                None => groups.push((class, vec![method])),
            }
        }
        let mut out = vec![format!("{} tests, {} classes", tests.len(), groups.len())];
        for (c, m) in &groups {
            out.push(format!(
                "{}: {}",
                if c.is_empty() { "(root)" } else { c },
                m.join(", ")
            ));
        }
        return cap_vec(out, l.list_max_lines, "classes").join("\n");
    }
    let mut out = vec![
        summary
            .map(|s| format!("phpunit: {}", s))
            .unwrap_or_else(|| format!("phpunit: {} failures", fails.len())),
    ];
    let shown = fails.len().min(l.test_max_failures);
    for f in &fails[..shown] {
        out.extend(f.clone());
    }
    if fails.len() > shown {
        out.push(more(fails.len() - shown, "failures"));
    }
    out.join("\n")
}

pub(crate) fn filter_phpstan(output: &str) -> String {
    let l = limits();
    let mut files: Vec<(String, Vec<String>)> = Vec::new();
    let mut current = String::new();
    let mut trailer: Option<String> = None;
    let mut count = 0usize;
    for line in output.lines() {
        let t = strip_ansi(line.trim_end());
        let tt = t.trim();
        if tt.is_empty() || tt.chars().all(|c| c == '-' || c == ' ' || c == '+') {
            continue;
        }
        if tt.starts_with("[OK]") {
            return "phpstan: ok".to_string();
        }
        if tt.starts_with("[ERROR]") {
            trailer = Some(squeeze_ws(tt.trim_start_matches("[ERROR]").trim()));
            continue;
        }
        // phpstan's table is space-aligned (`  12     message`), pipes only with
        // some formatters — handle both, and fold wrapped continuation rows.
        let cells: Vec<String> = if tt.contains('|') {
            tt.trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect()
        } else {
            match tt.split_once(char::is_whitespace) {
                Some((first, rest)) if !rest.trim().is_empty() => {
                    vec![first.to_string(), rest.trim().to_string()]
                }
                _ => vec![tt.to_string()],
            }
        };
        if cells.len() == 2 {
            if cells[0].eq_ignore_ascii_case("Line") {
                current = cells[1].clone();
                files.push((current.clone(), Vec::new()));
                continue;
            }
            if !cells[0].chars().all(|c| c.is_ascii_digit()) {
                // not a row: a wrapped message continues the previous entry
                if let Some((_, v)) = files.iter_mut().find(|(p, _)| *p == current) {
                    if let Some(last) = v.last_mut() {
                        last.push(' ');
                        last.push_str(&squeeze_ws(tt));
                        continue;
                    }
                }
                continue;
            }
            count += 1;
            let entry = format!("  {}  {}", cells[0], truncate(&squeeze_ws(&cells[1]), 200));
            match files.iter_mut().find(|(p, _)| *p == current) {
                Some((_, v)) => v.push(entry),
                None => files.push((current.clone(), vec![entry])),
            }
            continue;
        }
        if let Some((_, v)) = files.iter_mut().find(|(p, _)| *p == current) {
            if let Some(last) = v.last_mut() {
                last.push(' ');
                last.push_str(&squeeze_ws(tt));
            }
        }
    }

    let mut out: Vec<String> = Vec::new();
    let mut shown = 0usize;
    let mut dropped = 0usize;
    for (path, entries) in &files {
        if entries.is_empty() {
            continue;
        }
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
        out.push(more(dropped, "errors"));
    }
    out.push(trailer.unwrap_or_else(|| format!("phpstan: {}", plural(count, "error"))));
    out.join("\n")
}

pub(crate) fn filter_composer(args: &[&str], output: &str) -> String {
    let l = limits();
    match find_subcommand(args) {
        Some("install") | Some("update") | Some("require") | Some("remove") => {
            let mut ops: Vec<String> = Vec::new();
            let mut abandoned = 0usize;
            let mut noise = 0usize;
            let mut keep: Vec<String> = Vec::new();
            for line in output.lines() {
                let t = strip_ansi(line.trim_end());
                let tt = t.trim();
                if tt.is_empty() {
                    continue;
                }
                if tt.starts_with("Loading composer repositories")
                    || tt.starts_with("Updating dependencies")
                    || tt.starts_with("Lock file operations")
                    || tt.starts_with("Writing lock file")
                    || tt.starts_with("Downloading ")
                    || tt.starts_with("Package operations")
                    || tt.starts_with("Generating autoload")
                    || tt.starts_with("- Downloading")
                {
                    noise += 1;
                    continue;
                }
                if tt.contains("is abandoned") {
                    abandoned += 1;
                    continue;
                }
                if let Some(op) = tt.strip_prefix("- ") {
                    ops.push(squeeze_ws(op));
                    continue;
                }
                keep.push(truncate(&squeeze_ws(tt), 200));
            }
            let mut out = cap_vec(ops, l.list_max_lines, "packages");
            out.extend(cap_vec(keep, l.max_diagnostics, "lines"));
            if abandoned > 0 {
                out.push(format!("({} abandoned packages)", abandoned));
            }
            if noise > 0 {
                out.push(format!("({} resolver steps)", noise));
            }
            out.join("\n")
        }
        Some("show") | Some("outdated") | Some("licenses") => {
            let rows: Vec<String> = output
                .lines()
                .map(|l| strip_ansi(l.trim_end()))
                .filter(|t| !t.trim().is_empty())
                .map(|t| truncate(&squeeze_ws(&t), 200))
                .collect();
            cap_vec(rows, l.list_max_lines, "packages").join("\n")
        }
        Some("validate") => {
            let keep: Vec<String> = output
                .lines()
                .map(|l| strip_ansi(l.trim_end()))
                .filter(|t| !t.trim().is_empty())
                .map(|t| squeeze_ws(&t))
                .collect();
            cap_vec(keep, l.max_diagnostics, "lines").join("\n")
        }
        _ => generic(output),
    }
}

// ─── Scala ────────────────────────────────────────────────────────────────────

pub(crate) fn filter_sbt(output: &str) -> String {
    let l = limits();
    let mut diags: Vec<(String, String, String)> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut tests: Vec<String> = Vec::new();
    let mut warnings = 0usize;
    let mut info = 0usize;
    let mut trailer: Option<String> = None;
    for line in output.lines() {
        let t = strip_ansi(line.trim_end());
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if let Some(rest) = tt.strip_prefix("[success] ") {
            trailer = Some(format!(
                "sbt: ok ({})",
                rest.trim_start_matches("Total time:").trim()
            ));
            continue;
        }
        if let Some(rest) = tt.strip_prefix("[error] ") {
            match parse_diag(rest) {
                Some(d) => diags.push(d),
                None => errors.push(truncate(&squeeze_ws(rest), 200)),
            }
            continue;
        }
        if tt.starts_with("[warn]") {
            warnings += 1;
            continue;
        }
        if let Some(rest) = tt.strip_prefix("[info] ") {
            if rest.contains("Passed: Total")
                || rest.contains("*** FAILED ***")
                || rest.starts_with("- ")
            {
                tests.push(squeeze_ws(rest));
            } else {
                info += 1;
            }
            continue;
        }
    }
    let mut out: Vec<String> = Vec::new();
    if !diags.is_empty() {
        out.push(group_diags("sbt", diags, Vec::new()));
    }
    out.extend(cap_vec(errors, l.max_diagnostics, "error lines"));
    out.extend(cap_vec(tests, l.max_diagnostics, "test lines"));
    if warnings > 0 {
        out.push(format!("({} warnings)", warnings));
    }
    out.push(trailer.unwrap_or_else(|| format!("sbt: done ({} info lines)", info)));
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rspec_keeps_failures_and_summary() {
        let out = filter_rspec(
            "....F..\n\nFailures:\n\n  1) Calculator#add returns the sum\n     Failure/Error: expect(add(1, 2)).to eq 4\n\n       expected: 4\n            got: 3\n\n       (compared using ==)\n     # ./spec/calc_spec.rs:12:in `block (3 levels)'\n\nFinished in 0.0123 seconds (files took 0.1 seconds to load)\n7 examples, 1 failure\n\nFailed examples:\n\nrspec ./spec/calc_spec.rb:10 # Calculator#add returns the sum",
        );
        assert!(
            out.starts_with("rspec: 7 examples, 1 failure (0.0123 seconds"),
            "{}",
            out
        );
        assert!(out.contains("1) Calculator#add returns the sum"), "{}", out);
        assert!(
            out.contains("Failure/Error: expect(add(1, 2)).to eq 4"),
            "{}",
            out
        );
        assert!(out.contains("expected: 4"), "{}", out);
        assert!(!out.contains("Failed examples:"), "{}", out);
    }

    #[test]
    fn rubocop_groups_offenses() {
        let out = filter_rubocop(
            "Inspecting 3 files\n.C.\n\napp/models/user.rb:12:5: C: Style/StringLiterals: Prefer single-quoted strings\napp/models/user.rb:20:1: W: Lint/UselessAssignment: Useless assignment to `x`\n\n3 files inspected, 2 offenses detected, 1 offense autocorrectable",
        );
        assert!(
            out.contains("app/models/user.rb\n  12:5  C: Style/StringLiterals"),
            "{}",
            out
        );
        assert!(out.contains("  20:1  W: Lint/UselessAssignment"), "{}", out);
        assert!(
            out.contains("3 files inspected, 2 offenses detected"),
            "{}",
            out
        );
        assert_eq!(filter_rubocop("Inspecting 2 files\n..\n"), "rubocop: ok");
    }

    #[test]
    fn dotnet_build_groups_and_test_summarises() {
        let build = filter_dotnet(
            &["build"],
            "MSBuild version 17.8.3\n  Determining projects to restore...\n  Restored /app/App.csproj (in 1.2 sec).\nProgram.cs(12,5): error CS1002: ; expected [/app/App.csproj]\nProgram.cs(20,9): warning CS0168: variable declared but never used [/app/App.csproj]\n  App -> /app/bin/Debug/App.dll\n\nBuild FAILED.\n    1 Warning(s)\n    1 Error(s)\nTime Elapsed 00:00:03.21",
        );
        assert!(
            build.contains("Program.cs\n  12:5  error CS1002: ; expected"),
            "{}",
            build
        );
        assert!(build.contains("  20:9  warning CS0168"), "{}", build);
        assert!(build.contains("Build FAILED"), "{}", build);
        assert!(build.contains("restore/link steps"), "{}", build);
        assert!(!build.contains("App.csproj]"), "{}", build);

        let test = filter_dotnet(
            &["test"],
            "  Determining projects to restore...\nFailed Calc.AddTest [12 ms]\n  Error Message:\n   Assert.Equal() Failure\n   Expected: 4\n   Actual:   3\n  Stack Trace:\n     at Calc.AddTest() in /app/Tests.cs:line 12\n\nFailed!  - Failed: 1, Passed: 11, Skipped: 0, Total: 12, Duration: 1 s",
        );
        assert!(
            test.starts_with("Failed! - Failed: 1, Passed: 11"),
            "{}",
            test
        );
        assert!(test.contains("FAIL Calc.AddTest [12 ms]"), "{}", test);
        assert!(test.contains("Error Message:"), "{}", test);
    }

    #[test]
    fn gradle_counts_tasks_and_keeps_what_went_wrong() {
        let out = filter_gradle(
            &["build"],
            "> Task :compileJava\n> Task :processResources NO-SOURCE\n> Task :classes\n\nFAILURE: Build failed with an exception.\n\n* What went wrong:\nExecution failed for task ':test'.\n> There were failing tests. See the report at: file:///app/report.html\n\n* Try:\n> Run with --stacktrace option\n\n* Get more help at https://help.gradle.org\n\nBUILD FAILED in 12s\n3 actionable tasks: 3 executed",
        );
        assert!(out.contains("Execution failed for task ':test'"), "{}", out);
        assert!(out.contains("There were failing tests"), "{}", out);
        assert!(
            out.contains("gradle: BUILD FAILED (12s, 3 tasks)"),
            "{}",
            out
        );
        assert!(!out.contains("--stacktrace"), "{}", out);
        assert!(!out.contains("help.gradle.org"), "{}", out);
    }

    #[test]
    fn mvn_dedupes_errors_and_keeps_test_totals() {
        let out = filter_mvn(
            "[INFO] Scanning for projects...\n[INFO] ------------------------------------------------------------------------\n[INFO] Building app 1.0\n[INFO] Downloading from central: https://repo/x.jar\n[INFO] Downloaded from central: https://repo/x.jar\n[WARNING] Using platform encoding\n[WARNING] Using platform encoding\n[INFO] Tests run: 12, Failures: 1, Errors: 0, Skipped: 0\n[ERROR] testAdd(CalcTest)  Time elapsed: 0.01 s  <<< FAILURE!\n[ERROR] expected:<4> but was:<3>\n[ERROR] expected:<4> but was:<3>\n[INFO] BUILD FAILURE\n[INFO] Total time:  4.512 s",
        );
        assert!(out.contains("Tests run: 12, Failures: 1"), "{}", out);
        assert!(out.contains("testAdd(CalcTest)"), "{}", out);
        assert!(out.contains("expected:<4> but was:<3>  (×2)"), "{}", out);
        assert!(out.contains("(2 warnings)"), "{}", out);
        assert!(out.contains("mvn: BUILD FAILURE in 4.512 s"), "{}", out);
        assert!(!out.contains("Downloading"), "{}", out);
    }

    #[test]
    fn make_counts_compile_steps_and_keeps_diagnostics() {
        let out = filter_make(
            "make[1]: Entering directory '/app/src'\ngcc -c -O2 -o main.o main.c\ngcc -c -O2 -o util.o util.c\nmain.c:12:5: error: 'x' undeclared (first use in this function)\nmake[1]: *** [Makefile:20: main.o] Error 1\nmake[1]: Leaving directory '/app/src'",
        );
        assert!(
            out.contains("main.c\n  12:5  error: 'x' undeclared"),
            "{}",
            out
        );
        assert!(out.contains("Error 1"), "{}", out);
        assert!(out.contains("2 compile steps"), "{}", out);
        assert!(out.contains("2 directory changes"), "{}", out);
        assert_eq!(filter_make(""), "make: ok");
        let dry = filter_make("cd src && cargo build --release\nstrip target/release/prism");
        assert!(dry.contains("cd src && cargo build --release"), "{}", dry);
    }

    #[test]
    fn phpunit_summary_failures_and_listing() {
        let ok = filter_phpunit(
            "PHPUnit 10.5.0 by Sebastian Bergmann\n\nRuntime:       PHP 8.3.0\n...............                                             15 / 15 (100%)\n\nTime: 00:00.123, Memory: 6.00 MB\n\nOK (15 tests, 32 assertions)",
        );
        assert_eq!(ok, "phpunit: OK (15 tests, 32 assertions)");
        let fail = filter_phpunit(
            "PHPUnit 10.5.0\n\n..F\n\nThere was 1 failure:\n\n1) CalcTest::testAdd\nFailed asserting that 3 matches expected 4.\n\n/app/tests/CalcTest.php:12\n\nFAILURES!\nTests: 3, Assertions: 3, Failures: 1.",
        );
        assert!(
            fail.contains("Tests: 3, Assertions: 3, Failures: 1"),
            "{}",
            fail
        );
        assert!(fail.contains("1) CalcTest::testAdd"), "{}", fail);
        assert!(
            fail.contains("Failed asserting that 3 matches expected 4"),
            "{}",
            fail
        );
        let list = filter_phpunit(
            "PHPUnit 10.5.0\n\nAvailable test(s):\n - CalcTest::testAdd\n - CalcTest::testSub\n - UserTest::testName",
        );
        assert!(list.starts_with("3 tests, 2 classes"), "{}", list);
        assert!(list.contains("CalcTest: testAdd, testSub"), "{}", list);
    }

    #[test]
    fn phpstan_table_becomes_grouped_lines() {
        let out = filter_phpstan(
            " ------ ----------------------------------------------------------------- \n  Line   app/Models/User.php                                              \n ------ ----------------------------------------------------------------- \n  12     Method App\\Models\\User::save() has no return type specified.     \n  40     Property $name has no type specified.                            \n ------ ----------------------------------------------------------------- \n\n [ERROR] Found 2 errors\n",
        );
        assert!(out.contains("app/Models/User.php"), "{}", out);
        assert!(
            out.contains("  12  Method App\\Models\\User::save() has no return type specified."),
            "{}",
            out
        );
        assert!(
            out.contains("  40  Property $name has no type specified."),
            "{}",
            out
        );
        assert!(out.ends_with("Found 2 errors"), "{}", out);
        assert!(!out.contains("------"), "{}", out);
        assert_eq!(filter_phpstan(" [OK] No errors\n"), "phpstan: ok");
    }

    #[test]
    fn composer_keeps_operations_drops_resolver_noise() {
        let out = filter_composer(
            &["install"],
            "Loading composer repositories with package information\nUpdating dependencies\nLock file operations: 2 installs, 0 updates, 0 removals\n  - Locking monolog/monolog (3.5.0)\nPackage operations: 2 installs, 0 updates, 0 removals\n  - Downloading monolog/monolog (3.5.0)\n  - Installing monolog/monolog (3.5.0): Extracting archive\nPackage foo/bar is abandoned, you should avoid using it.\nGenerating autoload files",
        );
        assert!(
            out.contains("Installing monolog/monolog (3.5.0)"),
            "{}",
            out
        );
        assert!(out.contains("(1 abandoned packages)"), "{}", out);
        assert!(out.contains("resolver steps"), "{}", out);
        assert!(!out.contains("Loading composer repositories"), "{}", out);
    }

    #[test]
    fn sbt_groups_compile_errors_and_success() {
        let out = filter_sbt(
            "[info] welcome to sbt 1.9.7\n[info] loading project definition\n[info] compiling 3 Scala sources\n[error] /app/src/Main.scala:12:5: not found: value foo\n[error] one error found\n[warn] deprecated method\n[error] (Compile / compileIncremental) Compilation failed",
        );
        assert!(
            out.contains("/app/src/Main.scala\n  12:5  not found: value foo"),
            "{}",
            out
        );
        assert!(out.contains("one error found"), "{}", out);
        assert!(out.contains("(1 warnings)"), "{}", out);
        let ok = filter_sbt("[info] compiling\n[success] Total time: 4 s, completed Sep 9, 2026");
        assert!(ok.contains("sbt: ok (4 s"), "{}", ok);
    }

    #[test]
    fn failure_caps_are_announced_and_empty_is_safe() {
        let mut raw = String::from("Failures:\n\n");
        for i in 0..40 {
            raw.push_str(&format!(
                "  {}) Group case {}\n     Failure/Error: boom\n\n",
                i + 1,
                i
            ));
        }
        raw.push_str("40 examples, 40 failures\n");
        let out = filter_rspec(&raw);
        assert!(has_truncation(&out), "{}", out);
        assert_eq!(filter_rspec(""), "rspec: 0 failures");
        assert_eq!(filter_phpstan(""), "phpstan: 0 errors");
        assert_eq!(filter_composer(&["install"], ""), "");
    }
}
