//! Python toolchain filters: `pytest`, type checkers, linters, formatters, `pip`,
//! `uv`, `poetry`.
//!
//! Test sessions collapse to counts plus the failing assertions; diagnostics group
//! under one line per file. The session header (platform/rootdir/plugins) is noise to
//! an agent and is dropped.

use super::common::*;

fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{} {}", n, noun)
    } else {
        format!("{} {}s", n, noun)
    }
}

// ─── diagnostics shared by mypy / pyright / ruff / flake8 ─────────────────────

/// Split `path:line[:col]: message` — the tail after the last numeric segment.
fn split_diag(line: &str) -> Option<(String, String, String)> {
    let mut parts = line.splitn(4, ':');
    let path = parts.next()?;
    let l1 = parts.next()?.trim();
    if path.is_empty() || !l1.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let third = parts.next()?;
    let (pos, msg) = if third.trim().chars().all(|c| c.is_ascii_digit()) && !third.trim().is_empty()
    {
        (
            format!("{}:{}", l1, third.trim()),
            parts.next().unwrap_or("").trim().to_string(),
        )
    } else {
        let rest = match parts.next() {
            Some(r) => format!("{}:{}", third, r),
            None => third.to_string(),
        };
        (l1.to_string(), rest.trim().to_string())
    };
    Some((path.to_string(), pos, msg))
}

