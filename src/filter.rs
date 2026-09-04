//! RTK-compatible output filters for 65+ CLI commands used by AI coding agents.

use std::borrow::Cow;

pub fn filter_output<'a>(output: &'a str, cmd: &str, args: &[String]) -> Cow<'a, str> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let filtered = match cmd {
        // --- VCS ---
        "git"                              => filter_git(&a, output),
        "gh"                               => filter_gh(&a, output),
        "glab"                             => filter_glab(&a, output),
        "jj"                               => filter_jj(&a, output),

        // --- Rust ---
        "cargo"                            => filter_cargo(&a, output),
        "nextest" | "cargo-nextest"        => filter_nextest(output),

        // --- Python ---
        "pytest" | "py.test"               => filter_pytest(&a, output),
        "mypy"                             => filter_mypy(output),
        "pyright"                          => filter_pyright(output),
        "ruff"                             => filter_ruff(output),
        "black"                            => filter_black(output),
        "isort"                            => filter_isort(output),
        "flake8"                           => filter_flake8(output),
        "bandit"                           => filter_bandit(output),
        "pip"                              => filter_pip(&a, output),
        "uv"                               => filter_uv(&a, output),
        "poetry"                           => filter_poetry(&a, output),

        // --- Go ---
        "go"                               => filter_go(&a, output),
        "golangci-lint"                    => filter_golangci(output),

        // --- JavaScript / TypeScript ---
        "tsc"                              => filter_tsc(output),
        "eslint"                           => filter_eslint(output),
        "biome"                            => filter_biome(&a, output),
        "prettier"                         => filter_prettier(output),
        "jest"                             => filter_jest(output),
        "vitest"                           => filter_vitest(output),
        "npm" | "pnpm" | "yarn"            => filter_npm(&a, output),
        "bun"                              => filter_bun(&a, output),
        "deno"                             => filter_deno(&a, output),
        "next"                             => filter_next(&a, output),

        // --- Ruby ---
        "rake"                             => filter_rake(output),
        "rubocop"                          => filter_rubocop(output),
        "rspec"                            => filter_rspec(output),

        // --- .NET ---
        "dotnet"                           => filter_dotnet(&a, output),

        // --- JVM ---
        "gradle" | "gradlew" | "./gradlew" => filter_gradle(&a, output),
        "mvn" | "mvnw" | "./mvnw"          => filter_mvn(output),

        // --- Build ---
        "make"                             => filter_make(output),

        // --- Containers ---
        "docker" | "podman"                => filter_docker(&a, output),

        // --- Kubernetes ---
        "kubectl"                          => filter_kubectl(&a, output),
        "helm"                             => filter_helm(&a, output),
        "stern"                            => filter_stern(output),
        "k9s"                              => filter_k9s(output),

        // --- IaC ---
        "terraform" | "tofu" | "tf"        => filter_terraform(&a, output),
        "pulumi"                           => filter_pulumi(&a, output),

        // --- Cloud CLIs ---
        "aws"                              => filter_aws(&a, output),
        "gcloud"                           => filter_gcloud(&a, output),
        "az"                               => filter_az(&a, output),

        // --- Databases ---
        "psql"                             => filter_psql(output),
        "mysql"                            => filter_mysql(output),
        "sqlite3"                          => filter_sqlite3(output),
        "redis-cli"                        => filter_redis(output),
        "mongosh"                          => filter_mongosh(output),
        "prisma"                           => filter_prisma(&a, output),

        // --- Security / SAST ---
        "semgrep"                          => filter_semgrep(output),
        "trivy"                            => filter_trivy(output),
        "hadolint"                         => filter_hadolint(output),

        // --- Search / Files ---
        "grep" | "rg" | "ripgrep"          => filter_grep(output),
        "find" | "fd"                      => filter_find(output),
        "ls" | "eza" | "exa"               => filter_ls(&a, output),
        "jq"                               => filter_jq(output),

        // --- CI / local ---
        "act"                              => filter_act(output),

        // --- Generic ---
        "lint"                             => filter_lint(output),

        _ => return Cow::Borrowed(output),
    };
    Cow::Owned(filtered)
}

// ─── GIT ──────────────────────────────────────────────────────────────────────

