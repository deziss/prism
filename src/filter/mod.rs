//! RTK-compatible output filters for 65+ CLI commands used by AI coding agents.
//!
//! One submodule per tool family; `common` holds the shared helpers and the
//! fidelity contract (announced truncation via `[+N more …]`, config-driven caps).

mod cloud;
pub mod common;
mod containers;
mod db;
mod files;
mod golang;
mod iac;
mod js;
mod jvm_misc;
mod misc;
mod python;
mod rules;
mod rust;
mod security;
mod vcs;

pub use common::{has_truncation, limits};
pub use rules::{Rules, loaded as loaded_rules};

use cloud::*;
use common::*;
use containers::*;
use db::*;
use files::*;
use golang::*;
use iac::*;
use js::*;
use jvm_misc::*;
use misc::*;
use python::*;
use rust::*;
use security::*;
use vcs::*;

use std::borrow::Cow;

/// Command names that reach a real filter — the set worth putting a PATH shim in front
/// of. Kept in sync with the dispatch table below by `filtered_tools_matches_dispatch`,
/// which parses this very file, so an added arm without an added name fails the build's
/// test run rather than silently shipping a tool nobody shims.
///
/// `./gradlew` and `./mvnw` are dispatched but deliberately absent: they are relative
/// paths, not PATH lookups, so a shim could never intercept them.
pub const FILTERED_TOOLS: &[&str] = &[
    "git",
    "gh",
    "glab",
    "jj",
    "cargo",
    "nextest",
    "cargo-nextest",
    "pytest",
    "py.test",
    "mypy",
    "pyright",
    "ruff",
    "black",
    "isort",
    "flake8",
    "bandit",
    "pip",
    "uv",
    "poetry",
    "go",
    "golangci-lint",
    "tsc",
    "eslint",
    "biome",
    "prettier",
    "jest",
    "vitest",
    "npm",
    "pnpm",
    "yarn",
    "bun",
    "deno",
    "next",
    "rake",
    "rubocop",
    "rspec",
    "dotnet",
    "gradle",
    "gradlew",
    "mvn",
    "mvnw",
    "make",
    "docker",
    "podman",
    "kubectl",
    "helm",
    "stern",
    "k9s",
    "terraform",
    "tofu",
    "tf",
    "pulumi",
    "aws",
    "gcloud",
    "az",
    "psql",
    "mysql",
    "sqlite3",
    "redis-cli",
    "mongosh",
    "prisma",
    "semgrep",
    "trivy",
    "hadolint",
    "grep",
    "rg",
    "ripgrep",
    "find",
    "fd",
    "ls",
    "eza",
    "exa",
    "jq",
    "act",
    "phpunit",
    "pest",
    "phpstan",
    "composer",
    "sbt",
    "npx",
    "playwright",
    "docker-compose",
    "podman-compose",
    "oc",
    "ansible",
    "ansible-playbook",
    "tree",
    "curl",
    "wget",
    "ping",
    "ps",
    "ss",
    "netstat",
    "df",
    "du",
    "free",
    "systemctl",
    "journalctl",
    "env",
    "printenv",
    "man",
    "lint",
];

/// Dispatch names that intentionally have no shim.
pub const UNSHIMMABLE: &[&str] = &["./gradlew", "./mvnw"];