/// Group `path:line:col: msg` diagnostics under one header per file.
fn group_diags(tool: &str, output: &str, is_summary: impl Fn(&str) -> bool) -> String {
    let l = limits();
    let mut files: Vec<(String, Vec<String>)> = Vec::new();
    let mut summaries: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    let mut count = 0usize;
    for line in output.lines() {
        let t = line.trim_end();
        if t.trim().is_empty() {
            continue;
        }
        if is_summary(t.trim()) {
            summaries.push(squeeze_ws(t));
            continue;
        }
        match split_diag(t) {
            Some((path, pos, msg)) => {
                count += 1;
                let entry = format!("  {}  {}", pos, truncate(&squeeze_ws(&msg), 200));
                match files.iter_mut().find(|(p, _)| *p == path) {
                    Some((_, v)) => v.push(entry),
                    None => files.push((path, vec![entry])),
                }
            }
            None => other.push(squeeze_ws(t)),
        }
    }
    if files.is_empty() {
        let mut out = summaries;
        out.extend(cap_vec(other, l.max_diagnostics, "lines"));
        if out.is_empty() {
            out.push(format!("{}: ok", tool));
        }
        return out.join("\n");
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
    if summaries.is_empty() {
        out.push(format!(
            "{}: {} in {}",
            tool,
            plural(count, "issue"),
            plural(files.len(), "file")
        ));
    } else {
        out.extend(summaries);
    }
    out.join("\n")
}

// ─── pytest ───────────────────────────────────────────────────────────────────

const PYTEST_HEADER: [&str; 10] = [
    "platform ",
    "cachedir:",
    "rootdir:",
    "plugins:",
    "asyncio:",
    "collecting",
    "collected ",
    "configfile:",
    "testpaths:",
    "benchmark:",
];

/// `==== 3 failed, 8 passed, 1 skipped in 0.12s ====` → counts + duration
fn pytest_summary(line: &str) -> Option<String> {
    let inner = line.trim().trim_matches('=').trim();
    if !(inner.contains(" in ") || inner.ends_with('s')) {
        return None;
    }
    if ![
        "passed",
        "failed",
        "error",
        "skipped",
        "no tests ran",
        "xfailed",
        "xpassed",
    ]
    .iter()
    .any(|k| inner.contains(k))
    {
        return None;
    }
    Some(format!("pytest: {}", inner))
}

pub(crate) fn filter_pytest(_args: &[&str], output: &str) -> String {
    let l = limits();
    if output.trim().is_empty() {
        return String::new();
    }
    // pytest could not even start (missing binary, permission denied, import error)
    if !output.contains("test session starts")
        && !output.contains("test result")
        && output.lines().count() < 6
    {
        return collapse_blank(output);
    }

    struct Fail {
        name: String,
        lines: Vec<String>,
    }
    let mut fails: Vec<Fail> = Vec::new();
    let mut summary: Option<String> = None;
    let mut per_file: Vec<(String, [usize; 4])> = Vec::new(); // passed, failed, skipped, error
    let mut errors: Vec<String> = Vec::new();
    let mut warn_count = 0usize;
    let mut in_section = ""; // "failures" | "errors" | "warnings" | "shortsummary"

    let bump = |per_file: &mut Vec<(String, [usize; 4])>, file: &str, slot: usize| match per_file
        .iter_mut()
        .find(|(f, _)| f == file)
    {
        Some((_, c)) => c[slot] += 1,
        None => {
            let mut c = [0usize; 4];
            c[slot] = 1;
            per_file.push((file.to_string(), c));
        }
    };

    for line in output.lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        // section banners
        if tt.starts_with('=') && tt.ends_with('=') {
            let title = tt.trim_matches('=').trim().to_ascii_lowercase();
            if let Some(s) = pytest_summary(tt) {
                summary = Some(s);
                continue;
            }
            in_section = if title.starts_with("failures") {
                "failures"
            } else if title.starts_with("errors") {
                "errors"
            } else if title.starts_with("warnings") {
                "warnings"
            } else if title.starts_with("short test summary") {
                "shortsummary"
            } else if title.starts_with("test session starts") {
                ""
            } else {
                in_section
            };
            continue;
        }
        if PYTEST_HEADER.iter().any(|h| tt.starts_with(h)) {
            continue;
        }
        // `-v` result lines: `test_a.py::test_x PASSED [ 10%]` (and `-rA`'s
        // `PASSED test_a.py::test_x`). Only in the progress region — the short
        // summary repeats the same verdicts and would double-count them.
        if in_section.is_empty() {
            let toks: Vec<&str> = tt.split_whitespace().collect();
            let verdict = toks
                .iter()
                .find(|t| {
                    matches!(
                        **t,
                        "PASSED" | "FAILED" | "SKIPPED" | "ERROR" | "XFAIL" | "XPASS"
                    )
                })
                .copied();
            let id = toks.iter().find(|t| t.contains("::")).copied();
            if let (Some(v), Some(id)) = (verdict, id) {
                let file = id.split("::").next().unwrap_or(id);
                let slot = match v {
                    "PASSED" | "XPASS" => 0,
                    "FAILED" => 1,
                    "SKIPPED" | "XFAIL" => 2,
                    _ => 3,
                };
                bump(&mut per_file, file, slot);
                if slot == 1 && !fails.iter().any(|f| f.name == id) {
                    // remember the failing id even if no traceback follows
                    fails.push(Fail {
                        name: id.to_string(),
                        lines: Vec::new(),
                    });
                }
                continue;
            }
        }
        // progress lines: `test_a.py ..FFs   [ 46%]`
        if tt.contains(".py ")
            && tt.ends_with(']')
            && tt
                .split_whitespace()
                .nth(1)
                .map(|p| p.chars().all(|c| ".FEsxX".contains(c)))
                .unwrap_or(false)
        {
            continue;
        }
        match in_section {
            "failures" | "errors" => {
                // `_______ test_name _______`
                if tt.starts_with('_') && tt.ends_with('_') {
                    let name = tt.trim_matches('_').trim();
                    if !name.is_empty() {
                        fails.push(Fail {
                            name: name.to_string(),
                            lines: Vec::new(),
                        });
                    }
                    continue;
                }
                if let Some(f) = fails.last_mut() {
                    // Assertion detail and the *user's* frames carry the signal; pytest's
                    // own machinery frames (site-packages/_pytest/pluggy) never do.
                    let vendored = tt.contains("site-packages")
                        || tt.contains("/_pytest/")
                        || tt.contains("/pluggy/");
                    let is_frame = tt.starts_with("File \"")
                        || (tt.contains(".py:")
                            && tt
                                .split(':')
                                .nth(1)
                                .map(|n| n.trim().chars().all(|c| c.is_ascii_digit()))
                                .unwrap_or(false));
                    let keep = !vendored && (tt.starts_with('E') || is_frame);
                    if keep && f.lines.len() < 4 {
                        f.lines.push(truncate(&squeeze_ws(tt), 200));
                    }
                }
            }
            "warnings" => {
                if tt.contains("Warning") {
                    warn_count += 1;
                }
            }
            "shortsummary" => {
                if tt.starts_with("ERROR") {
                    errors.push(squeeze_ws(tt));
                }
            }
            _ => {
                if tt.starts_with("ERROR ") || tt.starts_with("INTERNALERROR") {
                    errors.push(squeeze_ws(tt));
                }
            }
        }
    }

    let mut out: Vec<String> = Vec::new();
    out.push(summary.unwrap_or_else(|| {
        let p: usize = per_file.iter().map(|(_, c)| c[0]).sum();
        let f: usize = per_file.iter().map(|(_, c)| c[1]).sum();
        format!("pytest: {} passed, {} failed", p, f)
    }));
    // per-file breakdown only adds value in -v mode with several files
    if per_file.len() > 1 {
        for (file, c) in &per_file {
            let mut bits = vec![format!("{} passed", c[0])];
            if c[1] > 0 {
                bits.push(format!("{} failed", c[1]))
            }
            if c[2] > 0 {
                bits.push(format!("{} skipped", c[2]))
            }
            if c[3] > 0 {
                bits.push(format!("{} error", c[3]))
            }
            out.push(format!("{}: {}", file, bits.join(", ")));
        }
    }
    let shown = fails.len().min(l.test_max_failures);
    for f in &fails[..shown] {
        out.push(format!("FAIL {}", f.name));
        for line in &f.lines {
            out.push(format!("  {}", line));
        }
    }
    if fails.len() > shown {
        out.push(more(fails.len() - shown, "failures"));
    }
    out.extend(cap_vec(errors, l.max_diagnostics, "errors"));
    if warn_count > 0 {
        out.push(format!("{} warnings", warn_count));
    }
    out.join("\n")
}

