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
    ("gemini-2.5-pro", 0.00125, 0.005),
    ("gemini-2.5-flash", 0.0001, 0.0004),
    ("gemini-2.5", 0.00125, 0.005),
    ("gemini-2.0", 0.0001, 0.0004),
    ("gemini-", 0.001, 0.004),
    ("gemini", 0.001, 0.004),
];

/// Providers that run on the user's own hardware and bill nothing.
const LOCAL_PROVIDERS: &[&str] = &["ollama", "lm-studio", "lmstudio", "local", "llamacpp"];

/// True when this provider charges nothing because the model runs locally.
pub fn is_local_provider(provider: &str) -> bool {
    let p = provider.to_lowercase();
    LOCAL_PROVIDERS.iter().any(|l| p == *l)
}

/// Estimate cost in USD for a given model and token counts.
///
/// Returns `None` when the model is unrecognised, rather than guessing. The caller
/// decides what an unknown model is worth — see [`estimate_cost`].
fn lookup_cost(model: &str, input_tokens: u32, output_tokens: u32) -> Option<f64> {
    let model_lower = model.to_lowercase();
    for (prefix, input_rate, output_rate) in PRICING {
        if model_lower.starts_with(prefix) {
            return Some(
                (input_tokens as f64 / 1000.0) * input_rate
                    + (output_tokens as f64 / 1000.0) * output_rate,
            );
        }
    }
    None
}

/// Estimate cost in USD, falling back to gpt-4o rates for an unrecognised model.
///
/// Prefer [`estimate_cost_for`] wherever the provider is known: this fallback
/// invents spend for anything it does not recognise.
pub fn estimate_cost(model: &str, input_tokens: u32, output_tokens: u32) -> f64 {
    lookup_cost(model, input_tokens, output_tokens).unwrap_or(
        // fallback: gpt-4o pricing
        (input_tokens as f64 / 1000.0) * 0.005 + (output_tokens as f64 / 1000.0) * 0.015,
    )
}

/// Estimate cost, but charge nothing for a model running on the user's own machine.
///
/// A request to Ollama or LM Studio costs the user electricity, not API spend. The
/// unconditional gpt-4o fallback billed local inference at hosted rates — one
/// `model: "unknown"` Ollama response produced $0.0059 of fictional spend and became
/// the entirety of `prism gain`'s "Spend on forwarded traffic". Reporting invented
/// money is worse than reporting none.
pub fn estimate_cost_for(
    provider: &str,
    model: &str,
    input_tokens: u32,
    output_tokens: u32,
) -> f64 {
    if is_local_provider(provider) {
        return 0.0;
    }
    estimate_cost(model, input_tokens, output_tokens)
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
    /// How many times this command ran.
    pub runs: usize,
    /// Approximate tokens the filters kept out of context for this command.
    pub saved_tokens: usize,
    /// Saved as a share of what the command would have cost unfiltered.
    pub saved_pct: f64,
}

