//! RTK-compatible output filters for 30+ CLI commands.

use std::borrow::Cow;

// --- main entry point ---
pub fn filter_output<'a>(output: &'a str, cmd: &str, args: &[String]) -> Cow<'a, str> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let filtered = match cmd {
        "git" => filter_git(&a, output),
        "cargo" => filter_cargo(&a, output),
        "pytest" | "py.test" => filter_pytest(&a, output),
        "grep" => filter_grep(output),
        "find" => filter_find(output),
        "ls" => filter_ls(&a, output),
        "tsc" => filter_tsc(output),
        "eslint" => filter_eslint(output),
        "docker" => filter_docker(&a, output),
        "kubectl" => filter_kubectl(&a, output),
        "psql" => filter_psql(output),
        "npm" | "pnpm" => filter_npm(&a, output),
        "aws" => filter_aws(&a, output),
        "gh" => filter_gh(&a, output),
        "dotnet" => filter_dotnet(&a, output),
        "jest" => filter_jest(output),
        "vitest" => filter_vitest(output),
        "mypy" => filter_mypy(output),
        "ruff" => filter_ruff(output),
        "rake" => filter_rake(output),
        "rubocop" => filter_rubocop(output),
        "rspec" => filter_rspec(output),
        "pip" => filter_pip(&a, output),
        "next" => filter_next(&a, output),
        "lint" => filter_lint(output),
        "prisma" => filter_prisma(&a, output),
        _ => return Cow::Borrowed(output),
    };
    Cow::Owned(filtered)
}

// --- git ---
fn filter_git(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"status") | Some(&"-s") | Some(&"--short") => filter_git_status(output),
        Some(&"diff") => filter_git_diff(output),
        Some(&"log") => filter_git_log(output),
        Some(&"branch") => filter_git_branch(output),
        Some(&"remote") => filter_git_remote(output),
        _ => output.to_string(),
    }
}