fn find_subcommand<'a>(args: &[&'a str]) -> Option<&'a str> {
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        if (arg == "-C" || arg == "-c" || arg == "--git-dir" || arg == "--work-tree" || arg == "--manifest-path") && i + 1 < args.len() {
            i += 2;
            continue;
        }
        if arg.starts_with('-') {
            i += 1;
            continue;
        }
        return Some(arg);
    }
    None
}

fn filter_git(args: &[&str], output: &str) -> String {
    let sub = find_subcommand(args);
    match sub {
        Some("status") => filter_git_status(output),
        Some("diff")   => filter_git_diff(output),
        Some("log")    => filter_git_log(output),
        Some("branch") => filter_git_branch(output),
        Some("remote") => filter_git_remote(output),
        _ => {
            if args.iter().any(|&a| a == "-s" || a == "--short" || a == "status") {
                filter_git_status(output)
            } else {
                output.to_string()
            }
        }
    }
}

fn filter_git_status(output: &str) -> String {
    let mut lines = Vec::new();
    let mut in_untracked = false;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("(use \"git")
            || trimmed.starts_with("no changes added")
            || trimmed.contains("Changes to be committed:")
            || trimmed.contains("Changes not staged") {
            continue;
        }
        if let Some(branch) = trimmed.strip_prefix("On branch ") {
            lines.push(format!("* {}", branch));
            continue;
        }
        if trimmed == "Untracked files:" {
            in_untracked = true;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("modified:") {
            lines.push(format!(" M {}", rest.trim()));
        } else if let Some(rest) = trimmed.strip_prefix("new file:") {
            lines.push(format!(" A {}", rest.trim()));
        } else if let Some(rest) = trimmed.strip_prefix("deleted:") {
            lines.push(format!(" D {}", rest.trim()));
        } else if let Some(rest) = trimmed.strip_prefix("renamed:") {
            lines.push(format!(" R {}", rest.trim()));
        } else if in_untracked {
            lines.push(format!("?? {}", trimmed));
        } else {
            lines.push(line.to_string());
        }
    }
    lines.join("\n")
}

fn filter_git_diff(output: &str) -> String {
    output.lines().filter(|l| {
        !l.starts_with("diff --")
            && !l.starts_with("index ")
            && !l.starts_with("similarity index")
    }).collect::<Vec<_>>().join("\n")
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

// ─── GH ───────────────────────────────────────────────────────────────────────

fn filter_gh(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"pr") | Some(&"issue") | Some(&"run") => output.lines().take(30).collect::<Vec<_>>().join("\n"),
        Some(&"api") => output.lines().take(50).collect::<Vec<_>>().join("\n"),
        _ => output.to_string(),
    }
}

// ─── GLAB (GitLab CLI) ────────────────────────────────────────────────────────

fn filter_glab(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"mr") | Some(&"issue") | Some(&"ci") | Some(&"pipeline") => {
            output.lines().take(30).collect::<Vec<_>>().join("\n")
        }
        _ => output.to_string(),
    }
}

// ─── JJ (Jujutsu) ─────────────────────────────────────────────────────────────

fn filter_jj(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"log") => output.lines().take(20).collect::<Vec<_>>().join("\n"),
        Some(&"diff") => filter_git_diff(output),
        Some(&"status") => output.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n"),
        _ => output.to_string(),
    }
}

// ─── CARGO ────────────────────────────────────────────────────────────────────

fn filter_cargo(args: &[&str], output: &str) -> String {
    match find_subcommand(args) {
        Some("test")    => filter_cargo_test(output),
        Some("nextest") => filter_nextest(output),
        Some("build")   => filter_cargo_build(output),
        Some("check")   => filter_cargo_check(output),
        Some("clippy")  => filter_clippy(output),
        Some("doc")     => filter_cargo_doc(output),
        Some("watch")   => filter_cargo_build(output),
        _ => output.to_string(),
    }
}

fn filter_cargo_test(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let mut out = Vec::new();
    for line in &lines {
        if line.contains("test result:") || line.contains("FAILED") || line.contains("running ") {
            out.push(*line);
        }
    }
    if out.is_empty() {
        output.lines().filter(|l| l.contains("Finished") || l.contains("test")).collect::<Vec<_>>().join("\n")
    } else {
        out.join("\n")
    }
}

fn filter_nextest(output: &str) -> String {
    output.lines().filter(|l| {
        l.contains("FAIL") || l.contains("PASS") || l.contains("test result")
            || l.contains("panicked") || l.contains("error")
    }).collect::<Vec<_>>().join("\n")
}