/// Everything `prism gain` can show, computed once and shared by the human printer and
/// `--json`. `history` is only populated when `--history` was requested.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GainReport {
    pub total_commands: usize,
    pub total_output_tokens: usize,
    /// Approximate tokens the raw command output would have cost unfiltered.
    pub total_input_tokens: usize,
    /// Approximate tokens the `prism cmd` filters kept out of context.
    pub total_saved_tokens: usize,
    pub saved_pct: f64,
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

    let total_input_tokens: usize = hist.commands.iter().map(|e| e.input_tokens()).sum();
    let total_saved_tokens: usize = hist.commands.iter().map(|e| e.saved_tokens()).sum();
    let saved_pct = if total_input_tokens > 0 {
        (total_saved_tokens as f64 / total_input_tokens as f64) * 100.0
    } else {
        0.0
    };

    // Rank by tokens *saved*, not tokens spent. The old ordering answered "which
    // command produced the most output", which reads as a cost ranking; what the
    // report is for is showing where the filtering earns its keep.
    #[derive(Default)]
    struct CmdAgg {
        tokens: usize,
        input: usize,
        saved: usize,
        runs: usize,
    }
    let mut by_cmd: HashMap<&str, CmdAgg> = HashMap::new();
    for e in &hist.commands {
        let agg = by_cmd.entry(&e.command).or_default();
        agg.tokens += e.tokens();
        agg.input += e.input_tokens();
        agg.saved += e.saved_tokens();
        agg.runs += 1;
    }
    let mut top: Vec<_> = by_cmd.into_iter().collect();
    top.sort_by_key(|(_, a)| std::cmp::Reverse(a.saved));
    let top_commands = top
        .into_iter()
        .take(10)
        .map(|(command, agg)| GainTopCommand {
            command: command.to_string(),
            tokens: agg.tokens,
            runs: agg.runs,
            saved_tokens: agg.saved,
            saved_pct: if agg.input > 0 {
                (agg.saved as f64 / agg.input as f64) * 100.0
            } else {
                0.0
            },
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
        total_input_tokens,
        total_saved_tokens,
        saved_pct,
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

        // The headline number. `prism cmd` filtering is what runs on every shimmed
        // command, so this is the figure that represents what PRISM actually did —
        // and it read a flat 0 until the report learned to derive it from the bytes
        // that were being recorded all along.
        println!("\n  {}", "CLI FILTERING (prism cmd):".bold().yellow());
        println!(
            "    {:<28} {}",
            "Raw output tokens:",
            report.total_input_tokens.to_string().cyan()
        );
        println!(
            "    {:<28} {}",
            "After filtering:",
            report.total_output_tokens.to_string().cyan()
        );
        println!(
            "    {:<28} {} ({:.1}%)",
            "Tokens saved:",
            report.total_saved_tokens.to_string().green().bold(),
            report.saved_pct
        );
        println!(
            "    {}",
            "approximate: bytes/3.5, not a tokenizer pass — counting exactly on the".dimmed()
        );
        println!("    {}", "hot path costs ~0.5s per command.".dimmed());

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

        println!("\n  {}", "TOP COMMANDS BY TOKENS SAVED:".bold().yellow());
        println!(
            "    {:<14} {:>7} {:>12} {:>8}",
            "command".dimmed(),
            "runs".dimmed(),
            "saved".dimmed(),
            "saved %".dimmed()
        );
        for top in &report.top_commands {
            println!(
                "    {:<14} {:>7} {:>12} {:>7.1}%",
                top.command,
                top.runs,
                top.saved_tokens.to_string().green(),
                top.saved_pct
            );
        }
        // rtk prints the same nudge when its hook is absent, and it is the right
        // call: with shims off, prism only sees commands typed as `prism cmd ...`,
        // so a near-empty report means "not wired up", not "no savings available".
        // Now that install no longer enables shims, this is the discovery path.
        if !crate::shim::status().active {
            println!();
            println!(
                "  {} {}",
                "[warn]".yellow().bold(),
                "No shims on PATH — only explicit `prism cmd ...` runs are counted.".yellow()
            );
            println!(
                "         {}",
                "Run `prism shim install --path` for automatic filtering.".dimmed()
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

    /// Tokens the command *would* have cost unfiltered, approximated from the raw
    /// byte count by the same chars/3.5 rule `tokens()` uses for the filtered side.
    ///
    /// Recorded entries carry `savings: 0` because counting tokens on the hot path
    /// costs ~0.54s per invocation (see `record_command`). The bytes to derive it
    /// from were always there; nothing ever derived it, so `prism gain` reported a
    /// flat zero saving no matter how much the filters actually removed.
    fn input_tokens(&self) -> usize {
        (self.input_bytes as f64 / 3.5) as usize
    }

    /// Approximate tokens this invocation kept out of the model's context.
    ///
    /// Saturating: a filter may add a byte or two (the `↳ /tee/path` pointer) on
    /// output it otherwise passed through, and a negative saving is not meaningful.
    fn saved_tokens(&self) -> usize {
        self.input_tokens().saturating_sub(self.tokens())
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

/// Write the history file atomically: serialise to a temp file in the same
/// directory, then rename over the target.
///
/// This used to be a plain `std::fs::write`, which truncates the file and then
/// streams bytes into it. Every shimmed command runs this, each in its *own
/// process*, so an in-process mutex would not help: two concurrent `prism cmd`
/// invocations could interleave and leave a half-written file. That is not
/// theoretical — it happened on this machine during a parallel build, and
/// `try_repair_history` salvaged 436 of roughly 1,500 entries before moving the
/// rest aside as `history.json.corrupt`. The repair path exists precisely because
/// this write was unsafe.
///
/// `rename(2)` within one filesystem is atomic, so a reader now sees either the
/// previous file or the complete new one, never a partial write.
///
/// Concurrent writers can still lose an *update* — both read the same state and
/// the last rename wins, dropping a handful of entries under heavy parallelism.
/// That is a far better failure than losing the file, and fixing it properly needs
/// cross-process advisory locking (flock), which would mean a new dependency on
/// the hot path of every command.
fn save_history(history: History) -> Result<()> {
    let dir = analytics_dir();
    std::fs::create_dir_all(&dir)?;
    let body = serde_json::to_string_pretty(&history)?;

    // Same directory as the target: rename is only atomic within a filesystem.
    let tmp = dir.join(format!("history.json.tmp.{}", std::process::id()));
    std::fs::write(&tmp, body)?;
    if let Err(e) = std::fs::rename(&tmp, history_path()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_env::TestEnv;

    fn entry(command: &str, input_bytes: usize, output_bytes: usize) -> CommandEntry {
        CommandEntry {
            timestamp: chrono::Utc::now(),
            command: command.to_string(),
            input_bytes,
            output_tokens: 0,
            output_bytes,
            savings: 0,
        }
    }

    /// Savings must be derived from the recorded bytes.
    ///
    /// `record_command` deliberately stores `savings: 0` and `output_tokens: 0` —
    /// counting tokens on the hot path costs ~0.5s per invocation. Nothing then
    /// derived them at report time, so `prism gain` reported a flat zero saving no
    /// matter how much the filters removed: a real 121,258 -> 6,611 byte reduction
    /// (95%) displayed as "0 (0.0%)".
    #[test]
    fn saved_tokens_are_derived_from_bytes_not_the_stored_zero() {
        let e = entry("ps", 121_258, 6_611);
        assert_eq!(
            e.savings, 0,
            "storage stays cheap: nothing is counted inline"
        );
        assert!(e.saved_tokens() > 30_000, "got {}", e.saved_tokens());
        let pct = e.saved_tokens() as f64 / e.input_tokens() as f64 * 100.0;
        assert!((94.0..=96.0).contains(&pct), "expected ~95%, got {pct:.1}");
    }

    /// A local model bills nothing. The unconditional gpt-4o fallback charged it
    /// hosted rates: one `model: "unknown"` Ollama response produced $0.0059 of
    /// fictional spend, which was the whole of `prism gain`'s reported cost.
    #[test]
    fn local_providers_cost_nothing_even_with_an_unknown_model() {
        assert_eq!(estimate_cost_for("ollama", "unknown", 1_000, 1_000), 0.0);
        assert_eq!(estimate_cost_for("lm-studio", "llama3", 5_000, 5_000), 0.0);
        // The exact case from the live event log.
        assert_eq!(estimate_cost_for("ollama", "unknown", 0, 394), 0.0);
    }

    /// Hosted providers must still be priced, including the unknown-model fallback —
    /// under-reporting real spend would be its own kind of lie.
    #[test]
    fn hosted_providers_are_still_priced() {
        assert!(estimate_cost_for("anthropic", "claude-3-5-sonnet", 1_000, 1_000) > 0.0);
        assert!(estimate_cost_for("openai", "some-new-model", 1_000, 1_000) > 0.0);
        assert_eq!(
            estimate_cost_for("openai", "gpt-4o", 1_000, 1_000),
            estimate_cost("gpt-4o", 1_000, 1_000)
        );
    }

    /// A partial write must never be visible: readers see the old file or the new
    /// one, never a truncated one. This is what `history.json.corrupt` came from.
    #[test]
    fn save_history_is_atomic_and_leaves_no_temp_file() {
        let dir = std::env::temp_dir().join(format!(
            "prism-hist-atomic-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::create_dir_all(dir.join("analytics")).unwrap();
        let _env = TestEnv::redirect(&dir);

        for n in 1..=3 {
            save_history(History {
                commands: (0..n).map(|_| entry("git", 100, 10)).collect(),
                total_savings_tokens: 0,
                sessions: 0,
            })
            .unwrap();
            // Always parseable after every write.
            assert_eq!(load_history().unwrap().commands.len(), n);
        }

        let leftovers: Vec<_> = std::fs::read_dir(dir.join("analytics"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A filter that passes output through unchanged saves nothing, and a filter that
    /// adds a byte (the `↳ /tee/path` pointer) must not report a negative saving.
    #[test]
    fn passthrough_and_growth_never_report_a_saving() {
        assert_eq!(entry("docker", 28_630, 28_630).saved_tokens(), 0);
        assert_eq!(entry("git", 100, 4_000).saved_tokens(), 0);
        assert_eq!(entry("noop", 0, 0).saved_tokens(), 0);
    }

    #[test]
    fn load_history_recovers_from_truncated_file() {
        let dir = std::env::temp_dir().join(format!(
            "prism-history-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join("analytics")).unwrap();
        let _env = TestEnv::redirect(&dir);

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

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_history_handles_unrepairable_garbage_safely() {
        let dir = std::env::temp_dir().join(format!(
            "prism-history-garbage-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join("analytics")).unwrap();
        let _env = TestEnv::redirect(&dir);

        std::fs::write(history_path(), "THIS IS NOT JSON AT ALL").unwrap();

        let loaded = load_history().unwrap();
        assert_eq!(loaded.commands.len(), 0);

        let corrupt_path = analytics_dir().join("history.json.corrupt");
        assert!(corrupt_path.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
