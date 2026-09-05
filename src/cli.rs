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
    Query {
        query: String,
        #[arg(short, long)]
        graph: Option<std::path::PathBuf>,
        #[arg(long, default_value_t = false)]
        graphify: bool,
    },
    Explain {
        node: String,
        #[arg(short, long)]
        graph: Option<std::path::PathBuf>,
    },
    Path {
        from: String,
        to: String,
        #[arg(short, long)]
        graph: Option<std::path::PathBuf>,
    },
    GodNodes {
        #[arg(short, long, default_value_t = 10)]
        top: usize,
        #[arg(short, long)]
        graph: Option<std::path::PathBuf>,
    },
    Import {
        path: std::path::PathBuf,
    },
    Extract { source: String },
    Export { output: String },
    Stats {
        #[arg(short, long)]
        graph: Option<std::path::PathBuf>,
    },
    Index {
        #[arg(default_value = ".")]
        path: std::path::PathBuf,
        #[arg(long)]
        from_graphify: Option<std::path::PathBuf>,
    },
}

#[derive(Parser, Debug)]
pub enum CacheCmd {
    Stats,
    Clear,
    Query { query: String },
    Put {
        #[arg(short, long)]
        prompt: String,
        #[arg(short, long)]
        response: String,
    },
}

#[derive(Parser, Debug)]
pub struct ConfigCmd {
    #[arg(long, default_value_t = false)]
    pub show: bool,
    #[arg(long)]
    pub set_key: Option<String>,
    #[arg(long)]
    pub set_val: Option<String>,
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
pub async fn init(global: bool, guide: bool) -> Result<()> {
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

    println!("
PRISM initialized ({scope}).");
    println!("  Data dir:     {}", data_dir().display());
    println!("
Next steps:");
    println!("  prism serve --port 27181    # start transparent LLM proxy");
    println!("  prism mcp   --port 27182    # start MCP server for Claude Code");
    if global {
        println!("  source ~/.bashrc           # reload shell env vars");
        println!("  claude mcp add prism --transport http http://localhost:27182");
    }
    println!("
Run prism gain to see token savings.");

    if guide {
        crate::guide::show_guide(None);
    } else {
        use std::io::Write;
        print!("
  [?] Explore the PRISM Interactive User Guide now? [y/N]: ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_ok() {
            let answer = line.trim().to_lowercase();
            if answer == "y" || answer == "yes" {
                crate::guide::show_guide(None);
            }
        }
    }

    Ok(())
}

// --- guide ---
pub async fn guide(topic: Option<String>) -> Result<()> {
    crate::guide::show_guide(topic.as_deref());
    Ok(())
}

fn write_shell_env(ca_cert_path: &str) -> Result<()> {
    let block = format!(
        "\n# PRISM — transparent LLM proxy (added by `prism init --global`)\n\
         export HTTP_PROXY=http://localhost:27181\n\
         export HTTPS_PROXY=http://localhost:27181\n\
         export NO_PROXY=localhost,127.0.0.1\n\
         export PRISM_HUB_URL=http://localhost:27183\n\
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
        serde_json::json!({ "type": "http", "url": "http://localhost:27182" }),
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
        GraphCmd::Query { query, graph, graphify } => {
            if graphify {
                crate::knowledge::explain_node(&query, graph.as_deref()).await
            } else {
                crate::knowledge::query_graph(&query, graph.as_deref()).await
            }
        }
        GraphCmd::Explain { node, graph } => {
            crate::knowledge::explain_node(&node, graph.as_deref()).await
        }
        GraphCmd::Path { from, to, graph } => {
            crate::knowledge::shortest_path(&from, &to, graph.as_deref()).await
        }
        GraphCmd::GodNodes { top, graph } => {
            crate::knowledge::god_nodes(top, graph.as_deref()).await
        }
        GraphCmd::Import { path } => {
            crate::knowledge::import_graph(&path).await
        }
        GraphCmd::Extract { source } => crate::knowledge::extract_from_source(&source).await,
        GraphCmd::Export { output } => crate::knowledge::export_to_obsidian(&output).await,
        GraphCmd::Stats { graph } => crate::knowledge::graph_stats(graph.as_deref()).await,
        GraphCmd::Index { path, from_graphify } => {
            if let Some(gf) = from_graphify {
                crate::knowledge::import_graph(&gf).await
            } else {
                crate::knowledge::index_codebase(&path).await
            }
        }
    }
}

// --- read ---
pub async fn read(
    path: std::path::PathBuf,
    mode_str: String,
    lines: Option<String>,
    line_numbers: bool,
) -> Result<()> {
    let mode = if let Some(range) = lines {
        crate::reader::parse_lines_range(&range)?
    } else {
        mode_str.parse::<crate::reader::ReadMode>()?
    };

    let out = crate::reader::read_file(&path, mode, line_numbers)?;
    println!("{}", out.content);
    if !out.is_cached_receipt {
        eprintln!(
            "[prism read] {} ({}) — {} -> {} tokens ({:.1}% saved)",
            path.display(), out.mode_used, out.original_tokens, out.returned_tokens, out.savings_pct
        );
    }
    Ok(())
}

// --- cache ---
pub async fn cache(cmd: CacheCmd) -> Result<()> {
    match cmd {
        CacheCmd::Stats => {
            let stats = crate::cache::get_cache_stats();
            println!("\n  PRISM Semantic Cache Statistics");
            println!("  {}", "═".repeat(40));
            println!("  Total entries: {}", stats.total_entries);
            println!("  Storage path:  {}", stats.sled_path);
        }
        CacheCmd::Clear => {
            let cleared = crate::cache::clear_cache()?;
            println!("Cleared {} cache entries.", cleared);
        }
        CacheCmd::Query { query } => {
            let results = crate::cache::lookup_similar(&query, 5);
            if results.is_empty() {
                println!("No cached entries found for: {}", query);
            } else {
                println!("Cache matches for '{}':\n", query);
                for (i, entry) in results.iter().enumerate() {
                    println!("  [{}] hash: {} | model: {}", i + 1, entry.key_hash, entry.model.as_deref().unwrap_or("unknown"));
                    let preview = if entry.response.len() > 150 { &entry.response[..150] } else { &entry.response };
                    println!("      {}\n", preview);
                }
            }
        }
        CacheCmd::Put { prompt, response } => {
            crate::cache::cache_response(&prompt, &response, "cli");
            println!("Cached response for prompt: \"{}\"", prompt);
        }
    }
    Ok(())
}

// --- config ---
pub async fn config(cmd: ConfigCmd) -> Result<()> {
    let global_cfg = crate::config::load_global().unwrap_or_default();
    if cmd.show || (cmd.set_key.is_none() && cmd.set_val.is_none()) {
        println!("\n  PRISM Configuration");
        println!("  {}", "═".repeat(40));
        println!("{}", crate::config::config_to_json(&global_cfg));
    } else if let (Some(key), Some(val)) = (cmd.set_key, cmd.set_val) {
        let mut cfg = global_cfg;
        match key.as_str() {
            "compression_ratio" => {
                if let Ok(r) = val.parse::<f64>() {
                    cfg.compression_ratio = Some(r);
                }
            }
            "cache_enabled" => {
                cfg.cache_enabled = val.parse::<bool>().unwrap_or(true);
            }
            "toon_enabled" => {
                cfg.toon_enabled = val.parse::<bool>().unwrap_or(true);
            }
            "proxy_port" => {
                cfg.proxy_port = val.parse::<u16>().ok();
            }
            "mcp_port" => {
                cfg.mcp_port = val.parse::<u16>().ok();
            }
            "tiktoken_model" => {
                cfg.tiktoken_model = Some(val.clone());
            }
            _ => {
                println!("Unknown config key: {}. (Supported: compression_ratio, cache_enabled, toon_enabled, proxy_port, mcp_port, tiktoken_model)", key);
                return Ok(());
            }
        }
        crate::config::save_global(&cfg)?;
        println!("Updated config: {} = {}", key, val);
    }
    Ok(())
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

    let raw_text = if stdout.is_empty() && !stderr.is_empty() {
        stderr.to_string()
    } else if !stderr.is_empty() && !output.status.success() {
        format!("{}\n{}", stdout, stderr)
    } else {
        stdout.clone()
    };

    // Filter output through RTK-compatible filters
    let filtered = crate::filter::filter_output(&raw_text, cmd, &args[1..]);
    let _ = std::io::stdout().write_all(filtered.as_bytes());
    if !filtered.ends_with('\n') && !filtered.is_empty() {
        let _ = std::io::stdout().write_all(b"\n");
    }

    // Track tokens
    if let Ok(tokens) = crate::analytics::count_tokens(&filtered, "gpt-4") {
        crate::analytics::record_command(cmd, filtered.len(), tokens).ok();
    }

    // RTK-style Failure Tee Mechanism: preserve raw output on command failure
    if !output.status.success() {
        let tee_dir = crate::prism_data_dir().join("tee");
        if std::fs::create_dir_all(&tee_dir).is_ok() {
            let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
            let safe_cmd: String = cmd.chars().filter(|c| c.is_alphanumeric()).collect();
            let tee_file = tee_dir.join(format!("{}_{}.log", safe_cmd, ts));
            let raw_combined = format!(
                "COMMAND: {} {:?}\nEXIT CODE: {}\n\n--- RAW STDOUT ---\n{}\n\n--- RAW STDERR ---\n{}",
                cmd, &args[1..], output.status.code().unwrap_or(-1), stdout, stderr
            );
            let _ = std::fs::write(&tee_file, raw_combined);
            eprintln!(
                "[prism] Command failed (exit code {}). Raw output preserved at: {}",
                output.status.code().unwrap_or(1),
                tee_file.display()
            );
        }
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
