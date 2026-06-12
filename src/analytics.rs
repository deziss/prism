//! Analytics — token counting, savings tracking, and dashboards.

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

pub fn prism_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("prism")
}

pub fn analytics_dir() -> PathBuf {
    prism_data_dir().join("analytics")
}

pub fn history_path() -> PathBuf {
    analytics_dir().join("history.json")
}

/// Count tokens in text for a given model.
pub fn count_tokens(text: &str, _model: &str) -> Result<usize> {
    use tiktoken_rs::cl100k_base;
    let bpe = cl100k_base()?;
    let tokens = bpe.encode_ordinary(text);
    Ok(tokens.len())
}

/// Record a command execution for analytics.
pub fn record_command(cmd: &str, input_bytes: usize, output_tokens: usize) -> Result<()> {
    let history = load_history()?;

    let entry = CommandEntry {
        timestamp: chrono::Utc::now(),
        command: cmd.to_string(),
        input_bytes,
        output_tokens,
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

/// Show token savings dashboard.
pub async fn show_gains(history_flag: bool) -> Result<()> {
    use colored::Colorize;

    let hist = load_history()?;

    if history_flag {
        println!("\n  PRISM Command History\n{}", "─".repeat(50));
        for entry in hist.commands.iter().rev().take(50) {
            let ts = entry.timestamp.format("%m-%d %H:%M");
            println!("  {}  {:>12} tokens  {}", ts, entry.output_tokens, entry.command);
        }
    } else {
        let total: usize = hist.commands.iter().map(|e| e.output_tokens).sum();
        let estimated_savings = (total as f64 * 0.65) as usize;

        println!("\n  {} PRISM Token Analytics {}\n", "═".repeat(50), "═".repeat(5));
        println!("  Total commands tracked:  {}", hist.commands.len().to_string().cyan());
        println!("  Total output tokens:      {}", total.to_string().cyan());
        println!(
            "  Estimated savings:         {} ({}%)",
            estimated_savings.to_string().green(),
            "65".green()
        );
        println!(
            "  Remaining after PRISM:    {}",
            (total.saturating_sub(estimated_savings)).to_string().yellow()
        );

        let mut by_cmd: HashMap<&str, usize> = HashMap::new();
        for e in &hist.commands {
            *by_cmd.entry(&e.command).or_default() += e.output_tokens;
        }
        let mut top: Vec<_> = by_cmd.iter().collect();
        top.sort_by(|a, b| b.1.cmp(a.1));

        println!("\n  Top commands by token usage:");
        for (cmd, tokens) in top.into_iter().take(10) {
            println!("    {:>12}  {}", tokens, cmd);
        }
    }

    Ok(())
}

/// Discover missed optimization opportunities from Claude Code history.
pub async fn discover() -> Result<()> {
    use colored::Colorize;

    println!("\n  {} PRISM Discovery {}\n", "═".repeat(50), "═".repeat(10));

    let hist = load_history()?;
    let uncached: Vec<_> = hist.commands.iter().filter(|e| !e.is_cacheable()).collect();

    println!("  Commands that could be cached: {}", uncached.len());
    for e in uncached.iter().take(5) {
        println!("    {}  ({} tokens)", e.command, e.output_tokens);
    }
    println!(
        "\n  {} opportunity{} found\n",
        uncached.len(),
        if uncached.len() == 1 { "" } else { "s" }
    );
    Ok(())
}

// --- internal types ---
#[derive(serde::Serialize, serde::Deserialize)]
pub struct History {
    commands: Vec<CommandEntry>,
    total_savings_tokens: usize,
    sessions: usize,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct CommandEntry {
    timestamp: chrono::DateTime<chrono::Utc>,
    command: String,
    input_bytes: usize,
    output_tokens: usize,
    savings: usize,
}

impl CommandEntry {
    fn is_cacheable(&self) -> bool {
        matches!(self.command.as_str(), "git" | "cargo" | "grep" | "find" | "ls")
    }
}

fn load_history() -> Result<History> {
    let path = history_path();
    if path.exists() {
        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    } else {
        Ok(History { commands: Vec::new(), total_savings_tokens: 0, sessions: 0 })
    }
}

fn save_history(history: History) -> Result<()> {
    std::fs::create_dir_all(analytics_dir())?;
    std::fs::write(history_path(), serde_json::to_string_pretty(&history)?)?;
    Ok(())
}