fn filter_cargo_build(output: &str) -> String {
    let has_issues = output.contains("error[E") || output.contains("warning:") || output.contains("error:");
    if !has_issues {
        output.lines()
            .find(|l| l.contains("Finished"))
            .map(|l| l.trim().to_string())
            .unwrap_or_else(|| "✓ cargo build finished".to_string())
    } else {
        output.lines().filter(|l| {
            l.contains("error") || l.contains("warning") || l.starts_with("  -->") || l.contains("Finished")
        }).collect::<Vec<_>>().join("\n")
    }
}

fn filter_cargo_check(output: &str) -> String {
    let has_issues = output.contains("error[E") || output.contains("warning:") || output.contains("error:");
    if !has_issues {
        output.lines()
            .find(|l| l.contains("Finished"))
            .map(|l| l.trim().to_string())
            .unwrap_or_else(|| "✓ cargo check passed with 0 warnings".to_string())
    } else {
        output.lines().filter(|l| {
            l.contains("error") || l.contains("warning") || l.starts_with("  -->") || l.contains("Finished")
        }).collect::<Vec<_>>().join("\n")
    }
}

fn filter_cargo_doc(output: &str) -> String {
    output.lines().filter(|l| l.contains("error") || l.contains("warning[E")).collect::<Vec<_>>().join("\n")
}

fn filter_clippy(output: &str) -> String {
    output.lines().filter(|l| l.contains("error") || l.contains("warning")).collect::<Vec<_>>().join("\n")
}

// ─── PYTHON ───────────────────────────────────────────────────────────────────

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

fn filter_mypy(output: &str) -> String {
    output.lines().filter(|l| {
        l.contains("error") || l.contains("warning") || l.contains("Found")
    }).collect::<Vec<_>>().join("\n")
}

fn filter_pyright(output: &str) -> String {
    output.lines().filter(|l| {
        l.contains("error") || l.contains("warning") || l.contains("reportM")
            || l.contains("reportG") || l.contains("0 errors")
    }).collect::<Vec<_>>().join("\n")
}

fn filter_ruff(output: &str) -> String {
    output.lines().filter(|l| !l.contains("Found")).collect::<Vec<_>>().join("\n")
}

fn filter_black(output: &str) -> String {
    output.lines().filter(|l| {
        l.contains("reformatted") || l.contains("would reformat")
            || l.contains("error") || l.contains("unchanged")
    }).collect::<Vec<_>>().join("\n")
}

fn filter_isort(output: &str) -> String {
    output.lines().filter(|l| {
        l.contains("ERROR") || l.contains("Fixing") || l.contains("Skipped")
    }).collect::<Vec<_>>().join("\n")
}

fn filter_flake8(output: &str) -> String {
    // flake8 only prints violations — pass through, but strip blank lines
    output.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
}

fn filter_bandit(output: &str) -> String {
    output.lines().filter(|l| {
        l.contains("Issue") || l.contains("Severity") || l.contains("Confidence")
            || l.contains("Total issues") || l.contains("High:")
    }).collect::<Vec<_>>().join("\n")
}

fn filter_pip(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"list")    => filter_pip_list(output),
        Some(&"install") => filter_pip_install(output),
        _ => output.to_string(),
    }
}

fn filter_pip_list(output: &str) -> String {
    output.lines().filter(|l| !l.contains("---") && !l.contains("Package") && !l.trim().is_empty())
        .take(20).collect::<Vec<_>>().join("\n")
}

fn filter_pip_install(output: &str) -> String {
    output.lines().filter(|l| l.contains("Successfully") || l.contains("ERROR")).collect::<Vec<_>>().join("\n")
}

fn filter_uv(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"sync") | Some(&"install") | Some(&"add") | Some(&"remove") => {
            // uv is already terse — filter progress noise
            output.lines().filter(|l| {
                !l.starts_with("Resolved") && !l.starts_with("Downloading") && !l.starts_with("Building")
                    || l.contains("error") || l.contains("warning")
            }).collect::<Vec<_>>().join("\n")
        }
        Some(&"run") => output.lines().rev().take(15).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"),
        Some(&"pip") => filter_pip(args.get(1..).unwrap_or_default(), output),
        _ => output.to_string(),
    }
}

