//! User-defined filter routing — `~/.config/prism/filters/*.yaml`.
//!
//! A rule file declares *which* primitive handles a tool's subcommand. It never
//! expresses the parsing itself: every [`Shape`] below is an existing Rust primitive, so
//! a rule inherits the fidelity contract for free — cuts still emit `[+N more …]`, caps
//! still come from `FilterLimits`, and `prism cmd` still tees when a marker appears.
//!
//! That boundary is the whole design. A regex keep/drop language would be more
//! expressive and strictly worse: regex can decide to drop a line, but it cannot *count*
//! what it dropped, and every marker in this crate is arithmetic over parsed structure.
//! So YAML buys the long tail — a new tool that prints a shape we already handle — and a
//! genuinely new shape still costs Rust.
//!
//! ```yaml
//! tool: nomad
//! subcommands:
//!   status:      { shape: table, cap: list_max_lines }
//!   alloc-logs:  { shape: logs }
//!   job-inspect: { shape: json }
//!   run:         { shape: verb-group, verbs: [started, updated] }
//! default:       { shape: generic }
//! ```

use super::common::*;
use super::containers::{fold_logs, kube_describe, pruned_json, split_glog, squeeze_table};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

/// The primitives a rule may name. Closed on purpose: an unknown `shape` is a load
/// error, not a silent passthrough, because a config typo that quietly does nothing is
/// worse than one that says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shape {
    /// Whitespace-squeezed rows, capped, `[+N more rows]`.
    Table,
    /// Repeated log templates folded to `(×N)`, then tailed.
    Logs,
    /// Re-emit JSON with values pruned; falls back to `table` when it will not parse.
    Json,
    /// `Key: value` report blocks — keeps the keys, drops empty and default-valued rows.
    Describe,
    /// Indented tree output flattened to paths.
    Tree,
    /// `"<object> <verb>"` lines grouped by their trailing verb.
    VerbGroup,
    /// Drop consecutive duplicate lines, then cap.
    Dedupe,
    /// Keep the last `cap` lines.
    Tail,
    /// Keep only lines that look like an error or warning.
    Errors,
    /// The conservative fallback: ANSI stripped, blank runs collapsed, capped.
    Generic,
    /// Emit the output untouched. Use to *exempt* a subcommand whose output is already
    /// minimal, so the enclosing `default` does not reformat it.
    Raw,
}

impl Shape {
    /// The `FilterLimits` field a shape uses when the rule does not name one.
    fn default_cap(self) -> &'static str {
        match self {
            Shape::Table | Shape::Tree | Shape::VerbGroup => "list_max_lines",
            Shape::Logs | Shape::Tail => "log_tail",
            Shape::Json => "json_max_lines",
            Shape::Dedupe => "passthrough_max_lines",
            Shape::Errors => "max_diagnostics",
            Shape::Describe | Shape::Generic | Shape::Raw => "",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub shape: Shape,
    /// Name of a `FilterLimits` field, e.g. `list_max_lines`. Defaults per shape.
    #[serde(default)]
    pub cap: Option<String>,
    /// `verb-group` only: the trailing words that mark a groupable line.
    #[serde(default)]
    pub verbs: Vec<String>,
    /// Strip a leading glog/klog storm (`E0102 12:00:00.000 1 file.go:12] msg`) and fold
    /// it before applying `shape` to the rest.
    #[serde(default)]
    pub strip_glog: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub tool: String,
    #[serde(default)]
    pub subcommands: BTreeMap<String, Action>,
    /// Applied when no `subcommands` entry matches.
    #[serde(default)]
    pub default: Option<Action>,
    /// Take precedence over a built-in Rust filter for this tool. Without it a rule only
    /// applies where the built-in dispatch would have fallen through to `generic`, so a
    /// stray file cannot silently downgrade a filter that already parses properly.
    #[serde(default)]
    pub r#override: bool,
}

#[derive(Debug, Default)]
pub struct Rules {
    by_tool: BTreeMap<String, Rule>,
    /// Load failures, kept so they can be reported instead of swallowed.
    pub errors: Vec<String>,
}

fn rules_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("PRISM_FILTER_RULES_DIR") {
        let d = d.trim();
        if d.is_empty() {
            return None;
        }
        return Some(PathBuf::from(d));
    }
    dirs::config_dir().map(|c| c.join("prism").join("filters"))
}

