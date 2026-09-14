//! Filters for tool families that had no coverage: diagnostic linters, OS and
//! language package managers, native build drivers, and JavaScript bundlers.
//!
//! These reuse the existing caps in [`crate::config::FilterLimits`] rather than adding
//! new keys. A new config key has to be declared in `known_field_names()` and shipped
//! to agents *before* any hub can push it — an older agent hard-fails its whole config
//! poll on a key it does not recognise — so new caps are a release-ordering problem and
//! these filters do not need one.

use super::common::*;

/// Group `path:line[:col]: message` diagnostics by file.
///
/// The shape every one of these tools emits, and the reason they can share a filter:
/// pylint, shellcheck, stylelint, yamllint, actionlint, markdownlint, cppcheck,
/// clang-tidy, luacheck, vale and tflint all print one problem per line, prefixed by
/// the file it is in. The saving is grouping — an agent reading 400 lines of
/// `src/a.ts:1:1: …` needs the file once, not four hundred times.
///
/// Lines that do not match the shape are kept only when they look like an alert, so a
/// trailing "Your code has been rated at 8.3/10" survives and banner noise does not.
pub(crate) fn filter_diagnostics(output: &str) -> String {
    let cap = limits().max_diagnostics;

    // Insertion-ordered so output follows the tool's own order rather than an
    // alphabetical one the user did not ask for.
    let mut files: Vec<(String, Vec<String>)> = Vec::new();
    let mut trailing: Vec<String> = Vec::new();
    let mut total = 0usize;

    for line in output.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            continue;
        }
        match split_diagnostic(trimmed) {
            Some((path, rest)) => {
                total += 1;
                match files.iter_mut().find(|(p, _)| p == path) {
                    Some((_, entries)) => entries.push(rest.to_string()),
                    None => files.push((path.to_string(), vec![rest.to_string()])),
                }
            }
            None if is_alert_line(trimmed) => trailing.push(trimmed.to_string()),
            None => {}
        }
    }

    if files.is_empty() {
        return if trailing.is_empty() {
            String::new()
        } else {
            trailing.join("\n")
        };
    }

    let mut out: Vec<String> = Vec::new();
    for (path, entries) in &files {
        out.push(format!("{path} ({})", entries.len()));
        for e in cap_vec(entries.clone(), cap, "problems") {
            out.push(format!("  {e}"));
        }
    }
    out.push(format!("{total} problem(s) in {} file(s)", files.len()));
    out.extend(trailing);
    out.join("\n")
}

/// Split `path:line:col: message` into the file and everything after it.
///
/// Deliberately not a regex: the crate already pulls `regex` in, but this runs per
/// line on the hot path and a hand-rolled split avoids the capture allocation. Windows
/// drive letters (`C:\src\a.c:12:3:`) are why the search starts after index 1.
fn split_diagnostic(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut search = 1;
    loop {
        let idx = line[search..].find(':')? + search;
        let after = idx + 1;
        // A file position must be followed by a digit; `note: something` is not one.
        if bytes.get(after).is_some_and(u8::is_ascii_digit) {
            let path = &line[..idx];
            // Reject a bare number as the "path" — that is a plain `12:34` timestamp.
            if path.is_empty() || path.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            return Some((path, line[after..].trim_start()));
        }
        search = after;
        if search >= line.len() {
            return None;
        }
    }
}

/// OS and language package managers: apt, dnf, brew, nix, gem, bundle, conda and kin.
///
/// Their output is overwhelmingly progress — fetch lines, percentages, dependency
/// resolution chatter — wrapped around a handful of lines that matter: what failed,
/// what changed, and the closing summary. Keep the alerts, keep the tail, drop the rest.
pub(crate) fn filter_pkg(output: &str) -> String {
    let cap = limits().list_max_lines;

    let alerts: Vec<String> = output
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .filter(|l| is_alert_line(l) || is_pkg_change(l))
        .map(str::to_string)
        .collect();

    let deduped = dedupe_consecutive(alerts);
    if deduped.is_empty() {
        // Nothing notable happened, so the closing lines are the whole story.
        return tail_lines(output, cap.min(12), "lines");
    }
    cap_vec(deduped, cap, "lines").join("\n")
}

/// Lines describing an actual change to the system, which a summary must not drop.
fn is_pkg_change(line: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "Setting up ",
        "Unpacking ",
        "Removing ",
        "Installing ",
        "Upgrading ",
        "Downgrading ",
        "Reinstalling ",
        "==> Installing",
        "==> Upgrading",
        "Successfully installed",
        "Bundle complete",
    ];
    PREFIXES.iter().any(|p| line.starts_with(p))
        || (line.contains(" upgraded, ") && line.contains(" newly installed"))
}

/// Native build drivers: cmake, bazel, meson, buck2, xcodebuild, swift, zig.
///
/// Deliberately excludes `ninja`, `clang` and `gcc`. prism filters when stdout is a
/// pipe — exactly the case where a build system is consuming the output — and those
/// three emit data as well as diagnostics (`gcc -E` is a preprocessor). Shimming them
/// would corrupt builds rather than compress them. The drivers here are the commands a
/// person types.
pub(crate) fn filter_native_build(output: &str) -> String {
    let cap = limits().max_diagnostics;

    let kept: Vec<String> = output
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .filter(|l| !is_build_progress(l))
        .filter(|l| is_alert_line(l) || is_build_summary(l))
        .map(str::to_string)
        .collect();

    if kept.is_empty() {
        return tail_lines(output, cap.min(10), "lines");
    }
    cap_vec(dedupe_consecutive(kept), cap, "lines").join("\n")
}