fn filter_poetry(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"install") | Some(&"add") | Some(&"remove") | Some(&"update") => {
            output.lines().filter(|l| {
                l.contains("Installing") || l.contains("Updating") || l.contains("Removing")
                    || l.contains("error") || l.contains("Warning")
            }).collect::<Vec<_>>().join("\n")
        }
        Some(&"run") | Some(&"build") | Some(&"publish") => {
            output.lines().rev().take(10).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")
        }
        _ => output.to_string(),
    }
}

// ─── GO ───────────────────────────────────────────────────────────────────────

fn filter_go(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"test")     => filter_go_test(output),
        Some(&"build")    => filter_go_build(output),
        Some(&"vet")      => filter_go_vet(output),
        Some(&"mod")      => filter_go_mod(args.get(1..).unwrap_or_default(), output),
        Some(&"generate") => output.to_string(),
        Some(&"fmt")      => output.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n"),
        _ => output.to_string(),
    }
}

fn filter_go_test(output: &str) -> String {
    output.lines().filter(|l| {
        l.starts_with("---") || l.contains("FAIL") || l.contains("ok ") || l.contains("panic:")
    }).collect::<Vec<_>>().join("\n")
}

fn filter_go_build(output: &str) -> String {
    output.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
}

fn filter_go_vet(output: &str) -> String {
    output.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
}

fn filter_go_mod(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"tidy") | Some(&"download") => {
            output.lines().filter(|l| l.contains("go:") || l.contains("error")).collect::<Vec<_>>().join("\n")
        }
        _ => output.to_string(),
    }
}

fn filter_golangci(output: &str) -> String {
    output.lines().filter(|l| {
        !l.contains("level=info") && !l.starts_with("time=") && !l.trim().is_empty()
    }).collect::<Vec<_>>().join("\n")
}

// ─── JS / TS ──────────────────────────────────────────────────────────────────

fn filter_tsc(output: &str) -> String {
    output.lines().filter(|l| !l.contains("Found ") && !l.contains("Starting")).collect::<Vec<_>>().join("\n")
}

fn filter_eslint(output: &str) -> String {
    output.lines().filter(|l| !l.contains("Parsing error")).collect::<Vec<_>>().join("\n")
}

fn filter_biome(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"check") | Some(&"lint") | Some(&"format") => {
            output.lines().filter(|l| {
                l.contains("error") || l.contains("warning") || l.contains("Checked")
                    || l.contains("Fixed") || l.contains("Skipped")
            }).collect::<Vec<_>>().join("\n")
        }
        _ => output.to_string(),
    }
}

fn filter_prettier(output: &str) -> String {
    // prettier --check prints only files that differ
    output.lines().filter(|l| !l.trim().is_empty() && !l.contains("Checking formatting...")).collect::<Vec<_>>().join("\n")
}

fn filter_jest(output: &str) -> String {
    output.lines().filter(|l| l.contains("PASS") || l.contains("FAIL") || l.contains("Tests:")).collect::<Vec<_>>().join("\n")
}

fn filter_vitest(output: &str) -> String {
    output.lines().filter(|l| {
        l.contains('✓') || l.contains("✗") || l.contains("FAIL")
            || l.contains("Tests") || l.contains("Duration")
    }).collect::<Vec<_>>().join("\n")
}

fn filter_npm(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"ls") | Some(&"list")  => output.lines().filter(|l| !l.contains("(empty)")).take(30).collect::<Vec<_>>().join("\n"),
        Some(&"test") | Some(&"run") => output.lines().rev().take(10).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"),
        Some(&"install") | Some(&"i") | Some(&"add") => {
            output.lines().filter(|l| {
                l.contains("added") || l.contains("removed") || l.contains("warn") || l.contains("error")
            }).collect::<Vec<_>>().join("\n")
        }
        _ => output.to_string(),
    }
}

fn filter_bun(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"test") => {
            output.lines().filter(|l| {
                l.contains("fail") || l.contains("pass") || l.contains("expect")
                    || l.contains("FAIL") || l.contains("Tests")
            }).collect::<Vec<_>>().join("\n")
        }
        Some(&"install") | Some(&"add") | Some(&"remove") => {
            output.lines().filter(|l| {
                l.contains("installed") || l.contains("removed") || l.contains("error")
            }).collect::<Vec<_>>().join("\n")
        }
        Some(&"build") => filter_cargo_build(output),
        Some(&"run")   => output.lines().rev().take(15).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"),
        _ => output.to_string(),
    }
}

