//! CLI subcommands for PRISM.

use anyhow::Result;
use clap::Parser;
use std::process::Command;

// Re-export command types so main.rs can use them
#[derive(Parser, Debug)]
pub enum MemoryCmd {
    Search { query: String },
    Save { key: String, value: String },
    List,
    Stats,
    Compact,
}

#[derive(Parser, Debug)]
pub enum GraphCmd {
    Query { query: String },
    Extract { source: String },
    Export { output: String },
    Stats,
}

#[derive(Parser, Debug)]
pub enum ToonCmd {
    Encode { input: String },
    Decode { input: String },
}

fn data_dir() -> std::path::PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("prism")
}

// --- init ---
pub async fn init(global: bool) -> Result<()> {
    let scope = if global { "global" } else { "local" };
    println!("Initializing PRISM ({scope})...");

    crate::hook::install(global).await?;
    init_data_dirs().await?;

    println!("\nPRISM initialized ({scope}).");
    println!("  Hook engine:  installed");
    println!("  Data dir:     {}", data_dir().display());
    println!("\nRun `prism gain` to see token savings.");
    Ok(())
}

// --- proxy ---
pub async fn proxy(cmd: Vec<String>) -> Result<()> {
    if cmd.is_empty() {
        anyhow::bail!("Usage: prism proxy <cmd> [args...]");
    }
    let mut c = Command::new(&cmd[0]);
    c.args(&cmd[1..]);
    let status = c.status()?;
    std::process::exit(status.code().unwrap_or(1));
}

// --- memory ---
pub async fn memory(cmd: MemoryCmd) -> Result<()> {
    match cmd {
        MemoryCmd::Search { query } => crate::memory::search(&query).await,
        MemoryCmd::Save { key, value } => crate::memory::save(&key, &value).await,
        MemoryCmd::List => crate::memory::list().await,
        MemoryCmd::Stats => crate::memory::stats().await,
        MemoryCmd::Compact => crate::memory::compact().await,
    }
}

// --- graph ---
pub async fn graph(cmd: GraphCmd) -> Result<()> {
    match cmd {
        GraphCmd::Query { query } => crate::knowledge::query_graph(&query).await,
        GraphCmd::Extract { source } => crate::knowledge::extract_from_source(&source).await,
        GraphCmd::Export { output } => crate::knowledge::export_to_obsidian(&output).await,
        GraphCmd::Stats => crate::knowledge::graph_stats().await,
    }
}

// --- toon ---
pub async fn toon(cmd: ToonCmd) -> Result<()> {
    match cmd {
        ToonCmd::Encode { input } => {
            let json: serde_json::Value = serde_json::from_str(&input)?;
            let toon_str = crate::encode::encode_json_to_toon(&json)?;
            println!("{toon_str}");
        }
        ToonCmd::Decode { input } => {
            let json = crate::encode::decode_toon_to_json(&input)?;
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
    }
    Ok(())
}

// --- count ---
pub async fn count(
    string: Option<String>,
    file: Option<std::path::PathBuf>,
    model: String,
) -> Result<()> {
    let text = if let Some(s) = string {
        s
    } else if let Some(f) = file {
        std::fs::read_to_string(f)?
    } else {
        anyhow::bail!("Use --string or --file");
    };
    let tokens = crate::analytics::count_tokens(&text, &model)?;
    println!("{tokens}");
    Ok(())
}

// --- rtk-style command runner ---
pub async fn run_command(args: Vec<String>) -> Result<()> {
    use std::io::Write;

    if args.is_empty() {
        anyhow::bail!("Usage: prism <cmd> [args...]");
    }

    let cmd = &args[0];

    // Run actual command
    let mut c = Command::new(cmd);
    c.args(&args[1..]);
    let output = c.output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Filter output through RTK-compatible filters
    let filtered = crate::filter::filter_output(&stdout, cmd, &args[1..]);

    if !output.stderr.is_empty() && !stderr.contains("error") && stderr.len() < 200 {
        let combined = format!("{}\n{}", filtered, stderr);
        let _ = std::io::stdout().write_all(combined.as_bytes());
    } else {
        let _ = std::io::stdout().write_all(filtered.as_bytes());
    }

    // Track tokens
    if let Ok(tokens) = crate::analytics::count_tokens(&filtered, "gpt-4") {
        crate::analytics::record_command(cmd, filtered.len(), tokens).ok();
    }

    std::process::exit(output.status.code().unwrap_or(0))
}

// --- helpers ---
async fn init_data_dirs() -> Result<()> {
    let data_dir = data_dir();
    std::fs::create_dir_all(data_dir.join("memory"))?;
    std::fs::create_dir_all(data_dir.join("graph"))?;
    std::fs::create_dir_all(data_dir.join("cache"))?;
    std::fs::create_dir_all(data_dir.join("analytics"))?;
    Ok(())
}