// ─── linters / type checkers / formatters ─────────────────────────────────────

pub(crate) fn filter_mypy(output: &str) -> String {
    group_diags("mypy", output, |t| {
        t.starts_with("Found ") || t.starts_with("Success:")
    })
}

pub(crate) fn filter_pyright(output: &str) -> String {
    group_diags("pyright", output, |t| {
        t.contains(" error") && t.contains(" warning") && t.contains("information")
    })
}

pub(crate) fn filter_ruff(output: &str) -> String {
    group_diags("ruff", output, |t| {
        t.starts_with("Found ") || t.starts_with("All checks passed")
    })
}

pub(crate) fn filter_flake8(output: &str) -> String {
    group_diags("flake8", output, |_| false)
}

pub(crate) fn filter_bandit(output: &str) -> String {
    let l = limits();
    let mut issues: Vec<String> = Vec::new();
    let mut totals: Vec<String> = Vec::new();
    let mut cur: Option<String> = None;
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(rest) = t.strip_prefix(">> Issue: ") {
            if let Some(c) = cur.take() {
                issues.push(c);
            }
            cur = Some(rest.to_string());
            continue;
        }
        if t.starts_with("Severity:") || t.starts_with("Location:") {
            if let Some(c) = cur.as_mut() {
                c.push_str(&format!("  {}", squeeze_ws(t)));
            }
            continue;
        }
        if t.starts_with("Total issues") || t.starts_with("Total lines") || t.contains("High: ") {
            totals.push(squeeze_ws(t));
        }
    }
    if let Some(c) = cur {
        issues.push(c);
    }
    let mut out = cap_vec(issues, l.max_diagnostics, "issues");
    out.extend(totals);
    if out.is_empty() {
        out.push("bandit: ok".to_string());
    }
    out.join("\n")
}