/// Parse every `*.yaml` in `dir`. A bad file is recorded and skipped — one typo must not
/// disable the rules that do parse.
pub fn load_from(dir: &std::path::Path) -> Rules {
    let mut rules = Rules::default();
    let Ok(read) = std::fs::read_dir(dir) else {
        return rules;
    };
    let mut paths: Vec<PathBuf> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            matches!(p.extension().and_then(|e| e.to_str()), Some("yaml") | Some("yml"))
        })
        .collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("?").to_string();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                rules.errors.push(format!("{name}: {e}"));
                continue;
            }
        };
        match serde_yaml::from_str::<Rule>(&text) {
            Ok(rule) => {
                if rule.tool.trim().is_empty() {
                    rules.errors.push(format!("{name}: `tool` is empty"));
                    continue;
                }
                if let Some(bad) = bad_cap(&rule) {
                    rules.errors.push(format!("{name}: unknown cap `{bad}`"));
                    continue;
                }
                rules.by_tool.insert(rule.tool.trim().to_string(), rule);
            }
            Err(e) => rules.errors.push(format!("{name}: {e}")),
        }
    }
    rules
}

/// First `cap` in the rule that does not name a `FilterLimits` field.
fn bad_cap(rule: &Rule) -> Option<String> {
    let l = limits();
    rule.subcommands
        .values()
        .chain(rule.default.iter())
        .filter_map(|a| a.cap.as_deref())
        .find(|c| cap_by_name(l, c).is_none())
        .map(String::from)
}

impl Rules {
    /// Tools that have a rule, sorted.
    pub fn tools(&self) -> Vec<&str> {
        self.by_tool.keys().map(String::as_str).collect()
    }
}

pub fn loaded() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| match rules_dir() {
        Some(d) => load_from(&d),
        None => Rules::default(),
    })
}

/// Resolve a `FilterLimits` field by name. Rules reference caps by the same names the
/// `filters:` config block and the `PRISM_FILTER_*` variables use.
fn cap_by_name(l: &Limits, name: &str) -> Option<usize> {
    Some(match name {
        "grep_max_per_file" => l.grep_max_per_file,
        "grep_max_results" => l.grep_max_results,
        "grep_line_width" => l.grep_line_width,
        "find_max_per_dir" => l.find_max_per_dir,
        "find_max_dirs" => l.find_max_dirs,
        "ls_max_entries" => l.ls_max_entries,
        "list_max_lines" => l.list_max_lines,
        "json_max_lines" => l.json_max_lines,
        "json_max_array" => l.json_max_array,
        "log_tail" => l.log_tail,
        "passthrough_max_lines" => l.passthrough_max_lines,
        "diff_context" => l.diff_context,
        "status_max_files" => l.status_max_files,
        "test_max_failures" => l.test_max_failures,
        "max_diagnostics" => l.max_diagnostics,
        "ps_max_rows" => l.ps_max_rows,
        _ => return None,
    })
}

/// Route `output` through the rule for `cmd`, if one applies.
///
/// `overrides_only` asks for rules that claim precedence over a built-in filter; it is
/// checked before the built-in dispatch table. The same function is called again from
/// the table's fallthrough arm with `overrides_only = false`, which is where a rule for a
/// tool prism has no Rust filter for takes effect.
pub fn apply(cmd: &str, args: &[&str], output: &str, overrides_only: bool) -> Option<String> {
    apply_with(loaded(), cmd, args, output, overrides_only)
}

pub fn apply_with(
    rules: &Rules,
    cmd: &str,
    args: &[&str],
    output: &str,
    overrides_only: bool,
) -> Option<String> {
    let rule = rules.by_tool.get(cmd)?;
    if overrides_only != rule.r#override {
        return None;
    }
    let action = match find_subcommand(args) {
        Some(sub) => rule.subcommands.get(sub).or(rule.default.as_ref()),
        None => rule.default.as_ref(),
    }?;
    Some(run(action, output))
}

fn run(action: &Action, output: &str) -> String {
    let l = limits();
    let cap = action
        .cap
        .as_deref()
        .and_then(|c| cap_by_name(l, c))
        .or_else(|| cap_by_name(l, action.shape.default_cap()))
        .unwrap_or(l.passthrough_max_lines);

    if action.strip_glog {
        let (glog, rest) = split_glog(output);
        if !glog.is_empty() {
            let mut out = glog;
            let tail = shape(action, &rest, cap);
            if !tail.trim().is_empty() {
                out.push(tail);
            }
            return out.join("\n");
        }
    }
    shape(action, output, cap)
}

