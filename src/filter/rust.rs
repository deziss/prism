//! Rust toolchain filters: `cargo` (+ subcommands) and `cargo-nextest`.
//!
//! Compiler diagnostics keep the message, the `file:line:col`, and any `help:`, and
//! drop the ASCII art gutter; identical messages across files fold into one `×N` line.
//! Success paths collapse to a single line with counts.

use super::common::*;

// ─── diagnostics (build / check / clippy / doc / test-compile) ────────────────

struct Diag {
    kind: String,    // "error[E0382]" | "warning" | "error"
    msg: String,
    loc: Option<String>,
    help: Vec<String>,
}

fn is_gutter(t: &str) -> bool {
    // `  |`, `3 | code`, `  = note: ...`, `...`
    t == "|" || t == "..." || t.starts_with("| ") || t == "|"
        || t.starts_with("= note")
        || t.starts_with("= help")
        || t.split_once(" | ").map(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit() || c == ' ')).unwrap_or(false)
}

/// Parse rustc/clippy diagnostics into structured records plus the leftover lines
/// (progress, `Finished`, summary counts) for the caller to summarise.
fn parse_diags(output: &str) -> (Vec<Diag>, Vec<String>) {
    let mut diags: Vec<Diag> = Vec::new();
    let mut rest: Vec<String> = Vec::new();
    let mut cur: Option<Diag> = None;
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let head = t
            .strip_prefix("error")
            .map(|r| ("error", r))
            .or_else(|| t.strip_prefix("warning").map(|r| ("warning", r)));
        if let Some((level, r)) = head {
            // `error: msg`, `error[E0382]: msg`, `warning: msg` — but not `error: could not compile`
            let (code, after) = match r.strip_prefix('[') {
                Some(x) => match x.split_once(']') {
                    Some((c, a)) => (Some(c), a),
                    None => (None, r),
                },
                None => (None, r),
            };
            if let Some(msg) = after.strip_prefix(": ") {
                if msg.starts_with("could not compile")
                    || msg.starts_with("aborting due to")
                    || msg.starts_with("build failed")
                    || msg.starts_with("`prism` (lib) generated")
                    || msg.ends_with("warnings emitted")
                {
                    rest.push(t.to_string());
                    continue;
                }
                if let Some(d) = cur.take() {
                    diags.push(d);
                }
                cur = Some(Diag {
                    kind: match code {
                        Some(c) => format!("{}[{}]", level, c),
                        None => level.to_string(),
                    },
                    msg: msg.to_string(),
                    loc: None,
                    help: Vec::new(),
                });
                continue;
            }
        }
        if let Some(loc) = t.strip_prefix("--> ") {
            if let Some(d) = cur.as_mut() {
                if d.loc.is_none() {
                    d.loc = Some(loc.to_string());
                }
            }
            continue;
        }
        if let Some(h) = t.strip_prefix("help: ") {
            if let Some(d) = cur.as_mut() {
                if d.help.len() < 2 {
                    d.help.push(h.to_string());
                }
            }
            continue;
        }
        if t.starts_with("note: run with `RUST_BACKTRACE") {
            continue;
        }
        if cur.is_some() && (is_gutter(t) || t.starts_with("note:") || t.starts_with("^")) {
            continue;
        }
        if cur.is_some() {
            // unrelated line ends the current diagnostic
            diags.push(cur.take().unwrap());
        }
        rest.push(t.to_string());
    }
    if let Some(d) = cur {
        diags.push(d);
    }
    (diags, rest)
}

/// `path/to/file.rs:12:5` → short form for the `×N` fold.
fn short_loc(loc: &str) -> String {
    let file = loc.rsplit('/').next().unwrap_or(loc);
    file.to_string()
}