/// `black`/`isort`: list the files that would change, count the untouched ones.
fn format_tool(tool: &str, output: &str) -> String {
    let l = limits();
    let mut changed: Vec<String> = Vec::new();
    let mut unchanged = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(p) = t
            .strip_prefix("would reformat ")
            .or_else(|| t.strip_prefix("reformatted "))
        {
            changed.push(p.to_string());
            continue;
        }
        if let Some(p) = t.strip_prefix("Fixing ") {
            changed.push(p.to_string());
            continue;
        }
        if t.starts_with("ERROR") || t.starts_with("error:") || t.starts_with("Oh no!") {
            errors.push(squeeze_ws(t));
            continue;
        }
        if t.contains("left unchanged")
            || t.contains("files would be left unchanged")
            || t.starts_with("Skipped")
        {
            if let Some(n) = t
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<usize>().ok())
            {
                unchanged += n;
            }
            continue;
        }
    }
    let mut out = Vec::new();
    if !changed.is_empty() {
        out.push(format!(
            "{}: {} to reformat:",
            tool,
            plural(changed.len(), "file")
        ));
        out.extend(cap_vec(changed, l.list_max_lines, "files"));
    }
    out.extend(errors);
    if out.is_empty() {
        out.push(format!("{}: ok ({} unchanged)", tool, unchanged));
    } else if unchanged > 0 {
        out.push(format!("{} unchanged", unchanged));
    }
    out.join("\n")
}

pub(crate) fn filter_black(output: &str) -> String {
    format_tool("black", output)
}

pub(crate) fn filter_isort(output: &str) -> String {
    format_tool("isort", output)
}

// ─── package managers ─────────────────────────────────────────────────────────

const PIP_NOISE: [&str; 8] = [
    "Collecting ",
    "Downloading ",
    "Using cached ",
    "Requirement already satisfied",
    "Preparing metadata",
    "Building wheel",
    "Created wheel",
    "Stored in directory",
];

fn pip_list(output: &str) -> String {
    let l = limits();
    let mut rows: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("---") || t.starts_with("Package ") {
            continue;
        }
        rows.push(squeeze_ws(t));
    }
    let n = rows.len();
    let mut out = cap_vec(rows, l.list_max_lines, "packages");
    out.push(format!("pip: {}", plural(n, "package")));
    out.join("\n")
}

fn pip_install(output: &str) -> String {
    let l = limits();
    let mut keep: Vec<String> = Vec::new();
    let mut noise = 0usize;
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if PIP_NOISE.iter().any(|n| t.starts_with(n)) || t.starts_with("[notice]") {
            noise += 1;
            continue;
        }
        if t.starts_with("Installing collected packages") {
            keep.push(truncate(t, 200));
            continue;
        }
        keep.push(squeeze_ws(t));
    }
    let mut out = cap_vec(keep, l.max_diagnostics, "lines");
    if noise > 0 {
        out.push(format!("({} download/build steps)", noise));
    }
    out.join("\n")
}

pub(crate) fn filter_pip(args: &[&str], output: &str) -> String {
    match find_subcommand(args) {
        Some("list") | Some("freeze") => pip_list(output),
        Some("install") | Some("uninstall") | Some("download") => pip_install(output),
        Some("show") => {
            let keep: Vec<String> = output
                .lines()
                .map(|l| l.trim())
                .filter(|t| {
                    !t.is_empty()
                        && t.split_once(':')
                            .map(|(_, v)| !v.trim().is_empty())
                            .unwrap_or(true)
                })
                .map(String::from)
                .collect();
            cap_vec(keep, limits().list_max_lines, "lines").join("\n")
        }
        _ => generic(output),
    }
}

