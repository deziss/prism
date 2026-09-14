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
    record_command_timed(cmd, input_bytes, output_bytes, 0, 0)
}

/// As [`record_command`], plus the two clocks `prism cmd` already measures:
/// `duration_ms` for the wrapped command and `filter_us` for PRISM's own pass.
///
/// A separate entry point rather than a wider `record_command` signature, so that
/// a caller with nothing to report (and every entry written before this existed)
/// stays valid. Both timings default to 0 and the report reads 0 as "not
/// measured" — see [`CommandEntry::is_timed`].
///
/// Both numbers are already in hand at the only call site (`cli.rs`, where they
/// are handed to `hub::spool_event`); the hub spool is not a substitute source,
/// because it is written only on enrolled machines and is deleted as it drains.
pub fn record_command_timed(
    cmd: &str,
    input_bytes: usize,
    output_bytes: usize,
    duration_ms: u64,
    filter_us: u64,
) -> Result<()> {
    let entry = CommandEntry {
        timestamp: chrono::Utc::now(),
        command: cmd.to_string(),
        input_bytes,
        output_tokens: 0,
        output_bytes,
        savings: 0,
        duration_ms,
        filter_us,
    };
    append_to_journal(&entry)
}

/// Append-only sidecar for [`record_command_timed`].
///
/// This used to read `history.json`, push one entry, and rewrite the whole file. That
/// is O(n) per command on a file that grows to the 10,000-entry cap: measured at
/// **7.7 ms of the ~12 ms** PRISM adds to `git status` on a 1.2 MB history — more than
/// twenty times the cost of the filter pass it exists to measure. An analytics feature
/// that is the dominant cost of the tool it instruments is self-defeating.
///
/// One line of JSON appended with `O_APPEND` instead. Writes under `PIPE_BUF` (4096 on
/// Linux) to a file opened for append are not interleaved by the kernel, so concurrent
/// `prism cmd` processes cannot corrupt each other's lines — which also closes the
/// lost-update race the old read-modify-write had, where two writers read the same
/// state and the last rename won.
fn append_to_journal(entry: &CommandEntry) -> Result<()> {
    use std::io::Write;

    let dir = analytics_dir();
    std::fs::create_dir_all(&dir)?;
    let mut line = serde_json::to_string(entry)?;
    line.push('\n');

    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(journal_path())?;
    f.write_all(line.as_bytes())?;

    // Fold into history.json only when the journal has grown enough to be worth it, so
    // the cost is amortised across many commands rather than paid by each one.
    if f.metadata().map(|m| m.len()).unwrap_or(0) > JOURNAL_COMPACT_BYTES {
        let _ = compact_journal();
    }
    Ok(())
}

/// Journal size past which the next writer folds it into `history.json`.
///
/// 512 KiB is roughly 2,500 entries — frequent enough that a reader never replays an
/// unbounded file, rare enough that the O(n) rewrite is amortised to well under a
/// microsecond per command.
const JOURNAL_COMPACT_BYTES: u64 = 512 * 1024;

pub(crate) fn journal_path() -> PathBuf {
    analytics_dir().join("commands.jsonl")
}

/// Read the journal, ignoring lines that do not parse.
///
/// A torn final line is expected rather than exceptional: the process can be killed
/// mid-append. Skipping it loses one command's telemetry, where failing the read would
/// lose the whole report.
fn read_journal() -> Vec<CommandEntry> {
    let Ok(raw) = std::fs::read_to_string(journal_path()) else {
        return Vec::new();
    };
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<CommandEntry>(l).ok())
        .collect()
}