fn render_diags(diags: &[Diag]) -> Vec<String> {
    let l = limits();
    // fold identical (kind, msg) pairs
    let mut folded: Vec<(&Diag, Vec<String>)> = Vec::new();
    for d in diags {
        match folded
            .iter_mut()
            .find(|(f, _)| f.kind == d.kind && f.msg == d.msg)
        {
            Some((_, locs)) => locs.push(d.loc.clone().unwrap_or_default()),
            None => folded.push((d, vec![d.loc.clone().unwrap_or_default()])),
        }
    }
    let mut out = Vec::new();
    let mut dropped = 0usize;
    for (d, locs) in &folded {
        if out.len() >= l.max_diagnostics {
            dropped += locs.len();
            continue;
        }
        if locs.len() > 1 {
            let shown = locs.len().min(8);
            let list: Vec<String> = locs[..shown].iter().map(|x| short_loc(x)).collect();
            let extra = if locs.len() > shown {
                format!(", {}", more(locs.len() - shown, "sites"))
            } else {
                String::new()
            };
            out.push(format!(
                "{}: {} ×{}: {}{}",
                d.kind,
                d.msg,
                locs.len(),
                list.join(", "),
                extra
            ));
        } else {
            let loc = locs[0].clone();
            if loc.is_empty() {
                out.push(format!("{}: {}", d.kind, d.msg));
            } else {
                out.push(format!("{} {}: {}", d.kind, loc, d.msg));
            }
        }
        for h in &d.help {
            out.push(format!("  help: {}", h));
        }
    }
    if dropped > 0 {
        out.push(more(dropped, "diagnostics"));
    }
    out
}

/// `Finished \`dev\` profile … in 0.43s` → `0.43s`
fn finished_time(rest: &[String]) -> Option<String> {
    rest.iter()
        .find(|l| l.starts_with("Finished"))
        .and_then(|l| l.rsplit(" in ").next().map(|s| s.trim().to_string()))
}

fn count_progress(rest: &[String]) -> usize {
    rest.iter()
        .filter(|l| l.starts_with("Compiling ") || l.starts_with("Checking ") || l.starts_with("Documenting "))
        .count()
}

/// Shared shape for build-like subcommands.
fn build_like(sub: &str, output: &str) -> String {
    let (diags, rest) = parse_diags(output);
    let errors = diags.iter().filter(|d| d.kind.starts_with("error")).count();
    let warnings = diags.len() - errors;
    let mut out = Vec::new();
    if diags.is_empty() {
        let mut head = format!("cargo {}: ok", sub);
        let crates = count_progress(&rest);
        let mut bits = Vec::new();
        if crates > 0 {
            bits.push(format!("{} crates", crates));
        }
        if let Some(t) = finished_time(&rest) {
            bits.push(t);
        }
        if !bits.is_empty() {
            head.push_str(&format!(" ({})", bits.join(", ")));
        }
        out.push(head);
        // surface anything unexpected that isn't progress noise
        let extra: Vec<&String> = rest
            .iter()
            .filter(|l| {
                !l.starts_with("Compiling ")
                    && !l.starts_with("Checking ")
                    && !l.starts_with("Documenting ")
                    && !l.starts_with("Finished")
                    && !l.starts_with("Blocking")
                    && !l.starts_with("Generated")
                    && !l.starts_with("Locking")
                    && !l.starts_with("Updating")
                    && !l.starts_with("Downloaded")
                    && !l.starts_with("Downloading")
            })
            .collect();
        if !extra.is_empty() {
            out.extend(
                cap_lines(extra.iter().map(|s| s.as_str()), limits().max_diagnostics, "lines")
                    .lines()
                    .map(String::from),
            );
        }
        return out.join("\n");
    }
    out.extend(render_diags(&diags));
    let mut trailer = format!("cargo {}: {}", sub, plural(errors, "error"));
    trailer.push_str(&format!(", {}", plural(warnings, "warning")));
    if let Some(t) = finished_time(&rest) {
        trailer.push_str(&format!(" ({})", t));
    }
    out.push(trailer);
    out.join("\n")
}

pub(crate) fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{} {}", n, noun)
    } else {
        format!("{} {}s", n, noun)
    }
}

// ─── cargo test ───────────────────────────────────────────────────────────────

struct TestTotals {
    passed: usize,
    failed: usize,
    ignored: usize,
    suites: usize,
    secs: f64,
}

fn parse_test_result(line: &str) -> Option<(usize, usize, usize, f64)> {
    // `test result: ok. 49 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.80s`
    let rest = line.split_once("test result:")?.1;
    // each clause is `<n> <kw>`, e.g. `ok. 49 passed` / ` 0 filtered out`
    let num = |kw: &str| -> usize {
        rest.split(';')
            .filter_map(|p| {
                let words: Vec<&str> = p.split_whitespace().collect();
                let at = words.iter().position(|w| *w == kw)?;
                words.get(at.checked_sub(1)?)?.parse().ok()
            })
            .next()
            .unwrap_or(0)
    };
    let secs = rest
        .rsplit("finished in ")
        .next()
        .and_then(|s| s.trim().trim_end_matches('s').parse::<f64>().ok())
        .unwrap_or(0.0);
    Some((num("passed"), num("failed"), num("ignored"), secs))
}

