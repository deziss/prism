//! Analytics — token counting, savings tracking, and dashboards.

use anyhow::Result;
use colored::Colorize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

static PROXY_EVENTS_LOCK: Mutex<()> = Mutex::new(());

pub fn prism_data_dir() -> PathBuf {
    crate::prism_data_dir()
}

pub fn analytics_dir() -> PathBuf {
    prism_data_dir().join("analytics")
}

pub fn history_path() -> PathBuf {
    analytics_dir().join("history.json")
}

/// Count tokens in text for a given model.
/// Shared cl100k tokenizer.
///
/// Building this parses a ~1.7MB BPE table and takes on the order of a second.
/// The scorer calls it once per sentence, so rebuilding per call added tens of
/// seconds of latency to every large proxied request.
pub fn bpe() -> Option<&'static tiktoken_rs::CoreBPE> {
    static BPE: std::sync::OnceLock<Option<tiktoken_rs::CoreBPE>> = std::sync::OnceLock::new();
    BPE.get_or_init(|| tiktoken_rs::cl100k_base().ok()).as_ref()
}

/// Count tokens in `text`. Falls back to a chars/3.5 estimate if the tokenizer
/// is unavailable.
pub fn count_tokens(text: &str, _model: &str) -> Result<usize> {
    Ok(match bpe() {
        Some(b) => b.encode_ordinary(text).len(),
        None => (text.chars().count() as f64 / 3.5) as usize,
    })
}

/// Pricing table: (model prefix, input $/1K tokens, output $/1K tokens)
const PRICING: &[(&str, f64, f64)] = &[
    ("gpt-4o-mini", 0.00015, 0.0006),
    ("gpt-4o", 0.005, 0.015),
    ("gpt-4-turbo", 0.01, 0.03),
    ("gpt-3.5", 0.0005, 0.0015),
    ("claude-3-5-haiku", 0.0008, 0.004),
    ("claude-3-5-sonnet", 0.003, 0.015),
    ("claude-3-opus", 0.015, 0.075),
    ("claude-3-haiku", 0.00025, 0.00125),
    ("claude-3-sonnet", 0.003, 0.015),
    ("gemini-1.5-flash", 0.000075, 0.0003),
    ("gemini-1.5-pro", 0.00125, 0.005),
    ("gemini-2.0-flash", 0.0001, 0.0004),
];

/// Estimate cost in USD for a given model and token counts.
pub fn estimate_cost(model: &str, input_tokens: u32, output_tokens: u32) -> f64 {
    let model_lower = model.to_lowercase();
    for (prefix, input_rate, output_rate) in PRICING {
        if model_lower.starts_with(prefix) {
            return (input_tokens as f64 / 1000.0) * input_rate
                + (output_tokens as f64 / 1000.0) * output_rate;
        }
    }
    // fallback: gpt-4o pricing
    (input_tokens as f64 / 1000.0) * 0.005 + (output_tokens as f64 / 1000.0) * 0.015
}

/// Record one proxied request/response to the local analytics log (`prism gain`'s data
/// source). This is local bookkeeping only and knows nothing about the hub — callers in
/// `proxy.rs` build a [`crate::hub::ProxyEvent`] once and separately hand it to
/// `crate::hub::HubSender` for telemetry. The two used to be the same badly-shaped
/// `serde_json::json!` literal (snake_case, silently ignored by the hub's camelCase
/// reader); they are now one typed struct serialized twice, for two different readers.
pub fn record_proxy_event(event: &crate::hub::ProxyEvent) {
    use std::io::Write;
    let path = analytics_dir().join("proxy_events.jsonl");
    let mut line = serde_json::to_string(event).unwrap_or_default();
    line.push('\n');
    if let Ok(_guard) = PROXY_EVENTS_LOCK.lock() {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
        }
    }
}

/// Record a command execution for analytics.
/// Append one `prism cmd` invocation to the local history.
///
/// Takes *bytes*, not tokens: this runs on every shimmed command, so it must not load a
/// tokenizer. `CommandEntry::tokens` approximates at report time.
pub fn record_command(cmd: &str, input_bytes: usize, output_bytes: usize) -> Result<()> {
    let history = load_history()?;

    let entry = CommandEntry {
        timestamp: chrono::Utc::now(),
        command: cmd.to_string(),
        input_bytes,
        output_tokens: 0,
        output_bytes,
        savings: 0,
    };

    let mut entries = history.commands;
    entries.push(entry);
    if entries.len() > 10_000 {
        entries = entries.split_off(entries.len() - 10_000);
    }

    save_history(History {
        commands: entries,
        total_savings_tokens: history.total_savings_tokens,
        sessions: history.sessions,
    })
}