/// Merge the journal into `history.json` and truncate it.
///
/// Ordering matters: `history.json` is written *before* the journal is cleared, so a
/// crash between the two replays entries that are already folded in — duplicated
/// telemetry, which is recoverable — rather than clearing a journal whose contents
/// were never persisted, which is data loss.
fn compact_journal() -> Result<()> {
    let merged = load_history()?;
    save_history(merged)?;
    // Truncate rather than remove: an appender may already hold this file open, and
    // unlinking it would send those writes to an orphaned inode.
    std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(journal_path())?;
    Ok(())
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

// ========================= report formatting ==============================
//
// `prism gain` is read at a glance, not parsed — `--json` carries the exact
// integers for anything that needs them. A raw `287079` makes the reader count
// digits before they can compare it to `40562`; `287.1K` and `40.6K` compare on
// sight. These helpers exist here rather than in `utils.rs` because nothing else
// in the tree needs them and `utils.rs` is itself unreferenced.

/// The per-command table is laid out to this width at a four-space indent, so it
/// ends at column 74 — wide enough for eight columns, narrow enough to survive an
/// 80-column terminal. A wrapped row turns the impact bars into confetti, so the
/// layout never assumes more room than that.
const TABLE_WIDTH: usize = 70;

/// Render a count the way a human reads it: `169564` -> `169.6K`.
///
/// Powers of a thousand, not of 1024: these are token counts, and a reader
/// comparing one against a model's context window is thinking in decimal.
fn human_count(n: u64) -> String {
    const SUFFIXES: [&str; 4] = ["K", "M", "B", "T"];
    if n < 1_000 {
        return n.to_string();
    }
    let mut value = n as f64 / 1_000.0;
    let mut tier = 0;
    // `999.95`, not `1000.0`: at one decimal place 999_999 formats as "1000.0K",
    // which is wider than the column and reads worse than the identical "1.0M".
    // Promote *before* rounding can produce a four-digit mantissa.
    while value >= 999.95 && tier + 1 < SUFFIXES.len() {
        value /= 1_000.0;
        tier += 1;
    }
    format!("{value:.1}{}", SUFFIXES[tier])
}

/// Render a wall-clock duration recorded in milliseconds: `0ms`, `326ms`,
/// `14.8s`, `600m10s`.
fn human_ms(ms: u64) -> String {
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    let secs = ms as f64 / 1_000.0;
    if secs < 60.0 {
        return format!("{secs:.1}s");
    }
    let total = ms / 1_000;
    format!("{}m{}s", total / 60, total % 60)
}

/// Render PRISM's own overhead, recorded in microseconds.
///
/// Kept in microseconds precisely because a filter pass over a small command is
/// well under a millisecond: rounding it to `0ms`, as a milliseconds-only clock
/// would, erases the one number that justifies putting a wrapper in front of
/// every command.
fn human_us(us: u64) -> String {
    if us < 1_000 {
        return format!("{us}\u{b5}s");
    }
    let ms = us as f64 / 1_000.0;
    if ms < 1_000.0 {
        return format!("{ms:.1}ms");
    }
    human_ms(us / 1_000)
}

/// A proportional meter, `width` cells wide.
///
/// Block characters rather than colour: `colored` strips escape codes the moment
/// stdout is not a terminal, so a colour-only bar would vanish exactly when
/// someone pipes the report into a file to keep it.
fn meter(fraction: f64, width: usize) -> String {
    let filled = ((fraction.clamp(0.0, 1.0) * width as f64).round() as usize).min(width);
    format!(
        "{}{}",
        "\u{2588}".repeat(filled),
        "\u{2591}".repeat(width - filled)
    )
}

/// A row's impact, relative to the biggest saver in the table.
///
/// Rounds *up*, so every command that saved anything shows at least one block.
/// rtk's equivalent floors, which renders every row below the top one as an empty
/// trough — discarding the ranking the column exists to show.
fn impact_bar(saved: u64, max: u64, width: usize) -> String {
    if max == 0 || saved == 0 {
        return "\u{2591}".repeat(width);
    }
    let filled = (((saved as f64 / max as f64) * width as f64).ceil() as usize).clamp(1, width);
    format!(
        "{}{}",
        "\u{2588}".repeat(filled),
        "\u{2591}".repeat(width - filled)
    )
}

/// Fit a label into a fixed column, eliding the tail.
///
/// Counts `chars`, not bytes: a byte slice would split a multi-byte command name
/// mid-codepoint and panic, and the format widths below count chars too.
fn fit(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let kept: String = s.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}\u{2026}")
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
    /// How many of `runs` carried timings — the denominator for the two averages
    /// below, which is not `runs` whenever history predates timing.
    pub timed_runs: usize,
    /// Mean wall clock of the wrapped command. `None` when no run was timed;
    /// deliberately not `0`, which would read as "instant".
    pub avg_duration_ms: Option<u64>,
    /// Mean microseconds PRISM itself spent filtering this command.
    pub avg_filter_us: Option<u64>,
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
    /// Commands whose entry carries a timing. Both totals below are sums over
    /// *these* entries, and both averages divide by this, not by `total_commands`.
    pub timed_commands: usize,
    /// Summed wall clock of the wrapped commands.
    pub total_duration_ms: u64,
    /// Summed microseconds PRISM itself spent filtering. The differentiator: a
    /// wrapper that only shells out can report the line above but not this one.
    pub total_filter_us: u64,
    pub proxy: Option<GainProxySummary>,
    pub top_commands: Vec<GainTopCommand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<GainHistoryEntry>>,
}