pub(crate) fn filter_cargo_test(output: &str) -> String {
    let l = limits();
    // compile failure before any test ran
    if output.contains("error[E") || output.contains("error: could not compile") {
        return build_like("test", output);
    }
    let mut totals = TestTotals { passed: 0, failed: 0, ignored: 0, suites: 0, secs: 0.0 };
    let mut failed_names: Vec<String> = Vec::new();
    let mut failure_details: Vec<String> = Vec::new();
    let mut listing: Vec<String> = Vec::new();
    let mut in_stdout_block: Option<String> = None;

    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some((p, f, i, s)) = parse_test_result(t) {
            totals.passed += p;
            totals.failed += f;
            totals.ignored += i;
            totals.secs += s;
            totals.suites += 1;
            continue;
        }
        // `cargo test -- --list` lines: `mod::test_name: test`
        if let Some(name) = t.strip_suffix(": test") {
            listing.push(name.to_string());
            continue;
        }
        if let Some(rest) = t.strip_prefix("---- ") {
            in_stdout_block = rest.strip_suffix(" stdout ----").map(String::from);
            continue;
        }
        if let Some(name) = t.strip_prefix("test ") {
            if let Some(n) = name.strip_suffix(" ... FAILED") {
                failed_names.push(n.to_string());
            }
            continue;
        }
        if let Some(test) = in_stdout_block.clone() {
            // keep the panic line + assertion text, drop backtrace chatter
            if let Some(at) = t.split_once("panicked at ") {
                failure_details.push(format!("FAIL {}\n  {}", test, at.1.trim_end_matches(':')));
                continue;
            }
            if t.starts_with("note:") || t.starts_with("stack backtrace") || t.starts_with("thread '") {
                continue;
            }
            if failure_details.last().map(|d| d.starts_with(&format!("FAIL {}", test))).unwrap_or(false) {
                let last = failure_details.last_mut().unwrap();
                if last.lines().count() < 4 {
                    last.push_str(&format!("\n  {}", truncate(t, 200)));
                }
            }
            continue;
        }
    }

    let mut out: Vec<String> = Vec::new();
    if !listing.is_empty() {
        // group `mod::path::test` by module
        let mut groups: Vec<(String, Vec<String>)> = Vec::new();
        for name in &listing {
            let (module, test) = match name.rfind("::") {
                Some(i) => (name[..i].to_string(), name[i + 2..].to_string()),
                None => (String::new(), name.clone()),
            };
            match groups.iter_mut().find(|(m, _)| *m == module) {
                Some((_, v)) => v.push(test),
                None => groups.push((module, vec![test])),
            }
        }
        out.push(format!("{} tests, {} modules", listing.len(), groups.len()));
        let mut dropped = 0usize;
        for (m, tests) in &groups {
            if out.len() > l.list_max_lines {
                dropped += tests.len();
                continue;
            }
            out.push(format!("{}: {}", if m.is_empty() { "(root)" } else { m }, tests.join(", ")));
        }
        if dropped > 0 {
            out.push(more(dropped, "tests"));
        }
        return out.join("\n");
    }

    if totals.suites == 0 {
        return generic(output);
    }
    let mut head = format!("cargo test: {} passed", totals.passed);
    if totals.failed > 0 {
        head.push_str(&format!(", {} failed", totals.failed));
    }
    if totals.ignored > 0 {
        head.push_str(&format!(", {} ignored", totals.ignored));
    }
    head.push_str(&format!(
        " ({}, {:.2}s)",
        plural(totals.suites, "suite"),
        totals.secs
    ));
    out.push(head);
    let shown = failure_details.len().min(l.test_max_failures);
    out.extend(failure_details[..shown].iter().cloned());
    if failure_details.len() > shown {
        out.push(more(failure_details.len() - shown, "failures"));
    }
    // failures with no captured detail still get named
    for n in &failed_names {
        if !failure_details.iter().any(|d| d.contains(n)) {
            out.push(format!("FAIL {}", n));
        }
    }
    out.join("\n")
}