#[derive(Default)]
struct ProxySummary {
    requests: usize,
    orig_tokens: u64,
    sent_tokens: u64,
    cost_usd: f64,
}

impl ProxySummary {
    fn saved_tokens(&self) -> u64 {
        self.orig_tokens.saturating_sub(self.sent_tokens)
    }
}

/// Aggregate the proxy event log written by `record_proxy_event`.
fn load_proxy_summary() -> ProxySummary {
    let path = analytics_dir().join("proxy_events.jsonl");
    let raw = {
        let _guard = PROXY_EVENTS_LOCK.lock().ok();
        std::fs::read_to_string(&path).ok()
    };
    let Some(raw) = raw else {
        return ProxySummary::default();
    };

    let mut sum = ProxySummary::default();
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        sum.requests += 1;
        sum.orig_tokens += v.get("origTokens").and_then(|x| x.as_u64()).unwrap_or(0);
        sum.sent_tokens += v.get("sentTokens").and_then(|x| x.as_u64()).unwrap_or(0);
        sum.cost_usd += v.get("costUsd").and_then(|x| x.as_f64()).unwrap_or(0.0);
    }
    sum
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GainHistoryEntry {
    pub timestamp: String,
    pub tokens: usize,
    pub command: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GainProxySummary {
    pub requests: usize,
    pub orig_tokens: u64,
    pub sent_tokens: u64,
    pub saved_tokens: u64,
    pub saved_pct: f64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GainTopCommand {
    pub command: String,
    pub tokens: usize,
}

/// Everything `prism gain` can show, computed once and shared by the human printer and
/// `--json`. `history` is only populated when `--history` was requested.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GainReport {
    pub total_commands: usize,
    pub total_output_tokens: usize,
    pub proxy: Option<GainProxySummary>,
    pub top_commands: Vec<GainTopCommand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<GainHistoryEntry>>,
}

/// Compute the gains report. Shared by the CLI's human printer and its `--json` mode —
/// there is exactly one place that reads `history.json` and the proxy event log.
pub fn compute_gains(history_flag: bool) -> Result<GainReport> {
    let hist = load_history()?;
    let total_output_tokens: usize = hist.commands.iter().map(|e| e.tokens()).sum();

    let proxy_summary = load_proxy_summary();
    let proxy = (proxy_summary.requests > 0).then(|| {
        let saved = proxy_summary.saved_tokens();
        let pct = if proxy_summary.orig_tokens > 0 {
            (saved as f64 / proxy_summary.orig_tokens as f64) * 100.0
        } else {
            0.0
        };
        GainProxySummary {
            requests: proxy_summary.requests,
            orig_tokens: proxy_summary.orig_tokens,
            sent_tokens: proxy_summary.sent_tokens,
            saved_tokens: saved,
            saved_pct: pct,
            cost_usd: proxy_summary.cost_usd,
        }
    });

    let mut by_cmd: HashMap<&str, usize> = HashMap::new();
    for e in &hist.commands {
        *by_cmd.entry(&e.command).or_default() += e.tokens();
    }
    let mut top: Vec<_> = by_cmd.into_iter().collect();
    top.sort_by_key(|a| std::cmp::Reverse(a.1));
    let top_commands = top
        .into_iter()
        .take(10)
        .map(|(command, tokens)| GainTopCommand {
            command: command.to_string(),
            tokens,
        })
        .collect();

    let history = history_flag.then(|| {
        hist.commands
            .iter()
            .rev()
            .take(50)
            .map(|e| GainHistoryEntry {
                timestamp: e.timestamp.format("%m-%d %H:%M").to_string(),
                tokens: e.tokens(),
                command: e.command.clone(),
            })
            .collect()
    });

    Ok(GainReport {
        total_commands: hist.commands.len(),
        total_output_tokens,
        proxy,
        top_commands,
        history,
    })
}

/// Show token savings dashboard.
pub async fn show_gains(history_flag: bool, json: bool) -> Result<()> {
    let report = compute_gains(history_flag)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    if let Some(history) = &report.history {
        println!("\n  PRISM Command History\n{}", "─".repeat(50));
        for entry in history {
            println!(
                "  {}  {:>12} tokens  {}",
                entry.timestamp, entry.tokens, entry.command
            );
        }
    } else {
        println!(
            "\n  {}  {}",
            "PRISM TOKEN ANALYTICS".bold().cyan(),
            concat!("v", env!("CARGO_PKG_VERSION")).dimmed()
        );
        println!("  {}\n", "─".repeat(65).dimmed());
        println!(
            "  {:<30} {}",
            "Total commands tracked:",
            report.total_commands.to_string().cyan().bold()
        );
        println!(
            "  {:<30} {}",
            "Total output tokens:",
            report.total_output_tokens.to_string().cyan().bold()
        );

        // Measured proxy savings — read from the event log, not estimated.
        if let Some(proxy) = &report.proxy {
            println!("\n  {}", "PROXY INTERCEPTION:".bold().yellow());
            println!(
                "    {:<28} {}",
                "Requests intercepted:",
                proxy.requests.to_string().cyan()
            );
            println!(
                "    {:<28} {}",
                "Original prompt tokens:",
                proxy.orig_tokens.to_string().cyan()
            );
            println!(
                "    {:<28} {}",
                "Sent prompt tokens:",
                proxy.sent_tokens.to_string().cyan()
            );
            println!(
                "    {:<28} {} ({:.1}%)",
                "Measured token savings:",
                proxy.saved_tokens.to_string().green().bold(),
                proxy.saved_pct
            );
            println!(
                "    {:<28} ${:.4}",
                "Spend on forwarded traffic:", proxy.cost_usd
            );
        } else {
            println!(
                "\n  {}",
                "No proxy traffic recorded yet — start intercepting with `prism-enable`.".dimmed()
            );
        }

        println!("\n  {}", "TOP COMMANDS BY TOKEN SPEND:".bold().yellow());
        for top in &report.top_commands {
            println!(
                "    {:<18} {:>10} tokens",
                top.command.cyan(),
                top.tokens.to_string().white()
            );
        }
        println!("  {}\n", "─".repeat(65).dimmed());
    }

    Ok(())
}

/// Discover missed optimization opportunities from Claude Code history.
pub async fn discover() -> Result<()> {
    println!(
        "\n  {}  {}",
        "PRISM DISCOVERY".bold().cyan(),
        concat!("v", env!("CARGO_PKG_VERSION")).dimmed()
    );
    println!("  {}\n", "─".repeat(65).dimmed());

    let hist = load_history()?;
    let uncached: Vec<_> = hist.commands.iter().filter(|e| !e.is_cacheable()).collect();

    println!("  Commands that could be cached: {}", uncached.len());
    for e in uncached.iter().take(5) {
        println!("    {}  ({} tokens)", e.command, e.tokens());
    }
    println!(
        "\n  {} opportunity{} found\n",
        uncached.len(),
        if uncached.len() == 1 { "" } else { "s" }
    );
    Ok(())
}

// --- internal types ---
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct History {
    commands: Vec<CommandEntry>,
    total_savings_tokens: usize,
    sessions: usize,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct CommandEntry {
    timestamp: chrono::DateTime<chrono::Utc>,
    command: String,
    input_bytes: usize,
    /// Exact token count. Only present on entries written before token counting moved
    /// off the hot path; zero on new ones, where `output_bytes` carries the size.
    #[serde(default)]
    output_tokens: usize,
    /// Bytes of filtered output. Free to record, unlike an exact count.
    #[serde(default)]
    output_bytes: usize,
    savings: usize,
}

impl CommandEntry {
    /// Tokens for reporting: exact when the entry carries one, otherwise the same
    /// chars/3.5 approximation `count_tokens` falls back to.
    ///
    /// Counting exactly here would mean loading the cl100k BPE table once per command —
    /// 0.54s measured — which is unacceptable now that a PATH shim puts prism in front
    /// of every command a user or agent runs. A savings dashboard does not need billing
    /// precision.
    fn tokens(&self) -> usize {
        if self.output_tokens > 0 {
            self.output_tokens
        } else {
            (self.output_bytes as f64 / 3.5) as usize
        }
    }
}

impl CommandEntry {
    fn is_cacheable(&self) -> bool {
        matches!(
            self.command.as_str(),
            "git" | "cargo" | "grep" | "find" | "ls"
        )
    }
}

/// Attempt to repair a partially written or truncated .
/// Discards any incomplete trailing entry, closes the JSON structure,
/// and parses the valid portion.
fn try_repair_history(content: &str) -> Option<History> {
    let mut idx = content.len();
    while let Some(pos) = content[..idx].rfind('}') {
        let candidate = format!(
            "{}\n  ],\n  \"total_savings_tokens\": 0,\n  \"sessions\": 0\n}}",
            &content[..=pos]
        );
        if let Ok(hist) = serde_json::from_str::<History>(&candidate) {
            return Some(hist);
        }
        idx = pos;
    }
    None
}

fn load_history() -> Result<History> {
    let path = history_path();
    if !path.exists() {
        return Ok(History {
            commands: Vec::new(),
            total_savings_tokens: 0,
            sessions: 0,
        });
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("failed to read history file at {}: {}", path.display(), e);
            return Ok(History {
                commands: Vec::new(),
                total_savings_tokens: 0,
                sessions: 0,
            });
        }
    };

    match serde_json::from_str::<History>(&content) {
        Ok(h) => Ok(h),
        Err(err) => {
            let corrupt_path = analytics_dir().join("history.json.corrupt");
            tracing::warn!(
                "history.json failed to parse ({err}); attempting repair and moving corrupt file to {}",
                corrupt_path.display()
            );
            eprintln!("Warning: history.json was corrupt ({err}); moved to history.json.corrupt");
            let _ = std::fs::rename(&path, &corrupt_path);

            if let Some(repaired) = try_repair_history(&content) {
                let count = repaired.commands.len();
                tracing::info!("recovered {count} commands from truncated history.json");
                let _ = save_history(History {
                    commands: repaired.commands.clone(),
                    total_savings_tokens: repaired.total_savings_tokens,
                    sessions: repaired.sessions,
                });
                Ok(repaired)
            } else {
                Ok(History {
                    commands: Vec::new(),
                    total_savings_tokens: 0,
                    sessions: 0,
                })
            }
        }
    }
}

fn save_history(history: History) -> Result<()> {
    std::fs::create_dir_all(analytics_dir())?;
    std::fs::write(history_path(), serde_json::to_string_pretty(&history)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_ANALYTICS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn load_history_recovers_from_truncated_file() {
        let _guard = TEST_ANALYTICS_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "prism-history-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join("analytics")).unwrap();
        unsafe { std::env::set_var("PRISM_DATA_DIR", &dir) };

        // Write a partially cut history file (cut in the middle of command 2)
        let broken = r#"{
  "commands": [
    {
      "timestamp": "2026-09-11T12:00:00Z",
      "command": "git status",
      "input_bytes": 100,
      "output_tokens": 0,
      "output_bytes": 50,
      "savings": 0
    },
    {
      "timestamp": "2026-09-11T12:01:00Z",
      "command": "cargo build"#;
        std::fs::write(history_path(), broken).unwrap();

        let loaded = load_history().unwrap();
        assert_eq!(loaded.commands.len(), 1);
        assert_eq!(loaded.commands[0].command, "git status");

        let corrupt_path = analytics_dir().join("history.json.corrupt");
        assert!(
            corrupt_path.exists(),
            "corrupt file must be preserved as history.json.corrupt"
        );

        unsafe { std::env::remove_var("PRISM_DATA_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_history_handles_unrepairable_garbage_safely() {
        let _guard = TEST_ANALYTICS_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "prism-history-garbage-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join("analytics")).unwrap();
        unsafe { std::env::set_var("PRISM_DATA_DIR", &dir) };

        std::fs::write(history_path(), "THIS IS NOT JSON AT ALL").unwrap();

        let loaded = load_history().unwrap();
        assert_eq!(loaded.commands.len(), 0);

        let corrupt_path = analytics_dir().join("history.json.corrupt");
        assert!(corrupt_path.exists());

        unsafe { std::env::remove_var("PRISM_DATA_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }
}