fn shape(action: &Action, output: &str, cap: usize) -> String {
    match action.shape {
        Shape::Table => squeeze_table(output, cap),
        Shape::Logs => fold_logs(output, cap),
        Shape::Json => pruned_json(output, |o| squeeze_table(o, cap)),
        Shape::Describe => kube_describe(output),
        Shape::Tree => {
            let lines: Vec<String> = output
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(flatten_tree)
                .collect();
            cap_vec(lines, cap, "entries").join("\n")
        }
        Shape::VerbGroup => {
            let verbs: Vec<&str> = action.verbs.iter().map(String::as_str).collect();
            let (mut out, other) = group_by_verb(output, &verbs, cap);
            out.extend(cap_vec(other, limits().max_diagnostics, "lines"));
            out.join("\n")
        }
        Shape::Dedupe => {
            let lines = dedupe_consecutive(
                output.lines().filter(|l| !l.trim().is_empty()).map(String::from),
            );
            cap_vec(lines, cap, "lines").join("\n")
        }
        Shape::Tail => tail_lines(output, cap, "lines"),
        Shape::Errors => {
            let lines: Vec<String> = output
                .lines()
                .filter(|l| is_alert_line(l))
                .map(|l| squeeze_ws(l))
                .collect();
            if lines.is_empty() {
                return generic(output);
            }
            cap_vec(lines, cap, "diagnostics").join("\n")
        }
        Shape::Generic => generic(output),
        Shape::Raw => output.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules_from(files: &[(&str, &str)]) -> (tempdir::Dir, Rules) {
        let dir = tempdir::Dir::new("prism-rules");
        for (name, body) in files {
            std::fs::write(dir.path().join(name), body).unwrap();
        }
        let r = load_from(dir.path());
        (dir, r)
    }

    /// Minimal scoped temp dir — no dev-dependency needed for four tests.
    mod tempdir {
        use std::path::{Path, PathBuf};
        pub struct Dir(PathBuf);
        impl Dir {
            pub fn new(tag: &str) -> Self {
                let p = std::env::temp_dir().join(format!(
                    "{tag}-{}-{:?}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::create_dir_all(&p).unwrap();
                Dir(p)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn a_rule_routes_a_subcommand_to_a_primitive() {
        let (_d, r) = rules_from(&[(
            "nomad.yaml",
            "tool: nomad\nsubcommands:\n  status: { shape: table, cap: list_max_lines }\n",
        )]);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let out = apply_with(&r, "nomad", &["status"], "ID    Type\nweb   service\n", false)
            .expect("rule should apply");
        assert_eq!(out, "ID Type\nweb service");
    }

    #[test]
    fn a_rule_without_override_does_not_preempt_a_builtin() {
        let (_d, r) =
            rules_from(&[("git.yaml", "tool: git\ndefault: { shape: generic }\n")]);
        // The dispatch table asks for overrides first; this rule must decline.
        assert!(apply_with(&r, "git", &["status"], "x", true).is_none());
        // …and only take effect in the fallthrough position.
        assert!(apply_with(&r, "git", &["status"], "x", false).is_some());
    }

    #[test]
    fn an_override_rule_wins_before_the_builtin() {
        let (_d, r) = rules_from(&[(
            "git.yaml",
            "tool: git\noverride: true\ndefault: { shape: raw }\n",
        )]);
        assert_eq!(apply_with(&r, "git", &["status"], "kept\n", true).as_deref(), Some("kept\n"));
    }

    #[test]
    fn a_typo_is_a_reported_error_not_a_silent_no_op() {
        let (_d, r) = rules_from(&[
            ("bad-shape.yaml", "tool: a\ndefault: { shape: tabel }\n"),
            ("bad-cap.yaml", "tool: b\ndefault: { shape: table, cap: no_such_limit }\n"),
            ("bad-key.yaml", "tool: c\ndefualt: { shape: table }\n"),
            ("ok.yaml", "tool: d\ndefault: { shape: table }\n"),
        ]);
        assert_eq!(r.errors.len(), 3, "{:?}", r.errors);
        assert!(r.errors.iter().any(|e| e.contains("no_such_limit")));
        // the valid file still loaded
        assert!(apply_with(&r, "d", &[], "a b\n", false).is_some());
    }

    #[test]
    fn verb_group_folds_object_lists_and_keeps_the_count() {
        let (_d, r) = rules_from(&[(
            "nomad.yaml",
            "tool: nomad\nsubcommands:\n  run: { shape: verb-group, verbs: [started, updated] }\n",
        )]);
        let out = apply_with(
            &r,
            "nomad",
            &["run"],
            "job/web started\njob/api started\njob/db updated\nsomething odd\n",
            false,
        )
        .unwrap();
        assert!(out.contains("started (2): job/web, job/api"), "{out}");
        assert!(out.contains("updated (1): job/db"), "{out}");
        assert!(out.contains("something odd"), "leftovers must survive: {out}");
    }

    #[test]
    fn raw_exempts_a_subcommand_from_the_default() {
        let (_d, r) = rules_from(&[(
            "nomad.yaml",
            "tool: nomad\nsubcommands:\n  version: { shape: raw }\ndefault: { shape: generic }\n",
        )]);
        assert_eq!(
            apply_with(&r, "nomad", &["version"], "Nomad v1.2.3\n", false).as_deref(),
            Some("Nomad v1.2.3\n")
        );
    }

    #[test]
    fn a_missing_rules_dir_is_not_an_error() {
        let r = load_from(std::path::Path::new("/nonexistent/prism/filters"));
        assert!(r.errors.is_empty());
        assert!(apply_with(&r, "anything", &[], "x", false).is_none());
    }
}
