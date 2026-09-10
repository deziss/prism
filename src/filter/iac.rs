//! Infrastructure-as-code filters: `terraform`/`tofu`, `pulumi`, `ansible`.
//!
//! Plans keep the per-resource actions and the attribute diffs that drive them;
//! refresh chatter, `known after apply` filler and the `│` warning frames go away.

use super::common::*;

fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{} {}", n, noun)
    } else {
        format!("{} {}s", n, noun)
    }
}

/// What a line contributes once terraform's `│ ` diagnostic frame is removed.
enum Gut {
    /// `╷` / `╵` — the frame closed, so the diagnostic block ended.
    Border,
    /// A blank line inside the frame: paragraph break, block continues.
    Blank,
    Text(String),
}

fn ungutter(line: &str) -> Gut {
    let t = line.trim();
    if t.starts_with('╷') || t.starts_with('╵') || t.starts_with('└') {
        return Gut::Border;
    }
    match t.strip_prefix('│') {
        Some(rest) => {
            let r = rest.trim();
            if r.is_empty() {
                Gut::Blank
            } else {
                Gut::Text(r.to_string())
            }
        }
        None if t.is_empty() => Gut::Blank,
        None => Gut::Text(t.to_string()),
    }
}

const TF_PROGRESS: [&str; 8] = [
    "Refreshing state...",
    "Reading...",
    "Read complete after",
    "Still creating...",
    "Still destroying...",
    "Still reading...",
    "Still modifying...",
    "Preparing the remote plan",
];

fn tf_action(line: &str) -> Option<(&'static str, String)> {
    // `  # aws_instance.web will be created`
    let t = line.trim();
    let rest = t.strip_prefix("# ")?;
    let (name, tail) = rest
        .split_once(" will be ")
        .or_else(|| rest.split_once(" must be "))?;
    let action = match tail.trim().trim_end_matches(':') {
        "created" => "create",
        "destroyed" => "destroy",
        "updated in-place" => "update",
        "replaced" => "replace",
        "read during apply" => "read",
        other if other.starts_with("replaced") => "replace",
        _ => return None,
    };
    Some((action, name.trim().to_string()))
}