fn filter_deno(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"test") => {
            output.lines().filter(|l| {
                l.contains("FAILED") || l.contains("ok") || l.contains("test result")
                    || l.contains("error")
            }).collect::<Vec<_>>().join("\n")
        }
        Some(&"check") | Some(&"lint") => {
            output.lines().filter(|l| l.contains("error") || l.contains("warning") || l.contains("Checked")).collect::<Vec<_>>().join("\n")
        }
        _ => output.to_string(),
    }
}

fn filter_next(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"build") => {
            output.lines().filter(|l| {
                l.contains("Error") || l.contains("Route") || l.contains("Compiled")
            }).take(30).collect::<Vec<_>>().join("\n")
        }
        _ => output.to_string(),
    }
}

// ─── RUBY ─────────────────────────────────────────────────────────────────────

fn filter_rake(output: &str) -> String {
    output.lines().filter(|l| !l.contains("rake ") && !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
}

fn filter_rubocop(output: &str) -> String {
    output.lines().filter(|l| l.contains("offense") || l.contains("Inspecting")).collect::<Vec<_>>().join("\n")
}

fn filter_rspec(output: &str) -> String {
    output.lines().filter(|l| {
        l.contains("example") || l.contains("failure") || l.contains("Finished")
    }).collect::<Vec<_>>().join("\n")
}

// ─── .NET ─────────────────────────────────────────────────────────────────────

fn filter_dotnet(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"test")  => filter_dotnet_test(output),
        Some(&"build") => filter_dotnet_build(output),
        _ => output.to_string(),
    }
}

fn filter_dotnet_test(output: &str) -> String {
    output.lines().filter(|l| {
        l.contains("Passed") || l.contains("Failed") || l.contains("Total tests")
    }).collect::<Vec<_>>().join("\n")
}

fn filter_dotnet_build(output: &str) -> String {
    output.lines().filter(|l| l.contains("Error") || l.contains("Build")).collect::<Vec<_>>().join("\n")
}

// ─── JVM ──────────────────────────────────────────────────────────────────────

fn filter_gradle(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"test") | Some(&"check") => {
            output.lines().filter(|l| {
                l.contains("FAILED") || l.contains("PASSED") || l.contains("BUILD")
                    || l.contains("tests were") || l.contains("error")
            }).collect::<Vec<_>>().join("\n")
        }
        Some(&"build") | Some(&"assemble") | Some(&"compile") => {
            output.lines().filter(|l| {
                !l.starts_with("> Task") || l.contains("FAILED") || l.contains("error")
            }).collect::<Vec<_>>().join("\n")
        }
        _ => output.lines().filter(|l| {
            l.contains("BUILD") || l.contains("error") || !l.trim().is_empty()
        }).take(40).collect::<Vec<_>>().join("\n"),
    }
}

fn filter_mvn(output: &str) -> String {
    output.lines().filter(|l| {
        l.contains("[ERROR]") || l.contains("[WARNING]") || l.contains("BUILD") || l.contains("Tests run:")
    }).collect::<Vec<_>>().join("\n")
}

// ─── BUILD ────────────────────────────────────────────────────────────────────

fn filter_make(output: &str) -> String {
    output.lines().filter(|l| {
        l.contains("Error") || l.contains("error") || l.contains("warning")
            || l.starts_with("make[") || !l.trim().is_empty()
    }).collect::<Vec<_>>().join("\n")
}

// ─── CONTAINERS ───────────────────────────────────────────────────────────────

fn filter_docker(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"ps")     => filter_docker_ps(output),
        Some(&"images") => filter_docker_images(output),
        Some(&"logs")   => output.lines().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"),
        Some(&"build")  => {
            output.lines().filter(|l| {
                !l.starts_with("Sending build context")
                    && !l.starts_with(" ---> ")
                    && !l.starts_with("Step ")
                    || l.contains("error") || l.contains("ERROR")
            }).collect::<Vec<_>>().join("\n")
        }
        _ => output.to_string(),
    }
}

fn filter_docker_ps(output: &str) -> String {
    output.lines().filter(|l| !l.contains("CONTAINER ID")).take(20).collect::<Vec<_>>().join("\n")
}