/// Route `output` of `cmd args…` through the matching filter.
/// Unknown commands get the conservative [`common::generic`] passthrough.
pub fn filter_output<'a>(output: &'a str, cmd: &str, args: &[String]) -> Cow<'a, str> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let cmd = std::path::Path::new(cmd)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(cmd);
    let raw = output; // kept for the never-inflate escape below
    let cleaned = strip_ansi(output);
    let output: &str = &cleaned;
    // User rules that claim precedence run before the built-in table; rules without
    // `override` are consulted from the fallthrough arm instead (see `_` below), so a
    // stray YAML file cannot quietly replace a filter that already parses properly.
    let filtered = match rules::apply(cmd, &a, output, true) {
        Some(out) => out,
        None => match cmd {
            // --- VCS ---
            "git" => filter_git(&a, output),
            "gh" => filter_gh(&a, output),
            "glab" => filter_glab(&a, output),
            "jj" => filter_jj(&a, output),

            // --- Rust ---
            "cargo" => filter_cargo(&a, output),
            "nextest" | "cargo-nextest" => filter_nextest(output),

            // --- Python ---
            "pytest" | "py.test" => filter_pytest(&a, output),
            "mypy" => filter_mypy(output),
            "pyright" => filter_pyright(output),
            "ruff" => filter_ruff(output),
            "black" => filter_black(output),
            "isort" => filter_isort(output),
            "flake8" => filter_flake8(output),
            "bandit" => filter_bandit(output),
            "pip" => filter_pip(&a, output),
            "uv" => filter_uv(&a, output),
            "poetry" => filter_poetry(&a, output),

            // --- Go ---
            "go" => filter_go(&a, output),
            "golangci-lint" => filter_golangci(output),

            // --- JavaScript / TypeScript ---
            "tsc" => filter_tsc(output),
            "eslint" => filter_eslint(output),
            "biome" => filter_biome(&a, output),
            "prettier" => filter_prettier(output),
            "jest" => filter_jest(output),
            "vitest" => filter_vitest(output),
            "npm" | "pnpm" | "yarn" => filter_npm(&a, output),
            "bun" => filter_bun(&a, output),
            "deno" => filter_deno(&a, output),
            "next" => filter_next(&a, output),

            // --- Ruby ---
            "rake" => filter_rake(output),
            "rubocop" => filter_rubocop(output),
            "rspec" => filter_rspec(output),

            // --- .NET ---
            "dotnet" => filter_dotnet(&a, output),

            // --- JVM ---
            "gradle" | "gradlew" | "./gradlew" => filter_gradle(&a, output),
            "mvn" | "mvnw" | "./mvnw" => filter_mvn(output),

            // --- Build ---
            "make" => filter_make(output),

            // --- Containers ---
            "docker" | "podman" => filter_docker(&a, output),

            // --- Kubernetes ---
            "kubectl" => filter_kubectl(&a, output),
            "helm" => filter_helm(&a, output),
            "stern" => filter_stern(output),
            "k9s" => filter_k9s(output),

            // --- IaC ---
            "terraform" | "tofu" | "tf" => filter_terraform(&a, output),
            "pulumi" => filter_pulumi(&a, output),

            // --- Cloud CLIs ---
            "aws" => filter_aws(&a, output),
            "gcloud" => filter_gcloud(&a, output),
            "az" => filter_az(&a, output),

            // --- Databases ---
            "psql" => filter_psql(output),
            "mysql" => filter_mysql(output),
            "sqlite3" => filter_sqlite3(output),
            "redis-cli" => filter_redis(output),
            "mongosh" => filter_mongosh(output),
            "prisma" => filter_prisma(&a, output),

            // --- Security / SAST ---
            "semgrep" => filter_semgrep(output),
            "trivy" => filter_trivy(output),
            "hadolint" => filter_hadolint(output),

            // --- Search / Files ---
            "grep" | "rg" | "ripgrep" => filter_grep(&a, output),
            "find" | "fd" => filter_find(&a, output),
            "ls" | "eza" | "exa" => filter_ls(&a, output),
            "jq" => filter_jq(output),

            // --- CI / local ---
            "act" => filter_act(output),

            // --- PHP / Scala ---
            "phpunit" | "pest" => filter_phpunit(output),
            "phpstan" => filter_phpstan(output),
            "composer" => filter_composer(&a, output),
            "sbt" => filter_sbt(output),

            // --- JS extras ---
            "npx" => filter_npx(&a, output),
            "playwright" => filter_playwright(output),

            // --- Containers extras ---
            "docker-compose" | "podman-compose" => filter_compose(&a, output),
            "oc" => filter_kubectl(&a, output),

            // --- IaC extras ---
            "ansible" | "ansible-playbook" => filter_ansible(output),

            // --- Files extras ---
            "tree" => filter_tree(output),

            // --- Network ---
            "curl" => filter_curl(&a, output),
            "wget" => filter_wget(output),
            "ping" => filter_ping(output),

            // --- System ---
            "ps" => filter_ps(&a, output),
            "ss" | "netstat" => filter_ss(output),
            "df" => filter_df(output),
            "du" => filter_du(output),
            "free" => filter_free(output),
            "systemctl" => filter_systemctl(&a, output),
            "journalctl" => filter_journalctl(output),
            "env" | "printenv" => filter_env(output),
            "man" => filter_man(output),

            // --- Generic ---
            "lint" => filter_lint(output),

            _ => rules::apply(cmd, &a, output, false).unwrap_or_else(|| generic(output)),
        },
    };
    // Never-inflate guarantee: filtering must not cost more than doing nothing.
    //
    // Bytes are the only length we can compare for free, and they are not tokens —
    // regrouping can shed bytes while *adding* tokens, because a token boundary falls
    // wherever punctuation and spacing land. On a short output that risk buys nothing
    // (there was little to save), so a small output has to get materially shorter to be
    // worth reformatting at all; a large one only has to get shorter.
    const SMALL: usize = 2048;
    let worth_it = if raw.len() < SMALL {
        filtered.len() * 10 <= raw.len() * 9 // at least 10% shorter
    } else {
        filtered.len() < raw.len()
    };
    if !worth_it {
        return Cow::Borrowed(raw);
    }
    Cow::Owned(filtered)
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;

    /// Parse the dispatch table out of this file and compare it with `FILTERED_TOOLS`.
    /// A new filter arm whose name nobody added to the list is a tool that installs no
    /// shim, which is invisible at runtime — so make it visible at test time.
    #[test]
    fn filtered_tools_matches_dispatch() {
        let src = include_str!("mod.rs");
        let body = src
            .split("None => match cmd {")
            .nth(1)
            .expect("dispatch table not found")
            .split("_ => rules::apply")
            .next()
            .unwrap();
        let mut dispatched: Vec<String> = Vec::new();
        for line in body.lines() {
            let Some((lhs, _)) = line.split_once("=>") else {
                continue;
            };
            let mut rest = lhs;
            while let Some(open) = rest.find('"') {
                let after = &rest[open + 1..];
                let Some(close) = after.find('"') else { break };
                dispatched.push(after[..close].to_string());
                rest = &after[close + 1..];
            }
        }
        assert!(
            dispatched.len() > 90,
            "parsed only {} arms",
            dispatched.len()
        );

        let listed: std::collections::BTreeSet<&str> = FILTERED_TOOLS.iter().copied().collect();
        let skipped: std::collections::BTreeSet<&str> = UNSHIMMABLE.iter().copied().collect();
        assert_eq!(
            listed.len(),
            FILTERED_TOOLS.len(),
            "FILTERED_TOOLS has duplicates"
        );

        let missing: Vec<&String> = dispatched
            .iter()
            .filter(|n| !listed.contains(n.as_str()) && !skipped.contains(n.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "dispatched but not in FILTERED_TOOLS: {missing:?}"
        );

        let extra: Vec<&&str> = FILTERED_TOOLS
            .iter()
            .filter(|n| !dispatched.iter().any(|d| d == *n))
            .collect();
        assert!(
            extra.is_empty(),
            "in FILTERED_TOOLS but never dispatched: {extra:?}"
        );
    }
}