pub(crate) fn filter_uv(args: &[&str], output: &str) -> String {
    let l = limits();
    match find_subcommand(args) {
        Some("pip") => {
            let rest: Vec<&str> = args
                .iter()
                .copied()
                .skip_while(|a| *a != "pip")
                .skip(1)
                .collect();
            filter_pip(&rest, output)
        }
        Some("sync") | Some("add") | Some("remove") | Some("install") | Some("lock")
        | Some("venv") => {
            let mut keep: Vec<String> = Vec::new();
            let mut progress = 0usize;
            for line in output.lines() {
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                if t.starts_with("Resolved ")
                    || t.starts_with("Prepared ")
                    || t.starts_with("Downloading")
                    || t.starts_with("Building ")
                    || t.starts_with("Built ")
                    || t.starts_with("Audited ")
                {
                    progress += 1;
                    continue;
                }
                keep.push(squeeze_ws(t));
            }
            let mut out = cap_vec(keep, l.max_diagnostics, "lines");
            if out.is_empty() {
                out.push(format!("uv: ok ({} steps)", progress));
            }
            out.join("\n")
        }
        Some("run") => cap_lines(
            collapse_blank(output).lines(),
            l.passthrough_max_lines,
            "lines",
        ),
        _ => generic(output),
    }
}