fn filter_docker_images(output: &str) -> String {
    output.lines().filter(|l| !l.contains("REPOSITORY")).map(|l| {
        let parts: Vec<&str> = l.split_whitespace().collect();
        if parts.len() >= 3 { format!("{}  {}  {}", parts[0], parts[1], parts[2]) } else { l.to_string() }
    }).collect::<Vec<_>>().join("\n")
}

// ─── KUBERNETES ───────────────────────────────────────────────────────────────

fn filter_kubectl(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"get") | Some(&"describe") | Some(&"top") => {
            output.lines().take(50).collect::<Vec<_>>().join("\n")
        }
        Some(&"logs") => output.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"),
        Some(&"apply") | Some(&"delete") | Some(&"create") => {
            output.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
        }
        _ => output.to_string(),
    }
}

fn filter_helm(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"install") | Some(&"upgrade") | Some(&"uninstall") => {
            output.lines().filter(|l| {
                l.contains("deployed") || l.contains("STATUS") || l.contains("NOTES")
                    || l.contains("error") || l.contains("Error")
            }).collect::<Vec<_>>().join("\n")
        }
        Some(&"lint") => {
            output.lines().filter(|l| l.contains("[ERROR]") || l.contains("[WARNING]") || l.contains("0 chart")).collect::<Vec<_>>().join("\n")
        }
        Some(&"diff") | Some(&"template") => output.lines().take(60).collect::<Vec<_>>().join("\n"),
        Some(&"test") => {
            output.lines().filter(|l| l.contains("PASSED") || l.contains("FAILED") || l.contains("Suite")).collect::<Vec<_>>().join("\n")
        }
        _ => output.to_string(),
    }
}

fn filter_stern(output: &str) -> String {
    // stern tails multi-pod logs — show last 40 lines, strip pod prefix for repeated pods
    output.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")
}

fn filter_k9s(output: &str) -> String {
    output.lines().filter(|l| !l.trim().is_empty()).take(30).collect::<Vec<_>>().join("\n")
}

// ─── IaC ──────────────────────────────────────────────────────────────────────

fn filter_terraform(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"plan") => {
            output.lines().filter(|l| {
                l.contains("Plan:") || l.contains("will be")
                    || l.contains("must be") || l.contains("Error")
                    || l.starts_with("  +") || l.starts_with("  -") || l.starts_with("  ~")
            }).take(80).collect::<Vec<_>>().join("\n")
        }
        Some(&"apply") | Some(&"destroy") => {
            output.lines().filter(|l| {
                l.contains("Apply complete") || l.contains("Destroy complete")
                    || l.contains("Error") || l.contains("created") || l.contains("destroyed")
            }).collect::<Vec<_>>().join("\n")
        }
        Some(&"init") => {
            output.lines().filter(|l| {
                l.contains("Initializing") || l.contains("Installing") || l.contains("Error")
            }).collect::<Vec<_>>().join("\n")
        }
        Some(&"validate") | Some(&"fmt") => {
            output.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
        }
        _ => output.to_string(),
    }
}

fn filter_pulumi(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"preview") | Some(&"up") | Some(&"destroy") => {
            output.lines().filter(|l| {
                l.contains("~") || l.contains("+") || l.contains("-")
                    || l.contains("Resources") || l.contains("error") || l.contains("Duration")
            }).take(60).collect::<Vec<_>>().join("\n")
        }
        _ => output.to_string(),
    }
}

// ─── CLOUD ────────────────────────────────────────────────────────────────────

fn filter_aws(_args: &[&str], output: &str) -> String {
    output.lines().take(50).collect::<Vec<_>>().join("\n")
}

fn filter_gcloud(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"builds") | Some(&"run") | Some(&"compute") => {
            output.lines().filter(|l| {
                l.contains("ERROR") || l.contains("WARNING") || l.contains("status")
            }).take(30).collect::<Vec<_>>().join("\n")
        }
        _ => output.lines().take(40).collect::<Vec<_>>().join("\n"),
    }
}

fn filter_az(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"deployment") | Some(&"group") | Some(&"vm") | Some(&"aks") => {
            output.lines().filter(|l| {
                l.contains("ERROR") || l.contains("Succeeded") || l.contains("Failed")
                    || l.contains("provisioningState")
            }).take(30).collect::<Vec<_>>().join("\n")
        }
        _ => output.lines().take(40).collect::<Vec<_>>().join("\n"),
    }
}

// ─── DATABASES ────────────────────────────────────────────────────────────────