/// Compute the gains report. Shared by the CLI's human printer and its `--json` mode —
/// there is exactly one place that reads `history.json` and the proxy event log.
pub fn compute_gains(history_flag: bool) -> Result<GainReport> {
    // `prism gain` is the natural place for housekeeping: it is a report, not a hot
    // path, so one `readdir` here costs nothing, whereas doing it per command would
    // reintroduce exactly the kind of overhead the journal removed. Temp files left by
    // a writer that died mid-rename are otherwise only swept at the next compaction.
    sweep_stale_temp_files(&analytics_dir());

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
        timed_runs: usize,
        duration_ms: u64,
        filter_us: u64,
    }
    let mut by_cmd: HashMap<&str, CmdAgg> = HashMap::new();
    for e in &hist.commands {
        let agg = by_cmd.entry(&e.command).or_default();
        agg.tokens += e.tokens();
        agg.input += e.input_tokens();
        agg.saved += e.saved_tokens();
        agg.runs += 1;
        // Only timed entries contribute to either clock *or* to the divisor, so an
        // untimed run neither inflates nor deflates the average — it is absent.
        if e.is_timed() {
            agg.timed_runs += 1;
            agg.duration_ms += e.duration_ms;
            agg.filter_us += e.filter_us;
        }
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
            timed_runs: agg.timed_runs,
            avg_duration_ms: (agg.timed_runs > 0).then(|| agg.duration_ms / agg.timed_runs as u64),
            avg_filter_us: (agg.timed_runs > 0).then(|| agg.filter_us / agg.timed_runs as u64),
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

    let timed = hist.commands.iter().filter(|e| e.is_timed());
    let (timed_commands, total_duration_ms, total_filter_us) = timed
        .fold((0usize, 0u64, 0u64), |(n, d, f), e| {
            (n + 1, d + e.duration_ms, f + e.filter_us)
        });

    Ok(GainReport {
        total_commands: hist.commands.len(),
        total_output_tokens,
        total_input_tokens,
        total_saved_tokens,
        saved_pct,
        timed_commands,
        total_duration_ms,
        total_filter_us,
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
        println!("  {}\n", "─".repeat(TABLE_WIDTH + 2).dimmed());

        // The headline block. `prism cmd` filtering is what runs on every shimmed
        // command, so this is the figure that represents what PRISM actually did —
        // and it read a flat 0 until the report learned to derive it from the bytes
        // that were being recorded all along.
        println!("  {}", "CLI FILTERING (prism cmd)".bold().yellow());
        println!(
            "    {:<20} {}",
            "Total commands:",
            human_count(report.total_commands as u64).cyan().bold()
        );
        println!(
            "    {:<20} {}",
            "Input tokens:",
            human_count(report.total_input_tokens as u64).cyan()
        );
        println!(
            "    {:<20} {}",
            "Output tokens:",
            human_count(report.total_output_tokens as u64).cyan()
        );
        println!(
            "    {:<20} {} ({:.1}%)",
            "Tokens saved:",
            human_count(report.total_saved_tokens as u64).green().bold(),
            report.saved_pct
        );

        // Two clocks, never one number. `duration_ms` is how long the user's own
        // `docker build` took; charging that to the wrapper — as a single "Time"
        // column does — makes PRISM look slow for work it did not do. `filter_us`
        // is what wrapping actually cost, and it is the figure a wrapper that only
        // shells out has no way to separate out.
        if report.timed_commands > 0 {
            let n = report.timed_commands as u64;
            println!(
                "    {:<20} {} (avg {})",
                "Command time:",
                human_ms(report.total_duration_ms).cyan(),
                human_ms(report.total_duration_ms / n)
            );
            // Denominator in microseconds so the ratio is dimensionless; guarded
            // because a run of instant commands can total 0ms of wall clock while
            // still having cost PRISM real microseconds.
            let share = if report.total_duration_ms > 0 {
                report.total_filter_us as f64 / (report.total_duration_ms as f64 * 1_000.0) * 100.0
            } else {
                100.0
            };
            println!(
                "    {:<20} {} (avg {}/run, {:.2}% of command time)",
                "PRISM overhead:",
                human_us(report.total_filter_us).green().bold(),
                human_us(report.total_filter_us / n),
                share
            );
        } else {
            // Say "not recorded", not "0ms". The columns exist and the aggregation
            // is live; what is missing is timed entries, and claiming zero overhead
            // would be exactly the kind of flattering lie this report avoids.
            println!("    {:<20} {}", "Command time:", "not recorded".dimmed());
            println!("    {:<20} {}", "PRISM overhead:", "not recorded".dimmed());
        }

        println!(
            "    {:<20} {} {:.1}%",
            "Efficiency:",
            meter(report.saved_pct / 100.0, 24).green(),
            report.saved_pct
        );
        println!(
            "    {}",
            "approximate: bytes/3.5, not a tokenizer pass — counting exactly on the".dimmed()
        );
        println!("    {}", "hot path costs ~0.5s per command.".dimmed());
        if report.timed_commands == 0 {
            println!(
                "    {}",
                "no timings in history yet — entries recorded before this build carry none."
                    .dimmed()
            );
        }

        // Measured proxy savings — read from the event log, not estimated.
        if let Some(proxy) = &report.proxy {
            println!("\n  {}", "PROXY INTERCEPTION".bold().yellow());
            println!(
                "    {:<20} {}",
                "Requests:",
                human_count(proxy.requests as u64).cyan()
            );
            println!(
                "    {:<20} {}",
                "Original prompt:",
                human_count(proxy.orig_tokens).cyan()
            );
            println!(
                "    {:<20} {}",
                "Sent prompt:",
                human_count(proxy.sent_tokens).cyan()
            );
            println!(
                "    {:<20} {} ({:.1}%)",
                "Measured savings:",
                human_count(proxy.saved_tokens).green().bold(),
                proxy.saved_pct
            );
            // Exact, not humanised: this is money, and $0.0062 rounded to "6.2m"
            // would be both wrong and unreadable.
            println!("    {:<20} ${:.4}", "Forwarded spend:", proxy.cost_usd);
            println!(
                "    {:<20} {} {:.1}%",
                "Efficiency:",
                meter(proxy.saved_pct / 100.0, 24).green(),
                proxy.saved_pct
            );
        } else {
            println!(
                "\n  {}",
                "No proxy traffic recorded yet — start intercepting with `prism-enable`.".dimmed()
            );
        }

        println!("\n  {}", "TOP COMMANDS BY TOKENS SAVED".bold().yellow());
        // Impact is relative to the best row, so the table reads as a ranking at a
        // glance instead of ten numbers the eye has to sort itself.
        let max_saved = report
            .top_commands
            .iter()
            .map(|t| t.saved_tokens as u64)
            .max()
            .unwrap_or(0);
        // Same leading indent and column widths as the rows below, so `#` sits over
        // the rank and every heading over its own column.
        let header = format!(
            "    {:>2}  {:<14} {:>5} {:>8} {:>6} {:>8} {:>8}  {}",
            "#", "command", "runs", "saved", "avg%", "cmd", "prism", "impact"
        );
        println!("{}", header.dimmed());
        println!("    {}", "─".repeat(TABLE_WIDTH).dimmed());
        for (i, top) in report.top_commands.iter().enumerate() {
            // Pad *before* colouring. `colored`'s Display writes escape codes and
            // only forwards the format width when colour is disabled, so `{:>8}`
            // applied to an already-coloured value silently stops aligning the
            // moment stdout is a terminal — which is why the old table looked
            // straight only when it was being read through a pipe.
            let saved = format!("{:>8}", human_count(top.saved_tokens as u64));
            // An em dash, not "0ms": no run of this command carried a timing.
            let cmd_time = top.avg_duration_ms.map_or("\u{2014}".into(), human_ms);
            let prism_time = top.avg_filter_us.map_or("\u{2014}".into(), human_us);
            println!(
                "    {:>2}. {:<14} {:>5} {} {:>5.1}% {:>8} {:>8}  {}",
                i + 1,
                fit(&top.command, 14),
                human_count(top.runs as u64),
                saved.green(),
                top.saved_pct,
                cmd_time,
                prism_time,
                impact_bar(top.saved_tokens as u64, max_saved, 10).cyan()
            );
        }
        println!("    {}", "─".repeat(TABLE_WIDTH).dimmed());
        println!(
            "    {}",
            "cmd = wrapped command's wall clock \u{b7} prism = PRISM's own overhead".dimmed()
        );
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
        println!("  {}\n", "─".repeat(TABLE_WIDTH + 2).dimmed());
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
    /// Wall clock of the **wrapped command**, in milliseconds — `prism cmd sleep 3`
    /// records ~3002 here. Mirrors `hub::CommandEvent::duration_ms`.
    ///
    /// Zero means "not measured", never "instant": entries written before this
    /// field existed have no timing at all, and `is_timed` keeps them out of the
    /// averages rather than dragging them towards zero.
    #[serde(default)]
    duration_ms: u64,
    /// Microseconds **PRISM itself** spent: the filter pass, truncation detection,
    /// teeing and the stdout write. Mirrors `hub::CommandEvent::filter_us`.
    ///
    /// Recorded separately from `duration_ms` because the two answer opposite
    /// questions. `duration_ms` is the user's own `docker build` being slow;
    /// `filter_us` is the only figure that says what wrapping it cost, and it is
    /// the number a wrapper that merely shells out has no way to report.
    #[serde(default)]
    filter_us: u64,
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

    /// Whether this entry carries timings at all.
    ///
    /// History mixes entries from before and after timings were recorded. Averaging
    /// over *every* entry would quietly drag the reported per-run cost towards zero
    /// in proportion to how much old history the user has, which is the sort of
    /// flattering-by-accident number this report exists not to print.
    fn is_timed(&self) -> bool {
        self.duration_ms > 0 || self.filter_us > 0
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

/// `history.json` plus everything appended to the journal since it was last folded in.
///
/// Every reader goes through here, so the split storage is invisible above this line:
/// the write path is append-only for speed, the read path presents one history.
fn load_history() -> Result<History> {
    let mut base = load_history_file()?;
    let pending = read_journal();
    if !pending.is_empty() {
        base.commands.extend(pending);
        // The same 10,000-entry cap the old read-modify-write applied, enforced here
        // now that the write path no longer sees the whole list.
        if base.commands.len() > 10_000 {
            base.commands = base.commands.split_off(base.commands.len() - 10_000);
        }
    }
    Ok(base)
}

fn load_history_file() -> Result<History> {
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

    sweep_stale_temp_files(&dir);

    // Same directory as the target: rename is only atomic within a filesystem.
    let tmp = dir.join(format!("history.json.tmp.{}", std::process::id()));
    std::fs::write(&tmp, body)?;
    if let Err(e) = std::fs::rename(&tmp, history_path()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Remove `history.json.tmp.*` left behind by a writer that died between the write and
/// the rename.
///
/// The rename is atomic and the error path unlinks its own temp file, but neither
/// helps if the process is killed in between — this machine had accumulated fifteen
/// zero-byte temp files that way.
///
/// Only files older than an hour, and never the current process's own: a concurrent
/// `prism cmd` may be mid-write, and deleting its temp file would turn a successful
/// write into a spurious failure.
fn sweep_stale_temp_files(dir: &std::path::Path) {
    const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(3600);
    let mine = format!("history.json.tmp.{}", std::process::id());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("history.json.tmp.") || name == mine {
            continue;
        }
        let stale = e
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().unwrap_or_default() > STALE_AFTER)
            .unwrap_or(false);
        if stale {
            let _ = std::fs::remove_file(e.path());
        }
    }
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
            duration_ms: 0,
            filter_us: 0,
        }
    }

    fn timed_entry(
        command: &str,
        input_bytes: usize,
        output_bytes: usize,
        duration_ms: u64,
        filter_us: u64,
    ) -> CommandEntry {
        CommandEntry {
            duration_ms,
            filter_us,
            ..entry(command, input_bytes, output_bytes)
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "prism-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join("analytics")).unwrap();
        dir
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

    /// The unit must change exactly at each power of a thousand, and rounding must
    /// never produce a four-digit mantissa: `999_999` is "1.0M", not "1000.0K",
    /// which would be wider than the column it has to sit in.
    #[test]
    fn human_count_switches_units_at_the_thousand_boundary() {
        assert_eq!(human_count(0), "0");
        assert_eq!(human_count(999), "999");
        assert_eq!(human_count(1_000), "1.0K");
        assert_eq!(human_count(999_999), "1.0M");
        assert_eq!(human_count(1_000_000), "1.0M");
        // The numbers from the report this replaced, and from the bar it has to clear.
        assert_eq!(human_count(169_564), "169.6K");
        assert_eq!(human_count(413_500_000), "413.5M");
        assert_eq!(human_count(10_100_000), "10.1M");
        // Saturates at the largest suffix rather than inventing one.
        assert!(human_count(u64::MAX).ends_with('T'));
    }

    /// A wall clock in milliseconds, at each scale the report can hit.
    #[test]
    fn human_ms_scales_from_milliseconds_to_minutes() {
        assert_eq!(human_ms(0), "0ms");
        assert_eq!(human_ms(23), "23ms");
        assert_eq!(human_ms(999), "999ms");
        assert_eq!(human_ms(1_000), "1.0s");
        assert_eq!(human_ms(14_800), "14.8s");
        assert_eq!(human_ms(59_999), "60.0s");
        assert_eq!(human_ms(60_000), "1m0s");
        assert_eq!(human_ms(36_010_000), "600m10s");
    }

    /// PRISM's overhead must never round to zero.
    ///
    /// A filter pass over a small command is hundreds of microseconds; reporting
    /// it on a milliseconds-only clock would print "0ms" for the single number
    /// that justifies putting a wrapper in front of every command.
    #[test]
    fn human_us_never_rounds_prism_overhead_to_zero() {
        assert_eq!(human_us(1), "1\u{b5}s");
        assert_eq!(human_us(430), "430\u{b5}s");
        assert_eq!(human_us(999), "999\u{b5}s");
        assert_eq!(human_us(1_000), "1.0ms");
        assert_eq!(human_us(2_100), "2.1ms");
        assert_eq!(human_us(999_999), "1000.0ms");
        assert_eq!(human_us(1_500_000), "1.5s");
        assert!(!human_us(1).starts_with('0'));
    }

    /// The meter is a proportion of its width, clamped at both ends, and always
    /// exactly `width` cells so the column below it stays straight.
    #[test]
    fn meter_is_proportional_and_clamped() {
        assert_eq!(meter(0.0, 4), "░░░░");
        assert_eq!(meter(0.5, 4), "██░░");
        assert_eq!(meter(1.0, 4), "████");
        // Out-of-range input must not panic or overflow the column.
        assert_eq!(meter(-1.0, 4), "░░░░");
        assert_eq!(meter(5.0, 4), "████");
        assert_eq!(meter(0.969, 24).chars().count(), 24);
    }

    /// Any command that saved something gets at least one block.
    ///
    /// Flooring instead (what rtk does) renders every row below the top one as an
    /// empty trough, throwing away the ranking the column exists to show: a row
    /// saving 2.6% of the leader is not the same as one saving nothing.
    #[test]
    fn impact_bar_shows_at_least_one_block_for_any_nonzero_saving() {
        assert_eq!(impact_bar(100, 100, 10), "██████████");
        assert_eq!(impact_bar(0, 100, 10), "░░░░░░░░░░");
        // 2.6% of the leader: floors to 0 cells, must still render one.
        assert_eq!(impact_bar(10_100_000, 383_400_000, 10), "█░░░░░░░░░");
        // No leader at all (an empty table) must not divide by zero.
        assert_eq!(impact_bar(0, 0, 10), "░░░░░░░░░░");
        assert_eq!(impact_bar(50, 100, 10).chars().count(), 10);
    }

    /// Long labels are elided, and the elision counts chars — a byte-sliced
    /// multi-byte command name would panic.
    #[test]
    fn fit_elides_long_labels_without_splitting_a_codepoint() {
        assert_eq!(fit("git", 14), "git");
        assert_eq!(fit("cargo clippy --all-targets", 14), "cargo clippy …");
        assert_eq!(fit("ünïcödé-command-name", 8).chars().count(), 8);
    }

    /// Averages divide by the runs that were *timed*, not by every run.
    ///
    /// History mixes entries written before and after timings existed. Dividing by
    /// all of them would drag the reported per-run overhead towards zero in
    /// proportion to how much old history the user happens to have — a number that
    /// flatters PRISM by accident, which is the opposite of what this report is for.
    #[test]
    fn timings_are_averaged_over_timed_runs_only() {
        let dir = temp_dir("gain-timed");
        let _env = TestEnv::redirect(&dir);

        save_history(History {
            commands: vec![
                timed_entry("git", 10_000, 1_000, 20, 2_000),
                timed_entry("git", 10_000, 1_000, 40, 4_000),
                // Same command, recorded before timings existed.
                entry("git", 10_000, 1_000),
            ],
            total_savings_tokens: 0,
            sessions: 0,
        })
        .unwrap();

        let report = compute_gains(false).unwrap();
        assert_eq!(report.total_commands, 3, "every run still counts as a run");
        assert_eq!(report.timed_commands, 2);
        assert_eq!(report.total_duration_ms, 60);
        assert_eq!(report.total_filter_us, 6_000);

        let git = &report.top_commands[0];
        assert_eq!(git.runs, 3);
        assert_eq!(git.timed_runs, 2);
        // 60/2 and 6000/2 — not 60/3 and 6000/3.
        assert_eq!(git.avg_duration_ms, Some(30));
        assert_eq!(git.avg_filter_us, Some(3_000));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With nothing timed, the report says so rather than claiming zero.
    ///
    /// `Some(0)` would render as "0ms" of PRISM overhead, which is a claim the
    /// data does not support; `None` renders as an em dash.
    #[test]
    fn untimed_history_reports_no_timing_rather_than_zero() {
        let dir = temp_dir("gain-untimed");
        let _env = TestEnv::redirect(&dir);

        save_history(History {
            commands: vec![entry("ps", 121_258, 6_611), entry("ps", 121_258, 6_611)],
            total_savings_tokens: 0,
            sessions: 0,
        })
        .unwrap();

        let report = compute_gains(false).unwrap();
        assert_eq!(report.timed_commands, 0);
        assert_eq!(report.total_duration_ms, 0);
        assert_eq!(report.total_filter_us, 0);
        assert_eq!(report.top_commands[0].avg_duration_ms, None);
        assert_eq!(report.top_commands[0].avg_filter_us, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A history file written by an older prism has no timing fields at all; it
    /// must still load, with both clocks reading "not measured".
    #[test]
    fn history_without_timing_fields_still_loads() {
        let dir = temp_dir("gain-legacy");
        let _env = TestEnv::redirect(&dir);

        let legacy = r#"{
  "commands": [
    {
      "timestamp": "2026-09-11T12:00:00Z",
      "command": "git",
      "input_bytes": 4000,
      "output_tokens": 0,
      "output_bytes": 400,
      "savings": 0
    }
  ],
  "total_savings_tokens": 0,
  "sessions": 0
}"#;
        std::fs::write(history_path(), legacy).unwrap();

        let report = compute_gains(false).unwrap();
        assert_eq!(report.total_commands, 1);
        assert_eq!(report.timed_commands, 0);
        assert!(
            report.total_saved_tokens > 0,
            "savings still derive from bytes"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `record_command_timed` is the path that carries `filter_us` into history;
    /// `record_command` stays a zero-timing shim so existing callers keep working.
    #[test]
    fn record_command_timed_persists_both_clocks() {
        let dir = temp_dir("gain-record");
        let _env = TestEnv::redirect(&dir);

        record_command_timed("cargo", 50_000, 2_000, 1_234, 987).unwrap();
        record_command("git", 4_000, 400).unwrap();

        let loaded = load_history().unwrap();
        assert_eq!(loaded.commands[0].duration_ms, 1_234);
        assert_eq!(loaded.commands[0].filter_us, 987);
        assert!(loaded.commands[0].is_timed());
        assert!(
            !loaded.commands[1].is_timed(),
            "an untimed record must not be counted as measured"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
    // ─── append-only journal ─────────────────────────────────────────────────

    #[test]
    fn recording_a_command_does_not_rewrite_history_json() {
        let dir = temp_dir("journal-append");
        let _env = TestEnv::redirect(&dir);

        // A big pre-existing history is exactly the case the old read-modify-write
        // made expensive: it rewrote all of this on every single command.
        let big: Vec<CommandEntry> = (0..500)
            .map(|i| entry(&format!("cmd{i}"), 100, 10))
            .collect();
        save_history(History {
            commands: big,
            total_savings_tokens: 7,
            sessions: 1,
        })
        .unwrap();
        let before = std::fs::metadata(history_path())
            .unwrap()
            .modified()
            .unwrap();

        record_command_timed("git", 1000, 100, 5, 250).unwrap();

        let after = std::fs::metadata(history_path())
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            before, after,
            "history.json must not be touched on the hot path"
        );
        assert!(
            journal_path().exists(),
            "the entry should be in the journal"
        );

        drop(_env);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_journalled_entry_is_visible_to_readers_immediately() {
        let dir = temp_dir("journal-read");
        let _env = TestEnv::redirect(&dir);

        save_history(History {
            commands: vec![entry("old", 10, 1)],
            total_savings_tokens: 3,
            sessions: 2,
        })
        .unwrap();
        record_command_timed("fresh", 900, 90, 12, 300).unwrap();

        let loaded = load_history().unwrap();

        assert_eq!(loaded.commands.len(), 2);
        assert_eq!(loaded.commands.last().unwrap().command, "fresh");
        // Fields outside the journal survive the merge.
        assert_eq!(loaded.total_savings_tokens, 3);
        assert_eq!(loaded.sessions, 2);

        drop(_env);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_torn_final_journal_line_costs_one_entry_not_the_report() {
        use std::io::Write;
        let dir = temp_dir("journal-torn");
        let _env = TestEnv::redirect(&dir);

        record_command_timed("good", 100, 10, 1, 1).unwrap();
        // Simulate being killed mid-append.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(journal_path())
            .unwrap();
        f.write_all(b"{\"command\":\"tru").unwrap();
        drop(f);

        let loaded = load_history().unwrap();

        assert_eq!(loaded.commands.len(), 1);
        assert_eq!(loaded.commands[0].command, "good");

        drop(_env);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compaction_folds_the_journal_in_without_losing_entries() {
        let dir = temp_dir("journal-compact");
        let _env = TestEnv::redirect(&dir);

        for i in 0..5 {
            record_command_timed(&format!("c{i}"), 100, 10, 1, 1).unwrap();
        }
        compact_journal().unwrap();

        assert_eq!(
            std::fs::metadata(journal_path()).unwrap().len(),
            0,
            "journal is truncated after folding in"
        );
        let loaded = load_history().unwrap();
        assert_eq!(loaded.commands.len(), 5);
        assert_eq!(loaded.commands[4].command, "c4");

        drop(_env);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_ten_thousand_entry_cap_still_applies_across_the_split() {
        let dir = temp_dir("journal-cap");
        let _env = TestEnv::redirect(&dir);

        let big: Vec<CommandEntry> = (0..10_000).map(|i| entry(&format!("c{i}"), 1, 1)).collect();
        save_history(History {
            commands: big,
            total_savings_tokens: 0,
            sessions: 0,
        })
        .unwrap();
        record_command_timed("newest", 1, 1, 1, 1).unwrap();

        let loaded = load_history().unwrap();

        assert_eq!(loaded.commands.len(), 10_000);
        assert_eq!(loaded.commands.last().unwrap().command, "newest");

        drop(_env);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_temp_file_is_swept_but_a_fresh_one_is_left_alone() {
        let dir = temp_dir("journal-sweep");
        let _env = TestEnv::redirect(&dir);
        let adir = dir.join("analytics");

        let stale = adir.join("history.json.tmp.999999");
        let fresh = adir.join("history.json.tmp.999998");
        std::fs::write(&stale, b"").unwrap();
        std::fs::write(&fresh, b"").unwrap();
        // Backdate the stale one past the one-hour threshold.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        filetime_set(&stale, old);

        save_history(History {
            commands: vec![],
            total_savings_tokens: 0,
            sessions: 0,
        })
        .unwrap();

        assert!(!stale.exists(), "an hour-old temp file should be swept");
        assert!(
            fresh.exists(),
            "a concurrent writer's temp file must survive"
        );

        drop(_env);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Set mtime without pulling in the `filetime` crate for one test.
    fn filetime_set(path: &std::path::Path, when: std::time::SystemTime) {
        let secs = when
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let out = std::process::Command::new("touch")
            .arg("-d")
            .arg(format!("@{secs}"))
            .arg(path)
            .status();
        assert!(out.map(|s| s.success()).unwrap_or(false), "touch failed");
    }
}