/// `[ 42%] Building CXX object …`, `-- Detecting C compiler ABI info`, and the rest of
/// the per-target chatter that scales with project size and says nothing.
fn is_build_progress(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("-- ")
        || t.starts_with("[ ")
        || (t.starts_with('[') && t.contains("%]"))
        || t.starts_with("Compiling ")
        || t.starts_with("Analyzing ")
        || t.starts_with("Loading:")
        || t.starts_with("INFO: Analyzed")
}

fn is_build_summary(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("Build ")
        || t.starts_with("** BUILD")
        || t.starts_with("INFO: Build completed")
        || t.starts_with("Executed ")
        || t.contains("targets up-to-date")
        || t.contains("Compilation failed")
}

/// JavaScript bundlers and monorepo runners: vite, webpack, rollup, esbuild, tsup,
/// parcel, turbo, nx, lerna.
///
/// The per-asset table is the bulk of the output and grows with the app; the numbers
/// that matter are the failures and the closing total. Keeps alerts in full and the
/// tail as the summary.
pub(crate) fn filter_js_bundler(output: &str) -> String {
    let cap = limits().list_max_lines;

    let alerts: Vec<String> = output
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .filter(|l| is_alert_line(l))
        .map(str::to_string)
        .collect();

    let summary = tail_lines(output, cap.min(8), "lines");
    if alerts.is_empty() {
        return summary;
    }

    let mut out = cap_vec(dedupe_consecutive(alerts), cap, "lines");
    if !summary.trim().is_empty() {
        out.push(summary);
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_group_by_file_and_count() {
        let out = filter_diagnostics(
            "src/a.ts:1:1: error one\nsrc/a.ts:9:2: error two\nsrc/b.ts:3:1: error three\n",
        );

        assert!(out.contains("src/a.ts (2)"), "{out}");
        assert!(out.contains("src/b.ts (1)"), "{out}");
        assert!(out.contains("3 problem(s) in 2 file(s)"), "{out}");
        // The file name appears once per group, not once per problem — the whole point.
        assert_eq!(out.matches("src/a.ts").count(), 1, "{out}");
    }

    #[test]
    fn diagnostics_keep_a_windows_path_intact() {
        let out = filter_diagnostics("C:\\src\\a.c:12:3: warning: unused\n");

        assert!(out.contains("C:\\src\\a.c (1)"), "{out}");
    }

    #[test]
    fn diagnostics_do_not_treat_a_bare_timestamp_as_a_file() {
        // `12:34:56 something` has the punctuation but no file.
        let out = filter_diagnostics("12:34:56 starting run\n");

        assert!(!out.contains("problem(s)"), "{out}");
    }

    #[test]
    fn diagnostics_keep_a_trailing_score_line() {
        let out = filter_diagnostics("src/a.py:1:0: C0114 missing docstring\nerror: 1 issue\n");

        assert!(out.contains("error: 1 issue"), "{out}");
    }

    #[test]
    fn pkg_keeps_changes_and_drops_fetch_progress() {
        let out = filter_pkg(
            "Get:1 http://deb.debian.org bookworm InRelease [151 kB]\n\
             Get:2 http://deb.debian.org bookworm/main amd64 Packages [8,693 kB]\n\
             Reading package lists...\n\
             Setting up curl (7.88.1-10) ...\n\
             1 upgraded, 0 newly installed, 0 to remove and 4 not upgraded.\n",
        );

        assert!(out.contains("Setting up curl"), "{out}");
        assert!(out.contains("1 upgraded"), "{out}");
        assert!(!out.contains("Get:2"), "{out}");
    }

    #[test]
    fn pkg_falls_back_to_the_tail_when_nothing_notable_happened() {
        let out = filter_pkg("Reading package lists...\nBuilding dependency tree...\nDone\n");

        assert!(out.contains("Done"), "{out}");
    }

    #[test]
    fn native_build_drops_percentage_progress_and_keeps_errors() {
        let out = filter_native_build(
            "-- Detecting C compiler ABI info\n\
             [ 42%] Building CXX object src/CMakeFiles/app.dir/main.cpp.o\n\
             src/main.cpp:12:5: error: no member named 'foo'\n\
             [100%] Built target app\n",
        );

        assert!(out.contains("error: no member named"), "{out}");
        assert!(!out.contains("42%"), "{out}");
        assert!(!out.contains("Detecting C compiler"), "{out}");
    }

    #[test]
    fn js_bundler_keeps_the_failure_and_the_summary() {
        let out = filter_js_bundler(
            "vite v5.0.0 building for production...\n\
             dist/assets/a.js  120.00 kB\n\
             dist/assets/b.js  240.00 kB\n\
             error during build: Could not resolve './missing'\n\
             built in 1.20s\n",
        );

        assert!(out.contains("Could not resolve"), "{out}");
        assert!(out.contains("built in 1.20s"), "{out}");
    }

    #[test]
    fn every_filter_survives_empty_input() {
        for f in [
            filter_diagnostics as fn(&str) -> String,
            filter_pkg,
            filter_native_build,
            filter_js_bundler,
        ] {
            let _ = f("");
        }
    }
}