fn filter_psql(output: &str) -> String {
    output.lines().filter(|l| {
        !l.contains("psql (") && !l.contains("Type \"help\"")
    }).collect::<Vec<_>>().join("\n")
}

fn filter_mysql(output: &str) -> String {
    output.lines().filter(|l| !l.starts_with("mysql>") && !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
}

fn filter_sqlite3(output: &str) -> String {
    output.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
}

fn filter_redis(output: &str) -> String {
    // redis-cli output is already terse — strip prompt lines
    output.lines().filter(|l| !l.starts_with("127.0.0.1:") && !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
}

fn filter_mongosh(output: &str) -> String {
    output.lines().filter(|l| {
        !l.contains("Current Mongosh") && !l.contains("Connecting to:")
            && !l.contains("Using MongoDB:") && !l.trim().is_empty()
    }).collect::<Vec<_>>().join("\n")
}

fn filter_prisma(args: &[&str], output: &str) -> String {
    match args.first() {
        Some(&"migrate") | Some(&"db") => filter_prisma_db(output),
        _ => output.to_string(),
    }
}

fn filter_prisma_db(output: &str) -> String {
    output.lines().filter(|l| {
        l.contains("error") || l.contains("migration") || l.contains("applied")
    }).collect::<Vec<_>>().join("\n")
}

// ─── SECURITY / SAST ─────────────────────────────────────────────────────────

fn filter_semgrep(output: &str) -> String {
    output.lines().filter(|l| {
        l.contains("finding") || l.contains("Finding") || l.contains("severity:")
            || l.contains("ERROR") || l.contains("Ran") || l.contains("rule")
    }).collect::<Vec<_>>().join("\n")
}

fn filter_trivy(output: &str) -> String {
    output.lines().filter(|l| {
        l.contains("CRITICAL") || l.contains("HIGH") || l.contains("MEDIUM")
            || l.contains("Total") || l.contains("error")
    }).collect::<Vec<_>>().join("\n")
}

fn filter_hadolint(output: &str) -> String {
    // hadolint only prints violations — pass through, strip blanks
    output.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
}

// ─── SEARCH / FILES ───────────────────────────────────────────────────────────

fn filter_grep(output: &str) -> String {
    output.lines().filter(|l| !l.contains("Binary file")).map(strip_ansi).collect::<Vec<_>>().join("\n")
}

fn filter_find(output: &str) -> String {
    let prune = ["node_modules", ".git", "target", "__pycache__", ".venv", "dist"];
    output.lines().filter(|l| !prune.iter().any(|p| l.contains(p))).collect::<Vec<_>>().join("\n")
}

fn filter_ls(args: &[&str], output: &str) -> String {
    let long_format = args.iter().any(|a| a.starts_with("-l") || *a == "--long");
    if long_format {
        output.lines().filter(|l| !l.starts_with("total ")).map(|l| {
            let parts: Vec<&str> = l.split_whitespace().collect();
            if parts.len() >= 9 { format!("{} {}", parts[5], parts[8]) } else { l.to_string() }
        }).collect::<Vec<_>>().join("\n")
    } else {
        output.to_string()
    }
}

fn filter_jq(output: &str) -> String {
    // jq output is already structured — only truncate if huge
    if output.lines().count() > 100 {
        output.lines().take(100).collect::<Vec<_>>().join("\n") + "\n... (truncated)"
    } else {
        output.to_string()
    }
}

// ─── CI ───────────────────────────────────────────────────────────────────────

fn filter_act(output: &str) -> String {
    output.lines().filter(|l| {
        l.contains("[") || l.contains("error") || l.contains("Error")
            || l.contains("Step") || l.contains("Job") || l.contains("success")
    }).collect::<Vec<_>>().join("\n")
}

// ─── GENERIC ──────────────────────────────────────────────────────────────────

fn filter_lint(output: &str) -> String {
    output.lines().filter(|l| {
        l.contains("error") || l.contains("warning") || l.contains("lint")
    }).collect::<Vec<_>>().join("\n")
}

// ─── UTILS ────────────────────────────────────────────────────────────────────

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max { format!("{}...", &s[..max.saturating_sub(3)]) } else { s.to_string() }
}

fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_escape = false;
    for ch in s.chars() {
        if ch == '\x1b' { in_escape = true; }
        else if in_escape { if ch == 'm' { in_escape = false; } }
        else { result.push(ch); }
    }
    result
}