pub(crate) fn filter_poetry(args: &[&str], output: &str) -> String {
    let l = limits();
    match find_subcommand(args) {
        Some("install") | Some("add") | Some("remove") | Some("update") | Some("lock") => {
            let mut keep: Vec<String> = Vec::new();
            let mut steps = 0usize;
            for line in output.lines() {
                let t = strip_ansi(line.trim()).trim().to_string();
                if t.is_empty() {
                    continue;
                }
                if t.starts_with("Installing ")
                    || t.starts_with("Updating ")
                    || t.starts_with("Removing ")
                    || t.starts_with("Downgrading ")
                {
                    steps += 1;
                    keep.push(t);
                    continue;
                }
                if t.starts_with("Resolving dependencies")
                    || t.starts_with("Writing lock file")
                    || t.starts_with("Package operations")
                    || t.starts_with("Creating virtualenv")
                {
                    continue;
                }
                keep.push(t);
            }
            let mut out = cap_vec(keep, l.max_diagnostics, "lines");
            if out.is_empty() {
                out.push(format!("poetry: ok ({} steps)", steps));
            }
            out.join("\n")
        }
        Some("run") | Some("build") | Some("publish") => cap_lines(
            collapse_blank(output).lines(),
            l.passthrough_max_lines,
            "lines",
        ),
        Some("show") => pip_list(output),
        _ => generic(output),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PYTEST_Q: &str = "\
============================= test session starts ==============================
platform linux -- Python 3.12.3, pytest-9.0.3, pluggy-1.6.0
rootdir: /tmp/bench/pyproj
plugins: anyio-4.13.0, Faker-40.15.0
collected 13 items

test_a.py ..FF..s                                                        [ 53%]
test_b.py .F.                                                            [100%]

=================================== FAILURES ===================================
________________________________ test_div_fail _________________________________

    def test_div_fail():
        total = 10
        parts = 0
>       assert total / max(parts, 1) == 5
E       assert (10 / 1) == 5
E        +  where 1 = max(0, 1)

test_a.py:12: AssertionError
________________________________ test_str_fail _________________________________

    def test_str_fail():
>       assert \"prism\".upper() == \"PRISN\"
E       AssertionError: assert 'PRISM' == 'PRISN'

test_a.py:20: AssertionError
=========================== short test summary info ============================
FAILED test_a.py::test_div_fail - assert (10 / 1) == 5
FAILED test_a.py::test_str_fail - AssertionError
=================== 3 failed, 9 passed, 1 skipped in 0.12s ====================";

    #[test]
    fn pytest_keeps_summary_and_assertions_drops_header() {
        let out = filter_pytest(&["-q"], PYTEST_Q);
        assert!(
            out.starts_with("pytest: 3 failed, 9 passed, 1 skipped in 0.12s"),
            "{}",
            out
        );
        assert!(out.contains("FAIL test_div_fail"), "{}", out);
        assert!(out.contains("E assert (10 / 1) == 5"), "{}", out);
        assert!(out.contains("test_a.py:12: AssertionError"), "{}", out);
        assert!(out.contains("FAIL test_str_fail"), "{}", out);
        assert!(
            out.contains("AssertionError: assert 'PRISM' == 'PRISN'"),
            "{}",
            out
        );
        // header, plugins, progress lines are gone
        assert!(!out.contains("platform linux"), "{}", out);
        assert!(!out.contains("rootdir"), "{}", out);
        assert!(!out.contains("[ 53%]"), "{}", out);
        assert!(!out.contains("def test_div_fail"), "{}", out);
    }

    #[test]
    fn pytest_verbose_groups_per_file() {
        let v = "\
============================= test session starts ==============================
platform linux -- Python 3.12.3
collected 4 items

test_a.py::test_ok PASSED                                                [ 25%]
test_a.py::test_bad FAILED                                               [ 50%]
test_b.py::test_ok PASSED                                                [ 75%]
test_b.py::test_skip SKIPPED (not ready)                                 [100%]

=================== 1 failed, 2 passed, 1 skipped in 0.05s ====================";
        let out = filter_pytest(&["-v"], v);
        assert!(out.contains("test_a.py: 1 passed, 1 failed"), "{}", out);
        assert!(out.contains("test_b.py: 1 passed"), "{}", out);
        assert!(out.contains("FAIL test_a.py::test_bad"), "{}", out);
    }

    #[test]
    fn pytest_all_green_is_one_line() {
        let g = "============================= test session starts ==============================\nplatform linux\ncollected 3 items\n\ntest_a.py ...                                                            [100%]\n\n============================== 3 passed in 0.01s ===============================";
        assert_eq!(filter_pytest(&["-q"], g), "pytest: 3 passed in 0.01s");
    }

    #[test]
    fn pytest_failure_cap_announced_and_errors_kept() {
        let mut raw = String::from(
            "============================= test session starts ==============================\ncollected 40 items\n\n=================================== FAILURES ===================================\n",
        );
        for i in 0..40 {
            raw.push_str(&format!("________ test_{} ________\n>       assert False\nE       AssertionError: no {}\n\ntest_x.py:{}: AssertionError\n", i, i, i));
        }
        raw.push_str("=========================== short test summary info ============================\nERROR test_y.py - ImportError: boom\n==================== 40 failed in 1.00s ====================");
        let out = filter_pytest(&[], &raw);
        assert!(has_truncation(&out), "{}", out);
        assert!(
            out.contains("ERROR test_y.py - ImportError: boom"),
            "{}",
            out
        );
        assert!(out.contains("pytest: 40 failed in 1.00s"), "{}", out);
    }

    #[test]
    fn pytest_empty_and_launch_failure_pass_through() {
        assert_eq!(filter_pytest(&[], ""), "");
        let boom = "/bin/bash: line 1: /home/u/.local/bin/pytest: Permission denied";
        assert_eq!(filter_pytest(&[], boom), boom);
    }

    #[test]
    fn mypy_ruff_group_by_file_and_report_clean() {
        let out = filter_mypy(
            "app/main.py:12: error: Incompatible return value type (got \"int\", expected \"str\")  [return-value]\napp/main.py:20: note: some note\napp/db.py:3: error: Cannot find implementation for module \"foo\"  [import-not-found]\nFound 2 errors in 2 files (checked 10 source files)",
        );
        assert!(
            out.contains("app/main.py\n  12  error: Incompatible return value type"),
            "{}",
            out
        );
        assert!(
            out.contains("app/db.py\n  3  error: Cannot find implementation"),
            "{}",
            out
        );
        assert!(out.contains("Found 2 errors in 2 files"), "{}", out);
        assert_eq!(filter_ruff("All checks passed!"), "All checks passed!");
        assert_eq!(filter_ruff(""), "ruff: ok");
    }

    #[test]
    fn ruff_diagnostics_have_col_and_code() {
        let out = filter_ruff(
            "src/a.py:1:1: F401 [*] `os` imported but unused\nsrc/a.py:9:5: E711 comparison to None\nFound 2 errors.",
        );
        assert!(
            out.contains("src/a.py\n  1:1  F401 [*] `os` imported but unused"),
            "{}",
            out
        );
        assert!(out.contains("  9:5  E711 comparison to None"), "{}", out);
    }

    #[test]
    fn diagnostics_cap_is_announced() {
        let mut raw = String::new();
        for i in 0..80 {
            raw.push_str(&format!("src/f{}.py:1:1: F401 unused {}\n", i, i));
        }
        let out = filter_ruff(&raw);
        assert!(has_truncation(&out), "{}", out);
    }

    #[test]
    fn black_lists_files_and_reports_clean() {
        let out = filter_black(
            "would reformat src/a.py\nwould reformat src/b.py\nOh no! 💥 💔 💥\n2 files would be reformatted, 8 files would be left unchanged.",
        );
        assert!(out.contains("black: 2 files to reformat:"), "{}", out);
        assert!(
            out.contains("src/a.py") && out.contains("src/b.py"),
            "{}",
            out
        );
        assert!(out.contains("Oh no!"), "{}", out);
        assert_eq!(
            filter_black("12 files would be left unchanged."),
            "black: ok (12 unchanged)"
        );
    }

    #[test]
    fn pip_list_and_install_compact() {
        let list = filter_pip(
            &["list"],
            "Package    Version\n---------- -------\nrequests   2.31.0\nurllib3    2.2.1",
        );
        assert!(list.contains("requests 2.31.0"), "{}", list);
        assert!(list.contains("pip: 2 packages"), "{}", list);
        let inst = filter_pip(
            &["install", "requests"],
            "Collecting requests\n  Downloading requests-2.31.0-py3-none-any.whl (62 kB)\nRequirement already satisfied: idna in ./lib\nInstalling collected packages: requests\nSuccessfully installed requests-2.31.0\n[notice] A new release of pip is available",
        );
        assert!(
            inst.contains("Successfully installed requests-2.31.0"),
            "{}",
            inst
        );
        assert!(inst.contains("(4 download/build steps)"), "{}", inst);
        assert!(!inst.contains("notice"), "{}", inst);
    }

    #[test]
    fn uv_and_poetry_collapse_progress() {
        let uv = filter_uv(
            &["sync"],
            "Resolved 42 packages in 12ms\nPrepared 3 packages in 1.2s\nInstalled 3 packages in 8ms\n + requests==2.31.0",
        );
        assert!(uv.contains("Installed 3 packages in 8ms"), "{}", uv);
        assert!(uv.contains("+ requests==2.31.0"), "{}", uv);
        assert!(!uv.contains("Resolved 42"), "{}", uv);
        let uv_pip = filter_uv(
            &["pip", "list"],
            "Package Version\n------- -------\nrich    13.7.0",
        );
        assert!(uv_pip.contains("rich 13.7.0"), "{}", uv_pip);
        let po = filter_poetry(
            &["install"],
            "Creating virtualenv x\nResolving dependencies... (1.2s)\n\nPackage operations: 3 installs, 0 updates\n\n  • Installing requests (2.31.0)",
        );
        assert!(po.contains("Installing requests (2.31.0)"), "{}", po);
        assert!(!po.contains("Resolving dependencies"), "{}", po);
    }

    #[test]
    fn bandit_keeps_issues_and_totals() {
        let out = filter_bandit(
            ">> Issue: [B101:assert_used] Use of assert detected.\n   Severity: Low   Confidence: High\n   Location: app/x.py:12:4\nTotal issues (by severity):\n\tHigh: 0",
        );
        assert!(out.contains("B101:assert_used"), "{}", out);
        assert!(out.contains("Location: app/x.py:12:4"), "{}", out);
        assert!(out.contains("High: 0"), "{}", out);
        assert_eq!(filter_bandit(""), "bandit: ok");
    }
}
