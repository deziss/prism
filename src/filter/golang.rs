//! Go toolchain filters: `go` subcommands and `golangci-lint`.

use super::common::*;

fn plural(n: usize, noun: &str) -> String {
    if n == 1 { format!("{} {}", n, noun) } else { format!("{} {}s", n, noun) }
}

/// Group `path:line[:col]: message` diagnostics under one header per file.
fn group_file_diags(tool: &str, output: &str) -> String {
    let l = limits();
    let mut files: Vec<(String, Vec<String>)> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    let mut pkgs: Vec<String> = Vec::new();
    let mut count = 0usize;
    for line in output.lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if let Some(pkg) = tt.strip_prefix("# ") {
            pkgs.push(pkg.to_string());
            continue;
        }
        let mut it = tt.splitn(4, ':');
        let path = it.next().unwrap_or("");
        let ln = it.next().unwrap_or("").trim();
        if !path.is_empty() && !ln.is_empty() && ln.chars().all(|c| c.is_ascii_digit()) {
            let third = it.next().unwrap_or("");
            let (pos, msg) = if third.trim().chars().all(|c| c.is_ascii_digit()) && !third.trim().is_empty() {
                (format!("{}:{}", ln, third.trim()), it.next().unwrap_or("").trim().to_string())
            } else {
                (ln.to_string(), format!("{}{}", third, it.next().map(|r| format!(":{}", r)).unwrap_or_default()).trim().to_string())
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
        let mut out = cap_vec(other, l.max_diagnostics, "lines");
        if out.is_empty() {
            out.push(format!("{}: ok", tool));
        }
        return out.join("\n");
    }
    let mut out: Vec<String> = Vec::new();
    if pkgs.len() > 1 {
        out.push(format!("packages: {}", pkgs.join(", ")));
    }
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
    out.push(format!("{}: {} in {}", tool, plural(count, "issue"), plural(files.len(), "file")));
    out.join("\n")
}

pub(crate) fn filter_go_test(output: &str) -> String {
    let l = limits();
    if output.contains("[build failed]") || output.contains("syntax error") {
        return group_file_diags("go build", output);
    }
    let mut ok_pkgs: Vec<String> = Vec::new();
    let mut no_test = 0usize;
    let mut fail_pkgs: Vec<String> = Vec::new();
    let mut fails: Vec<(String, Vec<String>)> = Vec::new();
    let mut coverage: Vec<String> = Vec::new();
    let mut in_panic = false;
    let mut panic_frames = 0usize;
    let mut cached = 0usize;

    for line in output.lines() {
        let t = line.trim_end();
        let tt = t.trim();
        if tt.is_empty() {
            continue;
        }
        if tt.starts_with("=== RUN") || tt.starts_with("=== PAUSE") || tt.starts_with("=== CONT") || tt.starts_with("--- PASS") {
            continue;
        }
        if let Some(rest) = tt.strip_prefix("--- FAIL: ") {
            let name = rest.split_whitespace().next().unwrap_or(rest).to_string();
            fails.push((name, Vec::new()));
            continue;
        }
        if let Some(rest) = tt.strip_prefix("ok  ") {
            let pkg = rest.split_whitespace().next().unwrap_or(rest);
            if rest.contains("(cached)") {
                cached += 1;
            }
            ok_pkgs.push(pkg.to_string());
            continue;
        }
        if tt.starts_with("?   ") || tt.contains("[no test files]") {
            no_test += 1;
            continue;
        }
        if let Some(rest) = tt.strip_prefix("FAIL\t").or_else(|| tt.strip_prefix("FAIL    ")) {
            fail_pkgs.push(rest.split_whitespace().next().unwrap_or(rest).to_string());
            continue;
        }
        if tt == "FAIL" || tt == "PASS" {
            continue;
        }
        if tt.starts_with("coverage:") {
            coverage.push(tt.to_string());
            continue;
        }
        if tt.starts_with("panic:") {
            in_panic = true;
            panic_frames = 0;
            fails.push((tt.to_string(), Vec::new()));
            continue;
        }
        if in_panic {
            // keep the first few source frames, drop the rest of the goroutine dump
            if tt.contains(".go:") {
                panic_frames += 1;
                if panic_frames <= 3 {
                    if let Some((_, lines)) = fails.last_mut() {
                        lines.push(squeeze_ws(tt));
                    }
                }
            }
            continue;
        }
        if let Some((_, lines)) = fails.last_mut() {
            if lines.len() < 6 {
                lines.push(truncate(&squeeze_ws(tt), 200));
            }
            continue;
        }
    }

    let mut out: Vec<String> = Vec::new();
    let mut head = format!("go test: {} ok", ok_pkgs.len());
    if !fail_pkgs.is_empty() {
        head.push_str(&format!(", {} failed", fail_pkgs.len()));
    }
    if no_test > 0 {
        head.push_str(&format!(", {} without tests", no_test));
    }
    if cached > 0 {
        head.push_str(&format!(", {} cached", cached));
    }
    out.push(head);
    if !fail_pkgs.is_empty() {
        out.push(format!("FAIL: {}", fail_pkgs.join(", ")));
    }
    let shown = fails.len().min(l.test_max_failures);
    for (name, lines) in &fails[..shown] {
        out.push(format!("FAIL {}", name));
        for x in lines {
            out.push(format!("  {}", x));
        }
    }
    if fails.len() > shown {
        out.push(more(fails.len() - shown, "failures"));
    }
    out.extend(cap_vec(coverage, 10, "coverage lines"));
    out.join("\n")
}

pub(crate) fn filter_go_build(output: &str) -> String {
    group_file_diags("go build", output)
}

pub(crate) fn filter_go_vet(output: &str) -> String {
    group_file_diags("go vet", output)
}

pub(crate) fn filter_go_mod(args: &[&str], output: &str) -> String {
    let l = limits();
    match args.first() {
        Some(&"tidy") | Some(&"download") | Some(&"vendor") => {
            let mut keep: Vec<String> = Vec::new();
            let mut fetched = 0usize;
            for line in output.lines() {
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                if t.starts_with("go: downloading") || t.starts_with("go: finding") || t.starts_with("go: extracting") {
                    fetched += 1;
                    continue;
                }
                keep.push(squeeze_ws(t));
            }
            let mut out = cap_vec(keep, l.max_diagnostics, "lines");
            if out.is_empty() {
                out.push(format!("go mod: ok ({} modules fetched)", fetched));
            } else if fetched > 0 {
                out.push(format!("({} modules fetched)", fetched));
            }
            out.join("\n")
        }
        Some(&"graph") | Some(&"why") | Some(&"edit") => {
            cap_lines(output.lines().filter(|l| !l.trim().is_empty()), l.list_max_lines, "lines")
        }
        _ => generic(output),
    }
}

pub(crate) fn filter_golangci(output: &str) -> String {
    let body: String = output
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.contains("level=info") && !t.starts_with("time=")
        })
        .collect::<Vec<_>>()
        .join("\n");
    group_file_diags("golangci-lint", &body)
}

