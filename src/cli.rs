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

#[derive(Parser, Debug)]
pub enum HookCmd {
    Install,
    Validate,
    Audit,
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

    // Generate CA cert (needed for MITM proxy)
    let ca = crate::proxy::ensure_ca()?;
    let ca_path = crate::proxy::ca_dir().join("ca.crt");
    println!("  CA cert:      {}", ca_path.display());

    if global {
        // Write env vars to shell rc files
        write_shell_env(ca_path.to_str().unwrap_or(""))?;

        // Try to install CA to system trust store (requires sudo)
        match crate::proxy::install_ca_system(&ca.cert_pem) {
            Ok(msg) => println!("  System trust: {}", msg),
            Err(_) => {
                println!("  System trust: manual install required:");
                println!("    Linux: sudo cp {} /usr/local/share/ca-certificates/prism.crt && sudo update-ca-certificates", ca_path.display());
                println!("    macOS: sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain {}", ca_path.display());
            }
        }

        // Write Claude Code MCP config
        write_claude_mcp_config()?;
    }

    println!("\nPRISM initialized ({scope}).");
    println!("  Data dir:     {}", data_dir().display());
    println!("\nNext steps:");
    println!("  prism serve --port 8080    # start transparent LLM proxy");
    println!("  prism mcp   --port 3003    # start MCP server for Claude Code");
    if global {
        println!("  source ~/.bashrc           # reload shell env vars");
        println!("  claude mcp add prism --transport http http://localhost:3003");
    }
    println!("\nRun `prism gain` to see token savings.");
    Ok(())
}

fn write_shell_env(ca_cert_path: &str) -> Result<()> {
    let block = format!(
        "\n# PRISM — transparent LLM proxy (added by `prism init --global`)\n\
         export HTTP_PROXY=http://localhost:8080\n\
         export HTTPS_PROXY=http://localhost:8080\n\
         export NO_PROXY=localhost,127.0.0.1\n\
         export PRISM_HUB_URL=http://localhost:3002\n\
         export NODE_EXTRA_CA_CERTS={ca}\n\
         export REQUESTS_CA_BUNDLE={ca}\n\
         export SSL_CERT_FILE={ca}\n\
         # end PRISM\n",
        ca = ca_cert_path
    );

    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("~"));
    let rc_files = [".bashrc", ".zshrc", ".profile"];
    let mut wrote = false;

    for rc in &rc_files {
        let path = home.join(rc);
        if path.exists() {
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            if !existing.contains("PRISM — transparent LLM proxy") {
                let mut f = std::fs::OpenOptions::new().append(true).open(&path)?;
                use std::io::Write;
                write!(f, "{}", block)?;
                println!("  Shell env:    written to ~/{}", rc);
                wrote = true;
            } else {
                println!("  Shell env:    already in ~/{} (skipped)", rc);
                wrote = true;
            }
        }
    }

    if !wrote {
        // Create ~/.bashrc if none exist
        let path = home.join(".bashrc");
        std::fs::write(&path, format!("#!/usr/bin/env bash{}", block))?;
        println!("  Shell env:    created ~/.bashrc");
    }

    Ok(())
}

/// Register PRISM as an MCP server for Claude Code.
///
/// Claude Code reads MCP server definitions from `~/.claude.json`, not from
/// `settings.json` — an entry written to the latter is silently ignored. The
/// existing file is merged, never overwritten, since it holds unrelated state.
fn write_claude_mcp_config() -> Result<()> {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let config_path = home.join(".claude.json");

    let mut config: serde_json::Value = if config_path.exists() {
        std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if !config.is_object() {
        config = serde_json::json!({});
    }

    let root = config.as_object_mut().expect("config is an object");
    root.entry("mcpServers").or_insert_with(|| serde_json::json!({}));

    let Some(servers) = root.get_mut("mcpServers").and_then(|v| v.as_object_mut()) else {
        anyhow::bail!("~/.claude.json has a non-object mcpServers field; leaving it alone");
    };
    servers.insert(
        "prism".to_string(),
        serde_json::json!({ "type": "http", "url": "http://localhost:3003" }),
    );

    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;
    println!("  Claude MCP:   registered in ~/.claude.json");
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

// --- hook (lifecycle hooks: repeated-read detection, write validation) ---
pub async fn hook(cmd: HookCmd) -> Result<()> {
    let root = std::env::current_dir()?;
    let system = crate::hooks::HookSystem::new(&root);
    match cmd {
        HookCmd::Install => {
            let msg = system.install().map_err(|e| anyhow::anyhow!(e))?;
            println!("{msg}");
        }
        HookCmd::Validate => println!("{}", system.validate()),
        HookCmd::Audit => println!("{}", crate::hooks::hook_audit_stats(&root.join(".prism"))),
    }
    Ok(())
}

// --- compress (LLMLingua-style context compression) ---
pub async fn compress(
    string: Option<String>,
    file: Option<std::path::PathBuf>,
    ratio: f64,
) -> Result<()> {
    let text = if let Some(s) = string {
        s
    } else if let Some(f) = file {
        std::fs::read_to_string(f)?
    } else {
        anyhow::bail!("Use --string or --file");
    };
    let out = crate::compress::compress(&text, ratio);
    println!("{}", out.compressed);
    eprintln!(
        "[prism] {} -> {} tokens ({:.1}% saved)",
        out.original_tokens, out.compressed_tokens, out.savings_pct
    );
    Ok(())
}

// --- vscode (generate VS Code extension scaffold) ---
pub async fn vscode_gen(output: std::path::PathBuf) -> Result<()> {
    std::fs::create_dir_all(output.join("out"))?;
    std::fs::write(
        output.join("package.json"),
        crate::vscode::ExtensionManifest::generate(),
    )?;
    std::fs::write(
        output.join("out").join("extension.js"),
        crate::vscode::ExtensionManifest::generate_extension_js(),
    )?;
    println!("VS Code extension scaffold written to {}", output.display());
    println!("  package.json");
    println!("  out/extension.js");
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