pub(crate) fn filter_nextest(output: &str) -> String {
    let l = limits();
    let mut passed = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut summary: Option<String> = None;
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with("PASS") {
            passed += 1;
            continue;
        }
        if t.starts_with("FAIL") || t.starts_with("TRY") || t.starts_with("SIGSEGV") {
            failures.push(squeeze_ws(t));
            continue;
        }
        if t.starts_with("Summary") {
            summary = Some(squeeze_ws(t));
            continue;
        }
        if t.starts_with("Starting") || t.starts_with("Compiling") || t.starts_with("Finished") {
            continue;
        }
        if !failures.is_empty() {
            failures.push(format!("  {}", truncate(t, 200)));
        }
    }
    let mut out = Vec::new();
    out.push(
        summary.unwrap_or_else(|| format!("nextest: {} passed, {} failed", passed, failures.len())),
    );
    out.extend(
        cap_lines(failures.iter().map(|s| s.as_str()), l.max_diagnostics, "lines")
            .lines()
            .map(String::from),
    );
    out.join("\n")
}

// ─── cargo fmt --check ────────────────────────────────────────────────────────

fn cargo_fmt(output: &str) -> String {
    let l = limits();
    if output.trim().is_empty() {
        return "cargo fmt: ok".to_string();
    }
    struct F {
        path: String,
        hunks: usize,
        add: usize,
        del: usize,
        first: Vec<String>,
    }
    let mut files: Vec<F> = Vec::new();
    let mut other: Vec<&str> = Vec::new();
    for line in output.lines() {
        let t = line.trim_end();
        if let Some(rest) = t.trim_start().strip_prefix("Diff in ") {
            let path = rest.split(':').next().unwrap_or(rest);
            let short = match path.rfind("/src/") {
                Some(i) => path[i + 1..].to_string(),
                None => path.rsplit('/').next().unwrap_or(path).to_string(),
            };
            match files.iter_mut().find(|f| f.path == short) {
                Some(f) => f.hunks += 1,
                None => files.push(F { path: short, hunks: 1, add: 0, del: 0, first: Vec::new() }),
            }
            continue;
        }
        if let Some(f) = files.last_mut() {
            if t.starts_with('+') {
                f.add += 1;
                if f.hunks == 1 && f.first.len() < 6 {
                    f.first.push(truncate(t, 120));
                }
                continue;
            }
            if t.starts_with('-') {
                f.del += 1;
                if f.hunks == 1 && f.first.len() < 6 {
                    f.first.push(truncate(t, 120));
                }
                continue;
            }
            continue; // unchanged context
        }
        if !t.trim().is_empty() {
            other.push(t);
        }
    }
    if files.is_empty() {
        return cap_lines(other.iter().copied(), l.passthrough_max_lines, "lines");
    }
    let mut out = Vec::new();
    let mut dropped = 0usize;
    for f in &files {
        if out.len() >= l.max_diagnostics {
            dropped += 1;
            continue;
        }
        out.push(format!(
            "{}: {}, +{} -{}",
            f.path,
            plural(f.hunks, "hunk"),
            f.add,
            f.del
        ));
        for line in &f.first {
            out.push(format!("  {}", line));
        }
        if f.hunks > 1 {
            out.push(format!("  {}", more(f.hunks - 1, "hunks")));
        }
    }
    if dropped > 0 {
        out.push(more(dropped, "files"));
    }
    out.push(format!("cargo fmt: {} need formatting", plural(files.len(), "file")));
    out.join("\n")
}

// ─── trees, listings, package ops ─────────────────────────────────────────────

fn cargo_tree(output: &str) -> String {
    let l = limits();
    let mut out = Vec::new();
    let mut dropped = 0usize;
    for line in output.lines() {
        let t = line.trim_end();
        if t.is_empty() {
            continue;
        }
        if out.len() < l.list_max_lines {
            out.push(flatten_tree(t));
        } else {
            dropped += 1;
        }
    }
    if dropped > 0 {
        out.push(more(dropped, "deps"));
    }
    out.join("\n")
}