pub(crate) fn filter_go(args: &[&str], output: &str) -> String {
    let l = limits();
    match find_subcommand(args) {
        Some("test") => filter_go_test(output),
        Some("build") | Some("install") | Some("generate") => filter_go_build(output),
        Some("vet") => filter_go_vet(output),
        Some("mod") => {
            let rest: Vec<&str> = args.iter().copied().skip_while(|a| *a != "mod").skip(1).collect();
            filter_go_mod(&rest, output)
        }
        Some("fmt") => {
            let files: Vec<&str> = output.lines().map(|l| l.trim()).filter(|t| !t.is_empty()).collect();
            if files.is_empty() {
                "go fmt: ok".to_string()
            } else {
                let n = files.len();
                let mut out = vec![format!("go fmt: {} reformatted:", plural(n, "file"))];
                out.push(cap_lines(files.into_iter(), l.list_max_lines, "files"));
                out.join("\n")
            }
        }
        Some("list") => cap_lines(
            output.lines().filter(|x| !x.trim().is_empty()),
            l.list_max_lines,
            "packages",
        ),
        Some("env") => {
            // `KEY="value"` — drop the empty ones, they are the majority
            let mut kept: Vec<String> = Vec::new();
            let mut empty = 0usize;
            for line in output.lines() {
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                match t.split_once('=') {
                    Some((k, v)) => {
                        let v = v.trim().trim_matches('"');
                        if v.is_empty() {
                            empty += 1;
                        } else {
                            kept.push(format!("{}={}", k, truncate(v, 120)));
                        }
                    }
                    None => kept.push(squeeze_ws(t)),
                }
            }
            kept.sort();
            let mut out = cap_vec(kept, l.list_max_lines, "vars");
            if empty > 0 {
                out.push(format!("({} empty vars omitted)", empty));
            }
            out.join("\n")
        }
        Some("run") => cap_lines(collapse_blank(output).lines(), l.passthrough_max_lines, "lines"),
        Some("version") => output.trim().to_string(),
        _ => generic(output),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_test_all_green_is_one_line() {
        let out = filter_go(
            &["test", "./..."],
            "ok  \tgithub.com/x/a\t0.123s\nok  \tgithub.com/x/b\t(cached)\n?   \tgithub.com/x/c\t[no test files]",
        );
        assert_eq!(out, "go test: 2 ok, 1 without tests, 1 cached");
    }

    #[test]
    fn go_test_failures_keep_names_messages_and_packages() {
        let raw = "\
=== RUN   TestAdd
--- PASS: TestAdd (0.00s)
=== RUN   TestSub
--- FAIL: TestSub (0.01s)
    math_test.go:22: expected 4, got 5
FAIL
FAIL\tgithub.com/x/math\t0.014s
ok  \tgithub.com/x/util\t0.002s
coverage: 81.2% of statements";
        let out = filter_go(&["test", "./..."], raw);
        assert!(out.starts_with("go test: 1 ok, 1 failed"), "{}", out);
        assert!(out.contains("FAIL: github.com/x/math"), "{}", out);
        assert!(out.contains("FAIL TestSub"), "{}", out);
        assert!(out.contains("math_test.go:22: expected 4, got 5"), "{}", out);
        assert!(out.contains("coverage: 81.2% of statements"), "{}", out);
        assert!(!out.contains("=== RUN"), "{}", out);
        assert!(!out.contains("--- PASS"), "{}", out);
    }

    #[test]
    fn go_test_panic_keeps_message_and_first_frames() {
        let raw = "panic: runtime error: index out of range [3] with length 2\n\ngoroutine 1 [running]:\nmain.boom(...)\n\t/app/main.go:12 +0x1d\nmain.main()\n\t/app/main.go:20 +0x5\nruntime.goexit()\n\t/usr/local/go/src/runtime/asm_amd64.s:1650 +0x1\nFAIL\tgithub.com/x/app\t0.002s";
        let out = filter_go(&["test"], raw);
        assert!(out.contains("panic: runtime error: index out of range"), "{}", out);
        assert!(out.contains("/app/main.go:12"), "{}", out);
        assert!(!out.contains("asm_amd64.s"), "{}", out);
    }

    #[test]
    fn go_test_failure_cap_is_announced() {
        let mut raw = String::new();
        for i in 0..40 {
            raw.push_str(&format!("--- FAIL: Test{} (0.00s)\n    x_test.go:{}: boom {}\n", i, i, i));
        }
        raw.push_str("FAIL\tgithub.com/x/y\t0.1s");
        let out = filter_go(&["test"], &raw);
        assert!(has_truncation(&out), "{}", out);
    }

    #[test]
    fn go_build_and_vet_group_by_file() {
        let out = filter_go(
            &["build", "./..."],
            "# github.com/x/app\nmain.go:12:6: undefined: foo\nmain.go:20:2: declared and not used: x\nhelper.go:3:1: syntax error",
        );
        assert!(out.contains("main.go\n  12:6  undefined: foo"), "{}", out);
        assert!(out.contains("  20:2  declared and not used: x"), "{}", out);
        assert!(out.contains("helper.go\n  3:1  syntax error"), "{}", out);
        assert!(out.contains("go build: 3 issues in 2 files"), "{}", out);
        assert_eq!(filter_go(&["vet", "./..."], ""), "go vet: ok");
    }

    #[test]
    fn go_mod_tidy_drops_download_noise() {
        let out = filter_go(
            &["mod", "tidy"],
            "go: downloading github.com/a/b v1.2.3\ngo: downloading github.com/c/d v0.1.0\ngo: finding module for package x",
        );
        assert_eq!(out, "go mod: ok (3 modules fetched)");
        let err = filter_go(&["mod", "tidy"], "go: updates to go.mod needed; to update it:\n\tgo mod tidy");
        assert!(err.contains("updates to go.mod needed"), "{}", err);
    }

    #[test]
    fn go_env_drops_empty_vars_and_counts_them() {
        let out = filter_go(&["env"], "GOARCH=\"amd64\"\nGOBIN=\"\"\nGOCACHE=\"/home/u/.cache/go-build\"\nGOFLAGS=\"\"");
        assert!(out.contains("GOARCH=amd64"), "{}", out);
        assert!(out.contains("GOCACHE=/home/u/.cache/go-build"), "{}", out);
        assert!(out.contains("(2 empty vars omitted)"), "{}", out);
    }

    #[test]
    fn go_list_and_fmt_and_unknown() {
        let list = filter_go(&["list", "./..."], "github.com/x/a\ngithub.com/x/b");
        assert_eq!(list, "github.com/x/a\ngithub.com/x/b");
        let fmt = filter_go(&["fmt", "./..."], "main.go\nhelper.go");
        assert!(fmt.starts_with("go fmt: 2 files reformatted:"), "{}", fmt);
        assert_eq!(filter_go(&["fmt"], ""), "go fmt: ok");
        assert_eq!(filter_go(&["version"], "go version go1.24.1 linux/amd64\n"), "go version go1.24.1 linux/amd64");
    }

    #[test]
    fn golangci_drops_log_lines_and_groups() {
        let out = filter_golangci(
            "level=info msg=\"[config_reader] Used config file .golangci.yml\"\nmain.go:12:6: `foo` is unused (unused)\ntime=2026-09-09 level=info msg=done",
        );
        assert!(out.contains("main.go\n  12:6  `foo` is unused (unused)"), "{}", out);
        assert!(!out.contains("level=info"), "{}", out);
        assert_eq!(filter_golangci("level=info msg=ok"), "golangci-lint: ok");
    }

    #[test]
    fn go_list_cap_is_announced() {
        let mut raw = String::new();
        for i in 0..400 {
            raw.push_str(&format!("github.com/x/pkg{}\n", i));
        }
        assert!(has_truncation(&filter_go(&["list", "./..."], &raw)));
    }
}