fn filter_git_status(output: &str) -> String {
    output
        .lines()
        .filter(|l| {
            !l.starts_with("On branch")
                && !l.contains("Changes not staged")
                && !l.contains("no changes added")
                && !l.trim().is_empty()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn filter_git_diff(output: &str) -> String {
    output
        .lines()
        .filter(|l| {
            !l.starts_with("diff --")
                && !l.starts_with("index ")
                && !l.starts_with("similarity index")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn filter_git_log(output: &str) -> String {
    output.lines().take(30).map(|l| truncate(l, 80)).collect::<Vec<_>>().join("\n")
}

fn filter_git_branch(output: &str) -> String {
    output.lines().filter(|l| !l.starts_with("  remotes/")).collect::<Vec<_>>().join("\n")
}

fn filter_git_remote(output: &str) -> String {
    output.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
}

// --- cargo ---
fn filter_cargo(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"test") => filter_cargo_test(output),
        Some(&"build") => filter_cargo_build(output),
        Some(&"check") => filter_cargo_check(output),
        Some(&"clippy") => filter_clippy(output),
        Some(&"doc") => filter_cargo_doc(output),
        _ => output.to_string(),
    }
}

fn filter_cargo_test(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let mut out = Vec::new();
    let mut in_failures = false;

    for line in &lines {
        if line.contains("test result:") || line.contains("FAILED") || line.contains("passed") {
            out.push(line.to_string());
        } else if line.contains("failures:") {
            in_failures = true;
            out.push(line.to_string());
        } else if in_failures {
            if line.starts_with("---- ") || line.contains("assertion") || line.contains("thread") {
                out.push(line.to_string());
            } else if line.trim().is_empty() {
                break;
            }
        }
    }

    if out.is_empty() {
        lines.iter().rev().take(5).map(|l| l.to_string()).collect::<Vec<_>>().join("\n")
    } else {
        out.join("\n")
    }
}

fn filter_cargo_build(output: &str) -> String {
    output
        .lines()
        .filter(|l| {
            !l.contains("Compiling")
                && !l.contains("Finished")
                && !l.contains("Building")
                && !l.trim().is_empty()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn filter_cargo_check(output: &str) -> String {
    output
        .lines()
        .filter(|l| l.contains("error") || l.contains("warning") || !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn filter_cargo_doc(output: &str) -> String {
    output
        .lines()
        .filter(|l| l.contains("error") || l.contains("warning[E"))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- pytest ---
fn filter_pytest(args: &[&str], output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let mut out = Vec::new();
    let failures_only = args.iter().any(|a| a.contains("fail") || a.contains("xunit"));

    for line in &lines {
        if line.contains("FAILED") || line.contains("PASSED") || line.contains("ERROR") {
            out.push(line.to_string());
        } else if line.contains("short test summary") {
            out.push(line.to_string());
        } else if failures_only && !out.is_empty() {
            if line.starts_with("FAILED ") || line.starts_with("  ") || !line.trim().is_empty() {
                out.push(line.to_string());
            }
        } else if line.contains("error") || line.contains("AssertionError") {
            out.push(line.to_string());
        }
    }

    out.join("\n")
}

// --- grep ---
fn filter_grep(output: &str) -> String {
    output
        .lines()
        .filter(|l| !l.contains("Binary file"))
        .map(strip_ansi)
        .collect::<Vec<_>>()
        .join("\n")
}

// --- find ---
fn filter_find(output: &str) -> String {
    let prune = ["node_modules", ".git", "target", "__pycache__", ".venv", "dist"];
    output
        .lines()
        .filter(|l| !prune.iter().any(|p| l.contains(p)))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- ls ---
fn filter_ls(args: &[&str], output: &str) -> String {
    let long_format = args.iter().any(|a| a.starts_with("-l") || *a == "--long");
    if long_format {
        output
            .lines()
            .filter(|l| !l.starts_with("total "))
            .map(|l| {
                let parts: Vec<&str> = l.split_whitespace().collect();
                if parts.len() >= 9 {
                    format!("{} {}", parts[5], parts[8])
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        output.to_string()
    }
}

// --- tsc ---
fn filter_tsc(output: &str) -> String {
    output
        .lines()
        .filter(|l| !l.contains("Found ") && !l.contains("Starting"))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- eslint ---
fn filter_eslint(output: &str) -> String {
    output
        .lines()
        .filter(|l| !l.contains("Parsing error"))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- docker ---
fn filter_docker(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"ps") => filter_docker_ps(output),
        Some(&"images") => filter_docker_images(output),
        _ => output.to_string(),
    }
}

fn filter_docker_ps(output: &str) -> String {
    output.lines().filter(|l| !l.contains("CONTAINER ID")).take(20).collect::<Vec<_>>().join("\n")
}

fn filter_docker_images(output: &str) -> String {
    output
        .lines()
        .filter(|l| !l.contains("REPOSITORY"))
        .map(|l| {
            let parts: Vec<&str> = l.split_whitespace().collect();
            if parts.len() >= 3 {
                format!("{}  {}  {}", parts[0], parts[1], parts[2])
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// --- kubectl ---
fn filter_kubectl(args: &[&str], output: &str) -> String {
    match args.get(1) {
        Some(&"get") | Some(&"describe") | Some(&"logs") | Some(&"top") => {
            output.lines().take(50).collect::<Vec<_>>().join("\n")
        }
        _ => output.to_string(),
    }
}

// --- psql ---
fn filter_psql(output: &str) -> String {
    output
        .lines()
        .filter(|l| !l.contains("psql (") && !l.contains("Type \"help\""))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- npm / pnpm ---
fn filter_npm(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"ls") | Some(&"list") => filter_npm_ls(output),
        Some(&"test") | Some(&"run") => filter_npm_run(output),
        _ => output.to_string(),
    }
}

fn filter_npm_ls(output: &str) -> String {
    output.lines().filter(|l| !l.contains("(empty)")).take(30).collect::<Vec<_>>().join("\n")
}

fn filter_npm_run(output: &str) -> String {
    output.lines().rev().take(10).collect::<Vec<_>>().join("\n")
}

// --- aws ---
fn filter_aws(_args: &[&str], output: &str) -> String {
    output.lines().take(50).collect::<Vec<_>>().join("\n")
}

// --- gh ---
fn filter_gh(_args: &[&str], output: &str) -> String {
    output.lines().take(30).collect::<Vec<_>>().join("\n")
}

// --- dotnet ---
fn filter_dotnet(args: &[&str], output: &str) -> String {
    match args.get(1) {
        Some(&"test") => filter_dotnet_test(output),
        Some(&"build") => filter_dotnet_build(output),
        _ => output.to_string(),
    }
}

fn filter_dotnet_test(output: &str) -> String {
    output
        .lines()
        .filter(|l| l.contains("Passed") || l.contains("Failed") || l.contains("Total tests"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn filter_dotnet_build(output: &str) -> String {
    output
        .lines()
        .filter(|l| l.contains("Error") || l.contains("Build"))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- jest ---
fn filter_jest(output: &str) -> String {
    output
        .lines()
        .filter(|l| l.contains("PASS") || l.contains("FAIL") || l.contains("Tests:"))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- vitest ---
fn filter_vitest(output: &str) -> String {
    output
        .lines()
        .filter(|l| l.contains('✓') || l.contains("✗") || l.contains("FAIL"))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- mypy ---
fn filter_mypy(output: &str) -> String {
    output
        .lines()
        .filter(|l| l.contains("error") || l.contains("warning") || l.contains("Found"))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- ruff ---
fn filter_ruff(output: &str) -> String {
    output.lines().filter(|l| !l.contains("Found")).collect::<Vec<_>>().join("\n")
}

// --- rake ---
fn filter_rake(output: &str) -> String {
    output.lines().filter(|l| !l.contains("rake ") && !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
}

// --- rubocop ---
fn filter_rubocop(output: &str) -> String {
    output
        .lines()
        .filter(|l| l.contains("offense") || l.contains("Inspecting"))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- rspec ---
fn filter_rspec(output: &str) -> String {
    output
        .lines()
        .filter(|l| l.contains("example") || l.contains("failure") || l.contains("Finished"))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- pip ---
fn filter_pip(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"list") => filter_pip_list(output),
        Some(&"install") => filter_pip_install(output),
        _ => output.to_string(),
    }
}

fn filter_pip_list(output: &str) -> String {
    output
        .lines()
        .filter(|l| !l.contains("---") && !l.contains("Package") && !l.trim().is_empty())
        .take(20)
        .collect::<Vec<_>>()
        .join("\n")
}

fn filter_pip_install(output: &str) -> String {
    output
        .lines()
        .filter(|l| l.contains("Successfully") || l.contains("ERROR"))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- next ---
fn filter_next(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"build") => filter_next_build(output),
        _ => output.to_string(),
    }
}

fn filter_next_build(output: &str) -> String {
    output
        .lines()
        .filter(|l| l.contains("Error") || l.contains("Route") || l.contains("Compiled"))
        .take(30)
        .collect::<Vec<_>>()
        .join("\n")
}

// --- lint ---
fn filter_lint(output: &str) -> String {
    output
        .lines()
        .filter(|l| l.contains("error") || l.contains("warning") || l.contains("lint"))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- prisma ---
fn filter_prisma(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"migrate") | Some(&"db") => filter_prisma_db(output),
        _ => output.to_string(),
    }
}

fn filter_prisma_db(output: &str) -> String {
    output
        .lines()
        .filter(|l| l.contains("error") || l.contains("migration") || l.contains("applied"))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- clippy ---
fn filter_clippy(output: &str) -> String {
    output
        .lines()
        .filter(|l| l.contains("error") || l.contains("warning"))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- utils ---
fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max.saturating_sub(3)])
    } else {
        s.to_string()
    }
}

fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_escape = false;
    for ch in s.chars() {
        if ch == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if ch == 'm' {
                in_escape = false;
            }
        } else {
            result.push(ch);
        }
    }
    result
}