fn cargo_list(output: &str) -> String {
    let l = limits();
    let mut names: Vec<String> = Vec::new();
    let mut head: Vec<&str> = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.ends_with(':') {
            head.push(t);
            continue;
        }
        // `    add                  Add dependencies to a Cargo.toml`
        names.push(t.split_whitespace().next().unwrap_or(t).to_string());
    }
    if names.is_empty() {
        return generic(output);
    }
    let mut out: Vec<String> = head.iter().map(|s| s.to_string()).collect();
    let shown = names.len().min(l.list_max_lines * 10);
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
        out.push(more(names.len() - shown, "commands"));
    }
    out.join("\n")
}

fn cargo_pkg_op(sub: &str, output: &str) -> String {
    let l = limits();
    let mut keep: Vec<String> = Vec::new();
    let mut progress = 0usize;
    for line in output.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with("Downloading")
            || t.starts_with("Downloaded")
            || t.starts_with("Compiling")
            || t.starts_with("Locking")
            || t.starts_with("Installing")
            || t.starts_with("Finished")
            || t.starts_with("Updating")
        {
            progress += 1;
            continue;
        }
        keep.push(squeeze_ws(t));
    }
    let mut out = cap_vec(keep, l.max_diagnostics, "lines");
    if out.is_empty() {
        out.push(format!("cargo {}: ok ({} steps)", sub, progress));
    }
    out.join("\n")
}

// ─── public entry points ──────────────────────────────────────────────────────

pub(crate) fn filter_cargo_build(output: &str) -> String {
    build_like("build", output)
}

pub(crate) fn filter_cargo_check(output: &str) -> String {
    build_like("check", output)
}

pub(crate) fn filter_cargo_doc(output: &str) -> String {
    build_like("doc", output)
}

pub(crate) fn filter_clippy(output: &str) -> String {
    build_like("clippy", output)
}