fn tf_plan(output: &str) -> String {
    let l = limits();
    let mut groups: Vec<(&'static str, Vec<String>)> = Vec::new();
    let mut attrs: Vec<String> = Vec::new(); // change lines of the current resource
    let mut current: Option<String> = None;
    let mut diags: Vec<String> = Vec::new();
    let mut trailer: Option<String> = None;
    let mut known_after = 0usize;
    let mut progress = 0usize;
    let mut in_diag = false;
    let mut detail: Vec<String> = Vec::new();

    for raw in output.lines() {
        let t = match ungutter(raw) {
            Gut::Border => {
                in_diag = false;
                continue;
            }
            Gut::Blank => continue,
            Gut::Text(t) => t,
        };
        if TF_PROGRESS.iter().any(|p| t.contains(p)) {
            progress += 1;
            continue;
        }
        if t.starts_with("Error:") || t.starts_with("Warning:") {
            in_diag = true;
            diags.push(t.clone());
            continue;
        }
        if in_diag {
            diags.push(format!("  {}", truncate(&squeeze_ws(&t), 200)));
            continue;
        }
        if t.starts_with("Plan:")
            || t.starts_with("No changes.")
            || t.starts_with("Apply complete!")
            || t.starts_with("Destroy complete!")
            || t.starts_with("Changes to Outputs")
        {
            trailer = Some(squeeze_ws(&t));
            continue;
        }
        if let Some((action, name)) = tf_action(&t) {
            if let Some(prev) = current.take() {
                if !attrs.is_empty() {
                    detail.push(format!("{}:", prev));
                    detail.append(&mut attrs);
                }
            }
            current = Some(name.clone());
            match groups.iter_mut().find(|(a, _)| *a == action) {
                Some((_, v)) => v.push(name),
                None => groups.push((action, vec![name])),
            }
            continue;
        }
        // attribute changes inside a resource block
        let is_change =
            t.starts_with('~') || t.starts_with('+') || t.starts_with('-') || t.starts_with("+/-");
        if is_change {
            if t.contains("(known after apply)") {
                known_after += 1;
                continue;
            }
            if attrs.len() < 12 {
                attrs.push(format!("  {}", truncate(&squeeze_ws(&t), 160)));
            }
            continue;
        }
        if t.contains("forces replacement") {
            attrs.push(format!("  {}", squeeze_ws(&t)));
        }
    }
    if let Some(prev) = current {
        if !attrs.is_empty() {
            detail.push(format!("{}:", prev));
            detail.append(&mut attrs);
        }
    }

    let mut out: Vec<String> = Vec::new();
    for (action, names) in &groups {
        let n = names.len();
        let shown = n.min(l.list_max_lines);
        let mut line = format!("{} ({}): {}", action, n, names[..shown].join(", "));
        if n > shown {
            line.push_str(&format!(", {}", more(n - shown, "resources")));
        }
        out.push(line);
    }
    out.extend(cap_vec(detail, l.max_diagnostics, "attribute lines"));
    if known_after > 0 {
        out.push(format!("({} attributes known after apply)", known_after));
    }
    out.extend(cap_vec(diags, l.max_diagnostics, "diagnostic lines"));
    if let Some(t) = trailer {
        out.push(t);
    } else if out.is_empty() {
        out.push(format!(
            "terraform: no changes ({} refresh steps)",
            progress
        ));
    }
    out.join("\n")
}

fn tf_apply(output: &str) -> String {
    let l = limits();
    let mut done: Vec<(String, Vec<String>)> = Vec::new(); // verb → resources
    let mut diags: Vec<String> = Vec::new();
    let mut outputs: Vec<String> = Vec::new();
    let mut trailer: Option<String> = None;
    let mut progress = 0usize;
    let mut in_outputs = false;
    let mut in_diag = false;

    for raw in output.lines() {
        let t = match ungutter(raw) {
            Gut::Border => {
                in_diag = false;
                continue;
            }
            Gut::Blank => continue,
            Gut::Text(t) => t,
        };
        if TF_PROGRESS.iter().any(|p| t.contains(p)) {
            progress += 1;
            continue;
        }
        if t.starts_with("Error:") || t.starts_with("Warning:") {
            in_diag = true;
            diags.push(t.clone());
            continue;
        }
        if in_diag {
            diags.push(format!("  {}", truncate(&squeeze_ws(&t), 200)));
            continue;
        }
        if t.starts_with("Apply complete!") || t.starts_with("Destroy complete!") {
            trailer = Some(squeeze_ws(&t));
            continue;
        }
        if t.starts_with("Outputs:") {
            in_outputs = true;
            continue;
        }
        if in_outputs {
            outputs.push(squeeze_ws(&t));
            continue;
        }
        // `aws_instance.web: Creation complete after 32s [id=i-abc]`
        if let Some((res, tail)) = t.split_once(": ") {
            let verb = tail.split(" complete").next().unwrap_or("");
            if tail.contains(" complete") {
                let key = verb.to_ascii_lowercase();
                match done.iter_mut().find(|(v, _)| *v == key) {
                    Some((_, v)) => v.push(res.to_string()),
                    None => done.push((key, vec![res.to_string()])),
                }
                continue;
            }
            if tail.contains("...") {
                progress += 1;
                continue;
            }
        }
    }
    let mut out: Vec<String> = Vec::new();
    for (verb, res) in &done {
        let n = res.len();
        let shown = n.min(l.list_max_lines);
        let mut line = format!("{} ({}): {}", verb, n, res[..shown].join(", "));
        if n > shown {
            line.push_str(&format!(", {}", more(n - shown, "resources")));
        }
        out.push(line);
    }
    if !outputs.is_empty() {
        out.push("outputs:".to_string());
        out.extend(cap_vec(outputs, l.list_max_lines, "outputs"));
    }
    out.extend(cap_vec(diags, l.max_diagnostics, "diagnostic lines"));
    if let Some(t) = trailer {
        out.push(t);
    }
    if out.is_empty() {
        out.push(format!("terraform: {} steps", progress));
    }
    out.join("\n")
}

pub(crate) fn filter_terraform(args: &[&str], output: &str) -> String {
    let l = limits();
    match find_subcommand(args) {
        Some("plan") => tf_plan(output),
        Some("apply") | Some("destroy") => tf_apply(output),
        Some("init") => {
            let mut providers: Vec<String> = Vec::new();
            let mut diags: Vec<String> = Vec::new();
            for raw in output.lines() {
                let t = match ungutter(raw) {
                    Gut::Text(t) => t,
                    _ => continue,
                };
                if let Some(p) = t.strip_prefix("- Installing ") {
                    providers.push(p.trim_end_matches("...").trim().to_string());
                    continue;
                }
                if t.starts_with("Error:") || t.starts_with("Warning:") || t.contains("must be") {
                    diags.push(squeeze_ws(&t));
                }
            }
            let mut out = Vec::new();
            if !providers.is_empty() {
                out.push(format!(
                    "terraform init: ok (providers: {})",
                    providers.join(", ")
                ));
            }
            out.extend(cap_vec(diags, l.max_diagnostics, "diagnostic lines"));
            if out.is_empty() {
                out.push("terraform init: ok".to_string());
            }
            out.join("\n")
        }
        Some("validate") => {
            let diags: Vec<String> = output
                .lines()
                .filter_map(|l| match ungutter(l) {
                    Gut::Text(t) => Some(t),
                    _ => None,
                })
                .filter(|t| !t.starts_with("Success!"))
                .map(|t| squeeze_ws(&t))
                .collect();
            if diags.is_empty() {
                "terraform validate: ok".to_string()
            } else {
                cap_vec(diags, l.max_diagnostics, "lines").join("\n")
            }
        }
        Some("fmt") => {
            // plain `fmt` lists paths; `-diff` adds unified diffs
            let mut files: Vec<String> = Vec::new();
            let mut changes: Vec<String> = Vec::new();
            for line in output.lines() {
                let t = line.trim_end();
                let tt = t.trim();
                if tt.is_empty() {
                    continue;
                }
                if tt.starts_with("---") || tt.starts_with("+++") || tt.starts_with("@@") {
                    continue;
                }
                if tt.starts_with('+') || tt.starts_with('-') {
                    changes.push(truncate(tt, 120));
                    continue;
                }
                files.push(tt.to_string());
            }
            if files.is_empty() && changes.is_empty() {
                return "terraform fmt: ok".to_string();
            }
            let mut out = vec![format!(
                "terraform fmt: {} need formatting",
                plural(files.len(), "file")
            )];
            out.extend(cap_vec(files, l.list_max_lines, "files"));
            out.extend(cap_vec(changes, l.max_diagnostics, "diff lines"));
            out.join("\n")
        }
        Some("output") => compact_json_output(output, |o| {
            cap_lines(collapse_blank(o).lines(), l.list_max_lines, "lines")
        }),
        Some("show") | Some("state") | Some("providers") | Some("graph") | Some("workspace") => {
            let body: Vec<String> = output
                .lines()
                .map(|l| l.trim_end())
                .filter(|t| !t.trim().is_empty() && !t.trim().starts_with('#'))
                .map(flatten_tree)
                .collect();
            cap_vec(body, l.list_max_lines, "lines").join("\n")
        }
        Some("version") => {
            // drop the "your version is out of date" paragraph, keep the hint
            let mut out: Vec<String> = Vec::new();
            let mut newer: Option<String> = None;
            for line in output.lines() {
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                // the "out of date" notice wraps across lines; keep only the facts
                if t.contains("out of date")
                    || t.starts_with("is ")
                    || t.contains("The latest version")
                {
                    if let Some(v) = t
                        .split("is ")
                        .nth(1)
                        .and_then(|r| r.split_whitespace().next())
                    {
                        let v = v.trim_end_matches('.');
                        if v.chars()
                            .next()
                            .map(|c| c.is_ascii_digit())
                            .unwrap_or(false)
                        {
                            newer = Some(v.to_string());
                        }
                    }
                    continue;
                }
                out.push(squeeze_ws(t));
            }
            if let Some(v) = newer {
                out.push(format!("(update available: {})", v));
            }
            out.join("\n")
        }
        _ => generic(output),
    }
}

pub(crate) fn filter_pulumi(args: &[&str], output: &str) -> String {
    let l = limits();
    match find_subcommand(args) {
        Some("preview") | Some("up") | Some("destroy") | Some("refresh") => {
            let mut rows: Vec<String> = Vec::new();
            let mut diags: Vec<String> = Vec::new();
            let mut outputs: Vec<String> = Vec::new();
            let mut trailer: Vec<String> = Vec::new();
            let mut url: Option<String> = None;
            let mut section = "";
            for line in output.lines() {
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                if t.starts_with("View Live:") || t.starts_with("View in Browser") {
                    url = t.split_whitespace().last().map(String::from);
                    continue;
                }
                if t.starts_with("Diagnostics:") {
                    section = "diag";
                    continue;
                }
                if t.starts_with("Outputs:") {
                    section = "out";
                    continue;
                }
                if t.starts_with("Resources:")
                    || t.starts_with("Duration:")
                    || t.starts_with("Updating")
                {
                    section = "";
                    trailer.push(squeeze_ws(t));
                    continue;
                }
                match section {
                    "diag" => diags.push(squeeze_ws(t)),
                    "out" => outputs.push(squeeze_ws(t)),
                    _ => {
                        if t.starts_with("Type ")
                            || t.contains("pulumi:pulumi:Stack")
                            || t.starts_with("Previewing")
                        {
                            continue;
                        }
                        rows.push(squeeze_ws(t));
                    }
                }
            }
            let mut out = cap_vec(rows, l.list_max_lines, "rows");
            if !outputs.is_empty() {
                out.push("outputs:".to_string());
                out.extend(cap_vec(outputs, l.list_max_lines, "outputs"));
            }
            out.extend(cap_vec(diags, l.max_diagnostics, "diagnostic lines"));
            out.extend(trailer);
            if let Some(u) = url {
                out.push(u);
            }
            out.join("\n")
        }
        Some("stack") | Some("config") | Some("about") => {
            cap_lines(collapse_blank(output).lines(), l.list_max_lines, "lines")
        }
        _ => generic(output),
    }
}

pub(crate) fn filter_ansible(output: &str) -> String {
    let l = limits();
    struct Task {
        name: String,
        ok: usize,
        changed: usize,
        skipped: usize,
        unreachable: usize,
        failed: Vec<String>,
    }
    let mut tasks: Vec<Task> = Vec::new();
    let mut recap: Vec<String> = Vec::new();
    let mut plays: Vec<String> = Vec::new();
    let mut in_recap = false;

    let banner = |t: &str, kw: &str| -> Option<String> {
        let rest = t.strip_prefix(kw)?.trim_start();
        let inner = rest.strip_prefix('[')?;
        let name = inner.split(']').next()?;
        Some(name.to_string())
    };

    for line in output.lines() {
        let t = strip_ansi(line).trim().to_string();
        if t.is_empty() || t.chars().all(|c| c == '*') {
            continue;
        }
        if t.starts_with("PLAY RECAP") {
            in_recap = true;
            continue;
        }
        if in_recap {
            if let Some((host, stats)) = t.split_once(':') {
                recap.push(format!("{}: {}", host.trim(), squeeze_ws(stats)));
            }
            continue;
        }
        if let Some(name) = banner(&t, "PLAY") {
            plays.push(name);
            continue;
        }
        if let Some(name) = banner(&t, "TASK").or_else(|| banner(&t, "HANDLER")) {
            tasks.push(Task {
                name,
                ok: 0,
                changed: 0,
                skipped: 0,
                unreachable: 0,
                failed: Vec::new(),
            });
            continue;
        }
        let host = |t: &str| -> String {
            t.split('[')
                .nth(1)
                .and_then(|r| r.split(']').next())
                .unwrap_or("?")
                .to_string()
        };
        if let Some(task) = tasks.last_mut() {
            if t.starts_with("ok:") {
                task.ok += 1;
            } else if t.starts_with("changed:") {
                task.changed += 1;
                task.name = task.name.clone();
            } else if t.starts_with("skipping:") {
                task.skipped += 1;
            } else if t.starts_with("unreachable:") {
                task.unreachable += 1;
                task.failed.push(format!("unreachable {}", host(&t)));
            } else if t.starts_with("fatal:") || t.starts_with("failed:") {
                // `fatal: [web]: FAILED! => {"msg": "boom", ...}`
                let msg = t
                    .split_once("=> ")
                    .and_then(|(_, j)| parse_json(j))
                    .and_then(|docs| {
                        docs.first()
                            .and_then(|d| d.get("msg"))
                            .and_then(|m| m.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_else(|| truncate(&squeeze_ws(&t), 200));
                task.failed.push(format!("{}: {}", host(&t), msg));
            }
        }
    }

    let mut out: Vec<String> = Vec::new();
    if !plays.is_empty() {
        out.push(format!("plays: {}", plays.join(", ")));
    }
    let mut lines: Vec<String> = Vec::new();
    for t in &tasks {
        if t.ok == 0
            && t.changed == 0
            && t.skipped == 0
            && t.failed.is_empty()
            && t.unreachable == 0
        {
            continue;
        }
        let mut bits = Vec::new();
        if t.ok > 0 {
            bits.push(format!("ok={}", t.ok))
        }
        if t.changed > 0 {
            bits.push(format!("changed={}", t.changed))
        }
        if t.skipped > 0 {
            bits.push(format!("skipped={}", t.skipped))
        }
        if !t.failed.is_empty() {
            bits.push(format!("failed={}", t.failed.len()))
        }
        lines.push(format!("{}: {}", t.name, bits.join(" ")));
        for f in &t.failed {
            lines.push(format!("  {}", f));
        }
    }
    out.extend(cap_vec(lines, l.list_max_lines, "tasks"));
    if !recap.is_empty() {
        out.push("recap:".to_string());
        out.extend(cap_vec(recap, l.list_max_lines, "hosts"));
    }
    if out.is_empty() {
        return compact_json_output(output, generic);
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = "\
aws_vpc.main: Refreshing state... [id=vpc-1]
aws_subnet.a: Refreshing state... [id=subnet-1]

Terraform used the selected providers to generate the following execution plan.

  # aws_instance.web will be created
  + resource \"aws_instance\" \"web\" {
      + ami           = \"ami-123\"
      + instance_type = \"t3.micro\"
      + id            = (known after apply)
      + arn           = (known after apply)
    }

  # aws_security_group.sg must be replaced
-/+ resource \"aws_security_group\" \"sg\" {
      ~ name = \"old\" -> \"new\" # forces replacement
    }

  # aws_subnet.a will be destroyed
  - resource \"aws_subnet\" \"a\" {
      - cidr_block = \"10.0.1.0/24\" -> null
    }

Plan: 2 to add, 0 to change, 2 to destroy.";

    #[test]
    fn plan_groups_actions_keeps_attrs_and_trailer() {
        let out = filter_terraform(&["plan"], PLAN);
        assert!(out.contains("create (1): aws_instance.web"), "{}", out);
        assert!(
            out.contains("replace (1): aws_security_group.sg"),
            "{}",
            out
        );
        assert!(out.contains("destroy (1): aws_subnet.a"), "{}", out);
        assert!(out.contains("instance_type = \"t3.micro\""), "{}", out);
        assert!(out.contains("forces replacement"), "{}", out);
        assert!(out.contains("(2 attributes known after apply)"), "{}", out);
        assert!(
            out.ends_with("Plan: 2 to add, 0 to change, 2 to destroy."),
            "{}",
            out
        );
        assert!(!out.contains("Refreshing state"), "{}", out);
    }

    #[test]
    fn plan_no_changes_and_error_gutter_stripped() {
        let none = filter_terraform(
            &["plan"],
            "aws_vpc.main: Refreshing state... [id=vpc-1]\n\nNo changes. Your infrastructure matches the configuration.",
        );
        assert!(none.starts_with("No changes."), "{}", none);
        let err = filter_terraform(
            &["plan"],
            "╷\n│ Error: Inconsistent dependency lock file\n│ \n│ The following dependency selections recorded in the lock file are\n│ inconsistent with the current configuration:\n╵",
        );
        assert!(
            err.contains("Error: Inconsistent dependency lock file"),
            "{}",
            err
        );
        assert!(
            err.contains("inconsistent with the current configuration"),
            "{}",
            err
        );
        assert!(!err.contains('│'), "{}", err);
    }

    #[test]
    fn plan_resource_cap_is_announced() {
        let mut raw = String::new();
        for i in 0..300 {
            raw.push_str(&format!("  # aws_instance.n{} will be created\n", i));
        }
        raw.push_str("Plan: 300 to add, 0 to change, 0 to destroy.");
        let out = filter_terraform(&["plan"], &raw);
        assert!(out.contains("create (300)"), "{}", out);
        assert!(has_truncation(&out), "{}", out);
    }

    #[test]
    fn apply_groups_completions_and_outputs() {
        let out = filter_terraform(
            &["apply", "-auto-approve"],
            "aws_instance.web: Creating...\naws_instance.web: Still creating... [10s elapsed]\naws_instance.web: Creation complete after 32s [id=i-abc]\naws_subnet.a: Destruction complete after 1s\n\nApply complete! Resources: 1 added, 0 changed, 1 destroyed.\n\nOutputs:\n\nurl = \"https://x\"",
        );
        assert!(out.contains("creation (1): aws_instance.web"), "{}", out);
        assert!(out.contains("destruction (1): aws_subnet.a"), "{}", out);
        assert!(out.contains("url = \"https://x\""), "{}", out);
        assert!(
            out.contains("Apply complete! Resources: 1 added, 0 changed, 1 destroyed."),
            "{}",
            out
        );
        assert!(!out.contains("Still creating"), "{}", out);
    }

    #[test]
    fn init_validate_fmt_version() {
        let init = filter_terraform(
            &["init"],
            "Initializing the backend...\n\nInitializing provider plugins...\n- Installing hashicorp/aws v5.1.0...\n- Installed hashicorp/aws v5.1.0 (signed)\n\nTerraform has been successfully initialized!",
        );
        assert!(init.contains("providers: hashicorp/aws v5.1.0"), "{}", init);
        assert_eq!(
            filter_terraform(&["validate"], "Success! The configuration is valid.\n"),
            "terraform validate: ok"
        );
        assert_eq!(
            filter_terraform(&["fmt", "-check"], ""),
            "terraform fmt: ok"
        );
        let fmt = filter_terraform(
            &["fmt", "-check", "-diff"],
            "main.tf\n--- old/main.tf\n+++ new/main.tf\n@@ -1,3 +1,3 @@\n-resource \"aws_vpc\" \"main\"{\n+resource \"aws_vpc\" \"main\" {\n",
        );
        assert!(
            fmt.contains("terraform fmt: 1 file need formatting"),
            "{}",
            fmt
        );
        assert!(fmt.contains("main.tf"), "{}", fmt);
        let ver = filter_terraform(
            &["version"],
            "Terraform v1.9.5\non linux_amd64\n\nYour version of Terraform is out of date! The latest version\nis 1.13.0. You can update by downloading from https://www.terraform.io/downloads.html",
        );
        assert!(ver.contains("Terraform v1.9.5"), "{}", ver);
        assert!(ver.contains("update available"), "{}", ver);
        assert!(!ver.contains("out of date"), "{}", ver);
    }

    #[test]
    fn pulumi_preview_keeps_rows_diags_and_trailer() {
        let out = filter_pulumi(
            &["preview"],
            "Previewing update (dev)\n\nView Live: https://app.pulumi.com/x/dev/updates/1\n\n     Type                 Name        Plan\n     pulumi:pulumi:Stack  proj-dev\n +   aws:s3:Bucket        my-bucket   create\n\nDiagnostics:\n  aws:s3:Bucket (my-bucket):\n    warning: bucket has no policy\n\nResources:\n    + 1 to create\n\nDuration: 2s",
        );
        assert!(out.contains("+ aws:s3:Bucket my-bucket create"), "{}", out);
        assert!(out.contains("warning: bucket has no policy"), "{}", out);
        assert!(out.contains("+ 1 to create"), "{}", out);
        assert!(out.contains("Duration: 2s"), "{}", out);
        assert!(
            out.contains("https://app.pulumi.com/x/dev/updates/1"),
            "{}",
            out
        );
        assert!(!out.contains("pulumi:pulumi:Stack"), "{}", out);
    }

    #[test]
    fn ansible_collapses_tasks_and_keeps_failures() {
        let out = filter_ansible(
            "PLAY [webservers] *********************************************\n\nTASK [Gathering Facts] ****************************************\nok: [web1]\nok: [web2]\n\nTASK [Install nginx] ******************************************\nchanged: [web1]\nok: [web2]\nskipping: [web3]\n\nTASK [Start nginx] ********************************************\nfatal: [web1]: FAILED! => {\"changed\": false, \"msg\": \"Could not find unit nginx.service\"}\n\nPLAY RECAP ****************************************************\nweb1                       : ok=2    changed=1    unreachable=0    failed=1    skipped=0\nweb2                       : ok=2    changed=0    unreachable=0    failed=0    skipped=0",
        );
        assert!(out.contains("plays: webservers"), "{}", out);
        assert!(out.contains("Gathering Facts: ok=2"), "{}", out);
        assert!(
            out.contains("Install nginx: ok=1 changed=1 skipped=1"),
            "{}",
            out
        );
        assert!(out.contains("Start nginx: failed=1"), "{}", out);
        assert!(
            out.contains("web1: Could not find unit nginx.service"),
            "{}",
            out
        );
        assert!(
            out.contains("web1: ok=2 changed=1 unreachable=0 failed=1 skipped=0"),
            "{}",
            out
        );
        assert!(!out.contains("****"), "{}", out);
    }

    #[test]
    fn empty_and_unknown_are_safe() {
        assert_eq!(
            filter_terraform(&["plan"], ""),
            "terraform: no changes (0 refresh steps)"
        );
        assert_eq!(filter_terraform(&["providers"], ""), "");
        assert_eq!(filter_pulumi(&["whoami"], "\n\nuser\n\n"), "user");
        let json = filter_ansible("{\"web1\":{\"ping\":\"pong\"}}");
        assert!(json.contains("ping: pong"), "{}", json);
    }
}