pub(crate) fn filter_cargo(args: &[&str], output: &str) -> String {
    match find_subcommand(args) {
        Some("test") if args.iter().any(|a| *a == "--list") => filter_cargo_test(output),
        Some("test") => filter_cargo_test(output),
        Some("nextest") => filter_nextest(output),
        Some("build") | Some("b") | Some("watch") => filter_cargo_build(output),
        Some("check") | Some("c") => filter_cargo_check(output),
        Some("clippy") => filter_clippy(output),
        Some("doc") | Some("rustdoc") => filter_cargo_doc(output),
        Some("bench") => filter_cargo_test(output),
        Some("tree") => cargo_tree(output),
        Some("metadata") | Some("read-manifest") => compact_json_output(output, generic),
        Some("fmt") => cargo_fmt(output),
        Some("add") | Some("remove") | Some("rm") | Some("update") | Some("install")
        | Some("uninstall") | Some("vendor") | Some("fetch") | Some("generate-lockfile") => {
            cargo_pkg_op(find_subcommand(args).unwrap_or("cargo"), output)
        }
        Some("run") | Some("r") => {
            // program output; strip cargo's own prefix lines
            let body: Vec<&str> = output
                .lines()
                .filter(|l| {
                    let t = l.trim();
                    !(t.starts_with("Compiling ")
                        || t.starts_with("Finished")
                        || t.starts_with("Running ")
                        || t.starts_with("Blocking"))
                })
                .collect();
            cap_lines(body.into_iter(), limits().passthrough_max_lines, "lines")
        }
        None => cargo_list(output),
        _ => generic(output),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_clean_is_one_line() {
        let out = filter_cargo(
            &["check", "--offline"],
            "    Checking prism v0.1.0 (/x)\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.75s\n",
        );
        assert_eq!(out, "cargo check: ok (1 crates, 0.75s)");
        assert_eq!(
            filter_cargo(&["check"], "    Finished `dev` profile target(s) in 0.43s\n"),
            "cargo check: ok (0.43s)"
        );
    }

    const ERR: &str = "\
    Checking prism v0.1.0 (/x)
error[E0382]: borrow of moved value: `files`
    --> src/filter/js.rs:1538:20
     |
1519 |     let mut files: Vec<String> = Vec::new();
     |         --------- move occurs because `files` has type `Vec<String>`
...
1534 |         out.extend(cap_vec(files, l.list_max_lines, \"files\"));
     |                            ----- value moved here
     |
note: consider changing this parameter type in function `cap_vec` to borrow instead
help: consider cloning the value if the performance cost is acceptable
     |
1534 |         out.extend(cap_vec(files.clone(), l.list_max_lines, \"files\"));
     |                                 ++++++++

warning: unused import: `super::common::*`
 --> src/filter/cloud.rs:3:5
  |
3 | use super::common::*;
  |     ^^^^^^^^^^^^^^^^
  |
  = note: `#[warn(unused_imports)]` on by default

warning: unused import: `super::common::*`
 --> src/filter/db.rs:3:5
  |
3 | use super::common::*;

error: could not compile `prism` (lib) due to 1 previous error; 2 warnings emitted";

    #[test]
    fn diagnostics_keep_location_message_help_and_fold_duplicates() {
        let out = filter_cargo(&["check"], ERR);
        assert!(
            out.contains("error[E0382] src/filter/js.rs:1538:20: borrow of moved value: `files`"),
            "{}",
            out
        );
        assert!(out.contains("  help: consider cloning the value"), "{}", out);
        // the two identical warnings fold into one line naming both sites
        assert!(
            out.contains("warning: unused import: `super::common::*` ×2: cloud.rs:3:5, db.rs:3:5"),
            "{}",
            out
        );
        assert!(out.contains("cargo check: 1 error, 2 warnings"), "{}", out);
        // gutter art is gone
        assert!(!out.contains("^^^"), "{}", out);
        assert!(!out.contains("1519 |"), "{}", out);
    }

    #[test]
    fn diagnostics_cap_is_announced() {
        let mut fixture = String::new();
        for i in 0..60 {
            fixture.push_str(&format!(
                "warning: thing {} is odd\n --> src/f{}.rs:1:1\n  |\n1 | code\n\n",
                i, i
            ));
        }
        let out = filter_cargo(&["check"], &fixture);
        assert!(has_truncation(&out), "{}", out);
        assert!(out.contains("cargo check: 0 errors, 60 warnings"), "{}", out);
    }

    const TEST_OK: &str = "\
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.36s
     Running unittests src/lib.rs (target/debug/deps/prism-0fb)
running 49 tests
test cache::tests::test_pseudo_embedding_similarity ... ok
test compress::code_protection_tests::diff_hunk_survives_verbatim ... ok
test result: ok. 49 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.80s

     Running unittests src/main.rs (target/debug/deps/prism-1ab)
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests prism
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s";

    #[test]
    fn test_all_green_is_one_line() {
        let out = filter_cargo(&["test", "--offline"], TEST_OK);
        assert_eq!(out, "cargo test: 49 passed, 2 ignored (3 suites, 0.80s)");
    }

    const TEST_FAIL: &str = "\
running 12 tests
test filter::common::tests::marker_roundtrip ... ok
test filter::common::tests::compact_json_shapes ... FAILED

failures:

---- filter::common::tests::compact_json_shapes stdout ----

thread 'filter::common::tests::compact_json_shapes' (2244452) panicked at src/filter/common.rs:485:9:
assertion failed: s.contains(\"deps\")
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    filter::common::tests::compact_json_shapes

test result: FAILED. 11 passed; 1 failed; 0 ignored; 0 measured; 49 filtered out; finished in 0.01s";

    #[test]
    fn test_failure_keeps_name_location_and_assertion() {
        let out = filter_cargo(&["test"], TEST_FAIL);
        assert!(out.starts_with("cargo test: 11 passed, 1 failed (1 suite, 0.01s)"), "{}", out);
        assert!(out.contains("FAIL filter::common::tests::compact_json_shapes"), "{}", out);
        assert!(out.contains("src/filter/common.rs:485:9"), "{}", out);
        assert!(out.contains("assertion failed: s.contains(\"deps\")"), "{}", out);
        assert!(!out.contains("RUST_BACKTRACE"), "{}", out);
    }

    #[test]
    fn test_list_groups_by_module_and_keeps_all_names() {
        let listing = "\
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.15s
     Running unittests src/lib.rs (target/debug/deps/prism-0fb)
cache::tests::test_pseudo_embedding_dimension_and_norm: test
cache::tests::test_pseudo_embedding_similarity: test
compress::code_protection_tests::compression_never_grows_short_text: test
filter::files::tests::grep_empty_and_single_line: test";
        let out = filter_cargo(&["test", "--offline", "--", "--list"], listing);
        assert!(out.starts_with("4 tests, 3 modules"), "{}", out);
        assert!(out.contains("cache::tests: test_pseudo_embedding_dimension_and_norm, test_pseudo_embedding_similarity"), "{}", out);
        assert!(out.contains("filter::files::tests: grep_empty_and_single_line"), "{}", out);
    }

    #[test]
    fn test_compile_error_routes_to_diagnostics() {
        let out = filter_cargo(&["test"], ERR);
        assert!(out.contains("error[E0382] src/filter/js.rs:1538:20"), "{}", out);
        assert!(out.contains("cargo test: 1 error"), "{}", out);
    }

    #[test]
    fn fmt_check_compacts_diffs_and_counts_files() {
        let fmt = "\
Diff in /home/u/prism/src/main.rs:3:
 // comment
-use std::{env, path::PathBuf, fs};
+use std::{env, fs, path::PathBuf};

Diff in /home/u/prism/src/main.rs:40:
-    let x=1;
+    let x = 1;

Diff in /home/u/prism/src/cli.rs:9:
-fn a(){}
+fn a() {}";
        let out = filter_cargo(&["fmt", "--check"], fmt);
        assert!(out.contains("src/main.rs: 2 hunks, +2 -2"), "{}", out);
        assert!(out.contains("-use std::{env, path::PathBuf, fs};"), "{}", out);
        assert!(out.contains("[+1 more hunks]"), "{}", out);
        assert!(out.contains("src/cli.rs: 1 hunk, +1 -1"), "{}", out);
        assert!(out.ends_with("cargo fmt: 2 files need formatting"), "{}", out);
        assert_eq!(filter_cargo(&["fmt", "--check"], ""), "cargo fmt: ok");
    }

    #[test]
    fn tree_flattens_and_announces_cap() {
        let out = filter_cargo(
            &["tree"],
            "prism v0.1.0 (/x)\n├── anyhow v1.0.102\n├── axum v0.8.9\n│   ├── axum-core v0.5.6\n│   │   ├── bytes v1.11.1\n│   └── http v1.4.1 (*)",
        );
        assert!(out.contains("prism v0.1.0 (/x)"), "{}", out);
        assert!(out.contains("\n anyhow v1.0.102"), "{}", out);
        assert!(out.contains("\n  axum-core v0.5.6"), "{}", out);
        assert!(out.contains("\n   bytes v1.11.1"), "{}", out);
        assert!(out.contains("http v1.4.1 (*)"), "{}", out);
        let mut big = String::new();
        for i in 0..400 {
            big.push_str(&format!("├── crate{} v1.0.0\n", i));
        }
        assert!(has_truncation(&filter_cargo(&["tree"], &big)));
    }

    #[test]
    fn metadata_and_unknown_and_empty() {
        let meta = filter_cargo(&["metadata", "--format-version", "1"], "{\"packages\":[{\"name\":\"prism\",\"version\":\"0.1.0\"}],\"workspace_root\":\"/x\"}");
        assert!(meta.contains("name: prism"), "{}", meta);
        assert!(meta.contains("workspace_root: /x"), "{}", meta);
        assert_eq!(filter_cargo(&["check"], ""), "cargo check: ok");
        // unknown subcommand still compacts rather than dumping raw
        let unk = filter_cargo(&["audit"], "\n\nCrate:     foo\n\n\nVersion:   1.0\n");
        assert_eq!(unk, "Crate:     foo\n\nVersion:   1.0"); // generic keeps one blank separator
    }

    #[test]
    fn nextest_summary_and_failures() {
        let out = filter_nextest(
            "    Starting 3 tests across 1 binary\n        PASS [   0.003s] prism cache::tests::a\n        FAIL [   0.004s] prism filter::b\n  thread panicked at src/x.rs:1:1\n  Summary [   0.010s] 3 tests run: 2 passed, 1 failed",
        );
        assert!(out.starts_with("Summary [ 0.010s] 3 tests run: 2 passed, 1 failed"), "{}", out);
        assert!(out.contains("FAIL [ 0.004s] prism filter::b"), "{}", out);
        assert!(out.contains("panicked at src/x.rs:1:1"), "{}", out);
    }

    #[test]
    fn run_strips_cargo_prefix_lines() {
        let out = filter_cargo(
            &["run", "--", "--help"],
            "   Compiling prism v0.1.0\n    Finished `dev` profile in 1.2s\n     Running `target/debug/prism --help`\nUsage: prism [OPTIONS]",
        );
        assert_eq!(out, "Usage: prism [OPTIONS]");
    }
}
