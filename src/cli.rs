//! CLI subcommands for PRISM.

use anyhow::Result;
use clap::Parser;
use std::process::Command;

// Re-export command types so main.rs can use them
#[derive(Parser, Debug)]
pub enum MemoryCmd {
    Search {
        query: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Save {
        key: String,
        value: String,
    },
    List,
    Stats {
        #[arg(long, default_value_t = false)]
        json: bool,
    },
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
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Explain {
        node: String,
        #[arg(short, long)]
        graph: Option<std::path::PathBuf>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Path {
        from: String,
        to: String,
        #[arg(short, long)]
        graph: Option<std::path::PathBuf>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    GodNodes {
        #[arg(short, long, default_value_t = 10)]
        top: usize,
        #[arg(short, long)]
        graph: Option<std::path::PathBuf>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Import {
        path: std::path::PathBuf,
    },
    Extract {
        source: String,
    },
    Export {
        output: String,
    },
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
    Stats {
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Clear,
    Query {
        query: String,
    },
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
    Encode {
        input: String,
    },
    Decode {
        input: String,
    },
    /// Render JSON as a TRON box-drawing table (display format, not round-trippable)
    Tron {
        input: String,
    },
}

#[derive(Parser, Debug)]
pub enum ShimCmd {
    /// Write a PATH shim for every filtered tool present on this machine
    Install {
        /// Also add the PATH line to your shell rc files
        #[arg(long)]
        path: bool,
    },
    /// Remove the shims prism generated
    Uninstall,
    /// Show whether the shims are installed and actually winning the PATH lookup
    Status,
    /// Print the shim directory
    Path,
}

#[derive(Parser, Debug)]
pub enum HookCmd {
    Install,
    /// Remove the project-local hook templates from `.prism/hooks/`.
    ///
    /// Note this is the *project* hook system (`src/hooks.rs`), not the PATH shims —
    /// those are `prism shim install|uninstall`. The two have unfortunately similar
    /// names; the shims are the ones that touch your shell.
    Uninstall,
    Validate,
    Audit,
}

#[derive(Parser, Debug)]
pub enum HubCmd {
    /// Exchange a team join token for a durable agent token
    Enroll {
        /// Hub base URL, e.g. http://localhost:27183
        #[arg(long)]
        url: String,
        /// Short-lived team join token
        #[arg(long)]
        token: String,
        /// How this agent appears in the hub's Fleet list. Defaults to the hostname.
        #[arg(long)]
        name: Option<String>,
        /// Advertise where this agent's MCP Streamable HTTP server is reachable, so the
        /// hub can drive its memory/graph tools. Omit for a local-only install.
        #[arg(long, requires = "mcp_port")]
        mcp_host: Option<String>,
        #[arg(long, requires = "mcp_host")]
        mcp_port: Option<u16>,
    },
    /// Show enrollment state and spool depth
    Status {
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Force an immediate drain-and-send of the on-disk spool
    Flush,
    /// Fetch hub-enforced policy into `<data>/hub-config.yaml`
    Config,
    /// Send one synthetic event and report success/failure
    Test {
        /// Send to this URL instead of the enrolled hub — point it at
        /// `python3 -m http.server` or `nc -l` to inspect the raw request body.
        #[arg(long)]
        url: Option<String>,
    },
}

fn data_dir() -> std::path::PathBuf {
    crate::prism_data_dir()
}

/// `prism status` — one place that answers "what has prism actually done to this machine?"
///
/// Previously that question had no single answer: `prism shim status` covered shims,
/// `prism hub status` covered enrolment, and nothing at all reported the rc PATH block,
/// the CA trust, or the Claude Code MCP registration. Since install used to enable
/// several of those silently, "is it on?" was genuinely hard to establish.
pub async fn status() -> Result<()> {
    use colored::Colorize;

    let on = |b: bool| {
        if b {
            "on".green().to_string()
        } else {
            "off".dimmed().to_string()
        }
    };

    println!(
        "\n  {}  {}",
        "PRISM STATUS".bold().cyan(),
        concat!("v", env!("CARGO_PKG_VERSION")).dimmed()
    );
    println!("  {}", "─".repeat(65).dimmed());

    // Shims + the rc block that makes them reachable. Both matter: shims installed but
    // not on PATH do nothing, and a PATH block with no shims behind it is the state
    // that used to break every command.
    let sh = crate::shim::status();
    println!(
        "  {:<22} {}  ({} installed, {})",
        "PATH shims:",
        on(sh.active),
        sh.installed,
        if sh.active {
            "ahead of the real tools"
        } else {
            "not on PATH"
        }
    );
    println!("  {:<22} {}", "  directory:", sh.dir.display());
    println!("  {:<22} {:?}", "  mode:", sh.mode);
    if sh.installed > 0 && !sh.active {
        println!(
            "  {:<22} {}",
            "",
            "shims exist but nothing uses them — `prism shim install --path`".dimmed()
        );
    }

    // CA trust.
    let ca = crate::proxy::ca_dir().join("ca.crt");
    println!("  {:<22} {}", "CA generated:", on(ca.exists()));
    let nss_trusted = std::process::Command::new("certutil")
        .args(["-d", "sql:.", "-L"])
        .output()
        .is_ok();
    if !nss_trusted {
        println!(
            "  {:<22} {}",
            "  browser trust:",
            "certutil not installed — cannot tell".dimmed()
        );
    }

    // Claude Code MCP registration.
    let claude_json = dirs::home_dir().unwrap_or_default().join(".claude.json");
    let mcp_registered = std::fs::read_to_string(&claude_json)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| {
            v.get("mcpServers")
                .and_then(|m| m.get("prism"))
                .map(|_| true)
        })
        .unwrap_or(false);
    println!("  {:<22} {}", "Claude Code MCP:", on(mcp_registered));

    // Hub enrolment.
    let hub = crate::hub::status();
    println!("  {:<22} {}", "Hub enrolled:", on(hub.enrolled));

    println!("  {}\n", "─".repeat(65).dimmed());
    Ok(())
}

// --- init ---
pub async fn init(global: bool, guide: bool, shims: bool, trust_ca: bool) -> Result<()> {
    let scope = if global { "global" } else { "local" };
    println!("Initializing PRISM ({scope})...");

    // Shims are opt-in.
    //
    // This used to be the first unconditional statement of init, and `install.sh` ran
    // `init --global < /dev/null`, so a plain install put prism in front of ~102
    // commands — `ls`, `grep`, `find`, `env`, `ps`, `systemctl` among them — and
    // rewrote every shell rc file, with no prompt and no way to undo it from the CLI.
    //
    // The failure mode that argues hardest for this: a shim's exec target is recorded
    // when the shim is written, so a moved or deleted prism takes `ls` and `grep` with
    // it — removing the tools needed to diagnose the breakage.
    if shims {
        crate::hook::install(global).await?;
    }
    init_data_dirs().await?;

    // Generate CA cert (needed for MITM proxy)
    let ca = crate::proxy::ensure_ca()?;
    let ca_path = crate::proxy::ca_dir().join("ca.crt");
    println!("  CA cert:      {}", ca_path.display());

    if global {
        // A combined bundle (system roots + PRISM CA): `SSL_CERT_FILE` and friends
        // replace the trust store, so they must never point at the bare CA.
        let bundle = crate::proxy::ensure_ca_bundle(&ca.cert_pem)?;
        println!("  CA bundle:    {}", bundle.display());

        // Safeguard: Ensure no legacy ~/.config/environment.d/10-prism.conf exists
        if let Some(home) = dirs::home_dir() {
            let env_d_prism = home
                .join(".config")
                .join("environment.d")
                .join("10-prism.conf");
            if env_d_prism.exists() {
                let _ = std::fs::remove_file(&env_d_prism);
                println!("  Cleaned legacy environment.d/10-prism.conf");
            }
        }
        // Scrub any lingering proxy exports from systemd user session
        let _ = std::process::Command::new("systemctl")
            .args([
                "--user",
                "unset-environment",
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
            ])
            .output();

        // Write env vars to shell rc files
        write_shell_env(
            ca_path.to_str().unwrap_or(""),
            bundle.to_str().unwrap_or(""),
        )?;

        // Chromium/Electron apps (VS Code, Antigravity, …) and Firefox read NSS, not
        // the system store — without this they reject every intercepted host.
        //
        // Opt-in, because this is a man-in-the-middle root. It is only needed once
        // traffic is actually routed through the proxy, and an unattended installer
        // should not put an interception CA into someone's browser on their behalf.
        if trust_ca {
            match crate::proxy::install_ca_nss(&ca.cert_pem) {
                Ok(dbs) if !dbs.is_empty() => {
                    println!("  NSS trust:    {} database(s) updated", dbs.len())
                }
                Ok(_) => println!(
                    "  NSS trust:    no NSS database found (install libnss3-tools if an IDE or browser rejects certs)"
                ),
                Err(e) => println!("  NSS trust:    failed: {}", e),
            }
        } else {
            println!(
                "  NSS trust:    not installed (re-run with --trust-ca once you route traffic through the proxy)"
            );
        }

        // The system trust store is deliberately NOT written.
        //
        // This used to call install_ca_system(), which writes
        // /usr/local/share/ca-certificates/prism.crt and runs
        // update-ca-certificates. Nothing in the project ever removed it, while
        // uninstalling *did* delete the CA private key under the data directory —
        // so a "complete removal" could leave the machine trusting an interception
        // root whose key no longer exists and which nobody would think to look for.
        //
        // NSS (above) already covers the browsers and Electron IDEs that actually
        // need it, and it is removable. Anyone who genuinely wants the system-wide
        // anchor can add it deliberately, and is told here how to take it back out.
        println!("  System trust: not modified (NSS above covers browsers and IDEs)");
        println!("    To add it system-wide anyway:");
        println!(
            "      sudo cp {} /usr/local/share/ca-certificates/prism.crt && sudo update-ca-certificates",
            ca_path.display()
        );
        println!("    To undo that later:");
        println!(
            "      sudo rm -f /usr/local/share/ca-certificates/prism.crt && sudo update-ca-certificates --fresh"
        );

        // Write Claude Code MCP config
        write_claude_mcp_config()?;
    }

    println!(
        "
PRISM initialized ({scope})."
    );
    println!("  Data dir:     {}", data_dir().display());
    println!(
        "
Next steps:"
    );
    println!("  prism serve --port 27181    # start transparent LLM proxy");
    println!("  prism mcp   --stdio         # MCP server for a local agent (already");
    println!("                                registered in ~/.claude.json if --global)");
    if global {
        println!("  source ~/.bashrc           # reload shell env vars");
    }
    println!(
        "
Run prism gain to see token savings."
    );

    if guide {
        crate::guide::show_guide(None);
    } else {
        use std::io::Write;
        print!(
            "
  [?] Explore the PRISM Interactive User Guide now? [y/N]: "
        );
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

/// Strip PRISM's legacy proxy exports from the user's shell startup files.
///
/// This used to also *write* a block exporting `PRISM_HUB_URL` into `.bashrc`,
/// `.zshrc` and `.profile`. Nothing in prism has ever read that variable — the hub
/// URL comes only from the credentials written by `prism hub enroll`
/// (`hub::load_credentials`) — so init was permanently editing three shell files for
/// a value with no effect, and TROUBLESHOOT.md told users the shipper would not start
/// without it.
///
/// The cleaning half is kept and is the entire point now: older versions really did
/// export `HTTP_PROXY`/`SSL_CERT_FILE` here, and those linger in rc files long after
/// the proxy is gone. Removing them is a fix; adding anything back is not.
///
/// Global `HTTP_PROXY`/`HTTPS_PROXY` are never written: they break system tools and
/// IDEs whenever the proxy is down. Use `prism-env <cmd>` or `eval $(prism proxy env)`.
fn write_shell_env(_ca_cert_path: &str, _ca_bundle_path: &str) -> Result<()> {
    let block = String::new();

    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("~"));
    let rc_files = [".bashrc", ".zshrc", ".profile"];

    for rc in &rc_files {
        let path = home.join(rc);
        if path.exists() {
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            if existing.contains("PRISM — transparent LLM proxy") {
                let cleaned: Vec<&str> = existing
                    .lines()
                    .filter(|line| {
                        !line.contains("PRISM — transparent LLM proxy")
                            && !line.contains("HTTP_PROXY=http://localhost:27181")
                            && !line.contains("HTTPS_PROXY=http://localhost:27181")
                            && !line.contains("NO_PROXY=localhost,127.0.0.1,::1")
                            && !line.contains("NODE_EXTRA_CA_CERTS=")
                            && !line.contains("REQUESTS_CA_BUNDLE=")
                            && !line.contains("CURL_CA_BUNDLE=")
                            && !line.contains("SSL_CERT_FILE=")
                    })
                    .collect();
                let mut new_text = cleaned.join("\n");
                if !new_text.is_empty() && !new_text.ends_with('\n') {
                    new_text.push('\n');
                }
                new_text.push_str(&block);
                std::fs::write(&path, new_text)?;
                println!("  Shell env:    cleaned legacy proxy & updated ~/{}", rc);
            }
            // No `else` branch: there is nothing to add. init only removes.
        }
    }

    Ok(())
}

/// Register PRISM as an MCP server for Claude Code.
///
/// Claude Code reads MCP server definitions from `~/.claude.json`, not from
/// `settings.json` — an entry written to the latter is silently ignored. The
/// existing file is merged, never overwritten, since it holds unrelated state.
///
/// Prefers stdio: a local, same-machine agent gets no network surface at all, and
/// prism's stdio transport (added alongside this rmcp rewrite) is the intended default
/// for exactly this case. The streamable-HTTP transport remains the right choice for
/// the hub's *remote* MCP client, but that entry is on the hub side, not written here.
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
    root.entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    let Some(servers) = root.get_mut("mcpServers").and_then(|v| v.as_object_mut()) else {
        anyhow::bail!("~/.claude.json has a non-object mcpServers field; leaving it alone");
    };
    // Absolute path: a relative "prism" would depend on PATH at the time Claude Code
    // itself launches the server, which is not guaranteed to be a shell that has run
    // rc files at all (GUI-launched clients often do not).
    let prism_bin = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "prism".to_string());
    servers.insert(
        "prism".to_string(),
        serde_json::json!({
            "type": "stdio",
            "command": prism_bin,
            "args": ["mcp", "--stdio"]
        }),
    );

    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;
    println!("  Claude MCP:   registered in ~/.claude.json (stdio transport)");
    println!(
        "                alternative for a remote/hub client: {{\"type\": \"http\", \"url\": \"http://localhost:27182/mcp\"}}"
    );
    Ok(())
}

// --- proxy ---
pub async fn proxy(cmd: Vec<String>) -> Result<()> {
    if cmd.is_empty() {
        println!("Run a command through the PRISM proxy, or manage proxy variables:");
        println!("\n  Run command: prism proxy <cmd> [args...]");
        println!("  Shell eval:  eval $(prism proxy env)");
        println!("  Turn off:    eval $(prism proxy off)");
        println!("  Status:      prism proxy status");
        return Ok(());
    }
    if cmd.len() == 1 && cmd[0] == "env" {
        println!("export HTTP_PROXY=http://localhost:27181");
        println!("export HTTPS_PROXY=http://localhost:27181");
        println!("export NO_PROXY=localhost,127.0.0.1,::1");
        let ca_path = crate::proxy::ca_dir().join("ca.crt");
        if ca_path.exists() {
            println!("export NODE_EXTRA_CA_CERTS={}", ca_path.display());
        }
        return Ok(());
    }
    if cmd.len() == 1 && (cmd[0] == "off" || cmd[0] == "unset") {
        println!(
            "unset HTTP_PROXY HTTPS_PROXY http_proxy https_proxy ALL_PROXY all_proxy NODE_EXTRA_CA_CERTS"
        );
        return Ok(());
    }
    if cmd.len() == 1 && cmd[0] == "status" {
        let is_running = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(500))
            .build()?
            .get("http://127.0.0.1:27181/health")
            .send()
            .await
            .is_ok();
        if is_running {
            println!("  PRISM Proxy is active on http://127.0.0.1:27181");
        } else {
            println!("  PRISM Proxy is NOT running on http://127.0.0.1:27181");
            println!("  Start proxy daemon with: prism serve --port 27181");
        }
        let env_http = std::env::var("HTTP_PROXY").or_else(|_| std::env::var("http_proxy"));
        let env_https = std::env::var("HTTPS_PROXY").or_else(|_| std::env::var("https_proxy"));
        if let Ok(p) = env_http {
            println!("  Current shell HTTP_PROXY: {}", p);
            if !is_running && p.contains("27181") {
                eprintln!(
                    "  WARNING: Current shell points to PRISM proxy on 27181, but proxy is offline!"
                );
                eprintln!(
                    "  Run `eval $(prism proxy off)` to restore direct internet connectivity."
                );
            }
        }
        if let Ok(p) = env_https {
            println!("  Current shell HTTPS_PROXY: {}", p);
        }
        return Ok(());
    }
    let mut c = Command::new(&cmd[0]);
    c.args(&cmd[1..]);
    c.env("HTTP_PROXY", "http://localhost:27181");
    c.env("HTTPS_PROXY", "http://localhost:27181");
    c.env("http_proxy", "http://localhost:27181");
    c.env("https_proxy", "http://localhost:27181");
    c.env("NO_PROXY", "localhost,127.0.0.1,::1");
    let ca_path = crate::proxy::ca_dir().join("ca.crt");
    if ca_path.exists() {
        c.env("NODE_EXTRA_CA_CERTS", ca_path);
    }
    let status = c.status()?;
    std::process::exit(status.code().unwrap_or(1));
}

// --- memory ---
pub async fn memory(cmd: MemoryCmd) -> Result<()> {
    match cmd {
        MemoryCmd::Search { query, json } => {
            if json {
                let blocks = crate::memory::search_blocks(&query, 10)?;
                println!("{}", serde_json::to_string_pretty(&blocks)?);
                Ok(())
            } else {
                crate::memory::search(&query).await
            }
        }
        MemoryCmd::Save { key, value } => crate::memory::save(&key, &value).await,
        MemoryCmd::List => crate::memory::list().await,
        MemoryCmd::Stats { json } => {
            if json {
                let stats = crate::memory::stats_data()?;
                println!("{}", serde_json::to_string_pretty(&stats)?);
                Ok(())
            } else {
                crate::memory::stats().await
            }
        }
        MemoryCmd::Compact => crate::memory::compact().await,
    }
}

/// The two hub-policy features, resolved.
///
/// The JSON above prints `null` for a flag no layer states, which does not answer the
/// question a user actually has after `prism graph` refused — is it on? — nor say where
/// the answer came from.
fn print_hub_feature_state(cfg: &crate::config::PrismConfig) {
    let state = |stated: Option<bool>, effective: bool| {
        format!(
            "{:<8}  ({})",
            if effective { "enabled" } else { "disabled" },
            match stated {
                Some(v) => format!("set to {v} by a config layer"),
                None => "not set by any layer — default".to_string(),
            }
        )
    };
    println!("\n  Hub-policy features");
    println!("  {}", "─".repeat(40));
    println!(
        "  GraphRAG       (prism graph)  {}",
        state(cfg.graph_enabled, cfg.graph_enabled())
    );
    println!(
        "  Semantic cache (prism cache)  {}",
        state(cfg.cache_enabled, cfg.cache_enabled())
    );
    if !cfg.graph_enabled() || !cfg.cache_enabled() {
        println!("  Enabled by policy from a licensed PRISM Hub:");
        println!("    prism hub enroll --url <hub> --token <join-token>");
    }
}

// --- graph ---
pub async fn graph(cmd: GraphCmd) -> Result<()> {
    graph_gated(cmd, crate::knowledge::graph_enabled()).await
}

/// `graph` with the policy decision injected.
///
/// One gate for all nine subcommands, up front and loud: with GraphRAG off the `--json`
/// arms below would otherwise print `null` and exit 0, which an agent or a script reads
/// as "no results" rather than "this feature is not enabled here". Taking `enabled` as
/// an argument keeps that gate testable without a config file on disk.
async fn graph_gated(cmd: GraphCmd, enabled: bool) -> Result<()> {
    if !enabled {
        anyhow::bail!("{}", crate::knowledge::GraphUnavailable::Disabled.message());
    }
    match cmd {
        GraphCmd::Query {
            query,
            graph,
            graphify,
            json,
        } => {
            if json {
                let data = crate::knowledge::query_graph_data(&query, 8, graph.as_deref());
                println!(
                    "{}",
                    serde_json::to_string_pretty(&data.map(|(p, n)| serde_json::json!({
                        "source": p.display().to_string(),
                        "nodes": n,
                    })))?
                );
                Ok(())
            } else if graphify {
                crate::knowledge::explain_node(&query, graph.as_deref()).await
            } else {
                crate::knowledge::query_graph(&query, graph.as_deref()).await
            }
        }
        GraphCmd::Explain { node, graph, json } => {
            if json {
                let data = crate::knowledge::explain_node_data(&node, graph.as_deref());
                println!(
                    "{}",
                    serde_json::to_string_pretty(&data.map(|(p, exp)| serde_json::json!({
                        "source": p.display().to_string(),
                        "node": exp.node,
                        "outgoing": exp.outgoing,
                        "incoming": exp.incoming,
                    })))?
                );
                Ok(())
            } else {
                crate::knowledge::explain_node(&node, graph.as_deref()).await
            }
        }
        GraphCmd::Path {
            from,
            to,
            graph,
            json,
        } => {
            if json {
                let data = crate::knowledge::shortest_path_data(&from, &to, graph.as_deref());
                println!(
                    "{}",
                    serde_json::to_string_pretty(&data.map(|(p, steps)| serde_json::json!({
                        "source": p.display().to_string(),
                        "steps": steps,
                    })))?
                );
                Ok(())
            } else {
                crate::knowledge::shortest_path(&from, &to, graph.as_deref()).await
            }
        }
        GraphCmd::GodNodes { top, graph, json } => {
            if json {
                let data = crate::knowledge::god_nodes_data(top, graph.as_deref());
                println!(
                    "{}",
                    serde_json::to_string_pretty(&data.map(|(p, nodes)| serde_json::json!({
                        "source": p.display().to_string(),
                        "nodes": nodes,
                    })))?
                );
                Ok(())
            } else {
                crate::knowledge::god_nodes(top, graph.as_deref()).await
            }
        }
        GraphCmd::Import { path } => crate::knowledge::import_graph(&path).await,
        GraphCmd::Extract { source } => crate::knowledge::extract_from_source(&source).await,
        GraphCmd::Export { output } => crate::knowledge::export_to_obsidian(&output).await,
        GraphCmd::Stats { graph } => crate::knowledge::graph_stats(graph.as_deref()).await,
        GraphCmd::Index {
            path,
            from_graphify,
        } => {
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
            path.display(),
            out.mode_used,
            out.original_tokens,
            out.returned_tokens,
            out.savings_pct
        );
    }
    Ok(())
}

// --- cache ---
pub async fn cache(cmd: CacheCmd) -> Result<()> {
    cache_gated(cmd, crate::cache::unavailable()).await
}

/// `cache` with the availability decision injected.
///
/// Two reasons a cache command cannot run, and neither of them is an empty cache: the
/// feature is disabled, or sled's directory lock is held by `prism serve`. Every command
/// below would otherwise report `Total entries: 0` for both, sending the user to stop
/// the proxy when the real fix is to enrol — or the reverse.
async fn cache_gated(cmd: CacheCmd, unavailable: Option<crate::cache::Unavailable>) -> Result<()> {
    if let Some(why) = unavailable {
        anyhow::bail!("{}", why.message());
    }
    match cmd {
        CacheCmd::Stats { json } => {
            let stats = crate::cache::get_cache_stats();
            if json {
                println!("{}", serde_json::to_string_pretty(&stats)?);
            } else {
                println!("\n  PRISM Semantic Cache Statistics");
                println!("  {}", "═".repeat(40));
                println!("  Total entries: {}", stats.total_entries);
                println!("  Storage path:  {}", stats.sled_path);
            }
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
                for (i, hit) in results.iter().enumerate() {
                    let entry = &hit.entry;
                    println!(
                        "  [{}] {:.0}% match | hash: {} | model: {}",
                        i + 1,
                        hit.score * 100.0,
                        entry.key_hash,
                        entry.model.as_deref().unwrap_or("unknown")
                    );
                    let preview = if entry.response.len() > 150 {
                        &entry.response[..150]
                    } else {
                        &entry.response
                    };
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
    if cmd.show || (cmd.set_key.is_none() && cmd.set_val.is_none()) {
        // The *effective* config: hub-enforced > project `.prismrc` > global > defaults —
        // what a command actually sees, not just what `config.yaml` says.
        let effective = crate::config::resolve();
        println!("\n  PRISM Configuration");
        println!("  {}", "═".repeat(40));
        println!("{}", crate::config::config_to_json(&effective));
        print_hub_feature_state(&effective);
        print_filter_rules();
    } else if let (Some(key), Some(val)) = (cmd.set_key, cmd.set_val) {
        let mut cfg = crate::config::load_global().unwrap_or_default();
        match key.as_str() {
            "compression_ratio" => {
                if let Ok(r) = val.parse::<f64>() {
                    cfg.compression_ratio = Some(r);
                }
            }
            "cache_enabled" => {
                cfg.cache_enabled = Some(val.parse::<bool>().unwrap_or(true));
            }
            "toon_enabled" => {
                cfg.toon_enabled = Some(val.parse::<bool>().unwrap_or(true));
            }
            // Settable because `prism toon tron` reads it. It was accepted by the
            // config file and by PRISM_FILTER-style env, but not here, so the only
            // documented way to turn TRON on did not work.
            "tron_enabled" => {
                cfg.tron_enabled = Some(val.parse::<bool>().unwrap_or(true));
            }
            "graph_enabled" => {
                cfg.graph_enabled = Some(val.parse::<bool>().unwrap_or(true));
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
                println!(
                    "Unknown config key: {}. (Supported: compression_ratio, cache_enabled, toon_enabled, tron_enabled, graph_enabled, proxy_port, mcp_port, tiktoken_model)",
                    key
                );
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
        // TRON had no caller anywhere: the encoder, its own test, and nothing else. It
        // was listed as a differentiator in the comparison docs while being unreachable
        // from any command. Gated on `tron_enabled` (default off) because, unlike TOON,
        // it is a *rendering* — box-drawing output for a human reading a table, with no
        // decoder — so it must never be something a caller reaches by accident.
        ToonCmd::Tron { input } => {
            let cfg = crate::config::resolve();
            if !cfg.tron_enabled() {
                anyhow::bail!(
                    "TRON rendering is off. Enable it with `prism config --set-key tron_enabled --set-val true`.\n\
                     It is a display format with no decoder: TOON is the one to use for data you need to read back."
                );
            }
            let json: serde_json::Value = serde_json::from_str(&input)?;
            println!("{}", crate::encode::tron_encode(&json));
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
        HookCmd::Uninstall => {
            let msg = system.uninstall().map_err(|e| anyhow::anyhow!(e))?;
            println!("{msg}");
        }
        HookCmd::Validate => println!("{}", system.validate()),
        HookCmd::Audit => println!("{}", crate::hooks::hook_audit_stats(&root.join(".prism"))),
    }
    Ok(())
}

// --- hub (PRISM Hub enrollment, telemetry spool, and policy) ---
pub async fn hub(cmd: HubCmd) -> Result<()> {
    match cmd {
        HubCmd::Enroll {
            url,
            token,
            name,
            mcp_host,
            mcp_port,
        } => {
            let mcp = match (mcp_host.as_deref(), mcp_port) {
                (Some(h), Some(p)) => Some((h, p)),
                _ => None,
            };
            let creds = crate::hub::enroll(&url, &token, name.as_deref(), mcp).await?;
            println!("Enrolled with hub: {}", creds.hub_url);
            println!("  Agent ID: {}", creds.agent_id);
            println!(
                "  Name:     {}",
                name.unwrap_or_else(crate::hub::default_agent_name)
            );
            println!("  Credentials written under the global config dir.");
        }
        HubCmd::Status { json } => {
            let status = crate::hub::status();
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!("\n  PRISM Hub Status");
                println!("  {}", "═".repeat(40));
                println!("  Enrolled:      {}", status.enrolled);
                println!(
                    "  Hub URL:       {}",
                    status.hub_url.as_deref().unwrap_or("(none)")
                );
                println!(
                    "  Agent ID:      {}",
                    status.agent_id.as_deref().unwrap_or("(none)")
                );
                println!("  Spool events:  {}", status.spool_events);
                println!(
                    "  Spool bytes:   {} / {} (cap)",
                    status.spool_bytes, status.spool_max_bytes
                );
                if status.dropped_events > 0 {
                    println!("  Dropped events: {}", status.dropped_events);
                }
                if !status.enrolled {
                    println!(
                        "\n  Not enrolled — run `prism hub enroll --url <hub> --token <join-token>`."
                    );
                }
            }
        }
        HubCmd::Flush => {
            let creds = crate::hub::load_credentials()
                .ok_or_else(|| anyhow::anyhow!("not enrolled — run `prism hub enroll` first"))?;
            let outcome = crate::hub::flush(&creds).await;
            println!(
                "Flushed {} event(s) in {} batch(es){}",
                outcome.sent,
                outcome.batches,
                if outcome.failed {
                    " — hub unreachable; spool restored for a later retry"
                } else {
                    ""
                }
            );
        }
        HubCmd::Config => {
            let creds = crate::hub::load_credentials()
                .ok_or_else(|| anyhow::anyhow!("not enrolled — run `prism hub enroll` first"))?;
            crate::hub::fetch_config(&creds).await?;
            println!(
                "Hub policy written to {}",
                crate::prism_data_dir().join("hub-config.yaml").display()
            );
        }
        HubCmd::Test { url } => {
            let targeted = url.is_some();
            crate::hub::send_test_event(url).await?;
            println!(
                "Test event accepted{}.",
                if targeted { "" } else { " by the enrolled hub" }
            );
        }
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
        anyhow::bail!("Usage: prism cmd <cmd> [args...]");
    }

    let cmd = &args[0];

    // When a person is watching, hand the tool through untouched — same stdio, same
    // colours, same interactivity. Filtering exists for the agent reading a pipe, and a
    // captured `git rebase -i` or `docker run -it` is a broken command, not a saving.
    let depth: u32 = std::env::var("PRISM_SHIM_DEPTH")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    let stdout_is_tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
    if !crate::shim::should_filter(crate::shim::mode(), stdout_is_tty, depth) {
        let status = match child_command(cmd, &args[1..]).status() {
            Ok(s) => s,
            Err(e) => anyhow::bail!("prism cmd: failed to run `{}`: {}", cmd, e),
        };
        std::process::exit(status.code().unwrap_or(1));
    }

    // Capture, but leave stdin connected: `echo '{}' | prism cmd jq .` must still work,
    // and `Command::output()` would silently hand the child an empty stdin.
    let cmd_started = std::time::Instant::now();
    let mut c = child_command(cmd, &args[1..]);
    c.stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = match c.spawn().and_then(|ch| ch.wait_with_output()) {
        Ok(o) => o,
        Err(e) => anyhow::bail!("prism cmd: failed to run `{}`: {}", cmd, e),
    };
    // Stops when the child exits. This is the CHILD's wall clock, not PRISM's cost —
    // `prism cmd sleep 3` records 3002 here. It was the only timing PRISM reported,
    // and the hub surfaced it under "Avg / run", which read as PRISM being slow when
    // it was showing how long the user's own docker builds took. `filter_ms` below is
    // the number that actually describes PRISM's overhead.
    let duration_ms = cmd_started.elapsed().as_millis() as u64;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    // Filters always see both streams (cargo/npm/pytest write diagnostics to stderr
    // even on success); stdout first so tabular parsers stay aligned.
    let raw_text = match (stdout.is_empty(), stderr.is_empty()) {
        (true, _) => stderr.clone(),
        (false, true) => stdout.clone(),
        (false, false) => format!("{}\n{}", stdout.trim_end_matches('\n'), stderr),
    };

    // Filter output through the tool-family filters.
    //
    // Timed separately from the child: this, plus the teeing and the write below, is
    // the entire cost of putting PRISM in front of a command. Nothing measured it
    // before, so there was no honest answer to "how much does PRISM add?".
    let filter_started = std::time::Instant::now();
    let filtered = crate::filter::filter_output(&raw_text, cmd, &args[1..]);
    let truncated = crate::filter::has_truncation(&filtered);
    let failed = !output.status.success();

    // Tee raw output whenever the filter dropped something or the command failed, so
    // every `[+N more …]` marker is recoverable from disk. Nothing to recover when the
    // command printed nothing — teeing then would only add noise to the output.
    // Only when something was actually held back: a failing command whose output the
    // filter passed through verbatim has nothing to recover, and the notice would then
    // be the single most expensive line in the output.
    let compacted = filtered.len() + 512 < raw_text.len();
    let tee_path = if !raw_text.trim().is_empty() && (truncated || compacted) {
        tee_raw(cmd, &args[1..], output.status.code(), &stdout, &stderr)
    } else {
        None
    };

    let mut out = filtered.into_owned();
    if !out.ends_with('\n') && !out.is_empty() {
        out.push('\n');
    }
    if truncated {
        if let Some(p) = &tee_path {
            out.push_str(&format!("↳ {}\n", p.display()));
        }
    }
    let _ = std::io::stdout().write_all(out.as_bytes());
    // Everything PRISM did on top of running the command, in microseconds so that a
    // sub-millisecond filter pass does not round to a meaningless 0.
    let filter_us = filter_started.elapsed().as_micros() as u64;

    // Record size, not an exact token count: a PATH shim puts this on the critical path
    // of every command, and loading the cl100k table to fill in a dashboard statistic
    // measured 0.54s per invocation. `prism gain` approximates at report time.
    // Both clocks, not just the byte counts: `duration_ms` is the wrapped command's
    // wall clock and `filter_us` is PRISM's own overhead. Until now only the hub spool
    // carried them, and that is an outbound queue — it early-returns when unenrolled
    // and deletes itself as it drains — so `prism gain` had no way to report either.
    crate::analytics::record_command_timed(cmd, raw_text.len(), out.len(), duration_ms, filter_us)
        .ok();

    // Hub telemetry: append-only, disk-only (see `hub` module docs) — this must never
    // become a network call on this path. `via_shim` is a proxy for "shims are
    // installed and on PATH right now", not proof this exact invocation went through
    // one; there is no cheaper signal available once we are already inside `prism cmd`.
    let _ = crate::hub::spool_event(&crate::hub::HubEvent::Command(crate::hub::CommandEvent {
        tool: cmd.clone(),
        subcommand: args.get(1).cloned().unwrap_or_default(),
        exit_code: output.status.code().unwrap_or(-1),
        duration_ms,
        filter_us,
        input_bytes: raw_text.len(),
        output_bytes: out.len(),
        filtered_bytes: raw_text.len().saturating_sub(out.len()),
        truncated,
        via_shim: crate::shim::status().active,
        ts: crate::hub::now(),
    }));

    // One terse line, and only when it points at something: a bare exit code is
    // already visible to the caller through the process status.
    if failed && !truncated {
        if let Some(p) = &tee_path {
            eprintln!(
                "[prism] exit {}; raw: {}",
                output.status.code().unwrap_or(-1),
                p.display()
            );
        }
    }

    std::process::exit(output.status.code().unwrap_or(if failed { 1 } else { 0 }))
}

/// Build the child process for `prism cmd`.
///
/// The shim directory is dropped from the child's `PATH`, which is what stops
/// `prism cmd git` from finding prism's own `git` shim and recursing forever. Resolution
/// is left to `PATH` rather than baked in at install time, so version managers (nvm,
/// rbenv, pyenv) still choose the binary. `PRISM_SHIM_DEPTH` is an independent guard for
/// any path that sanitising misses.
fn child_command(cmd: &str, rest: &[String]) -> Command {
    let mut c = Command::new(cmd);
    c.args(rest);
    if let Ok(path) = std::env::var("PATH") {
        c.env("PATH", crate::shim::strip_from_path(&path));
    }
    c.env("PRISM_SHIM_DEPTH", "1");
    c
}

/// Write raw stdout/stderr to `<data>/tee/<unix_ts>_<cmd>_<args>.log`, keeping at most
/// `TEE_MAX_FILES` files. Returns the path on success.
fn tee_raw(
    cmd: &str,
    args: &[String],
    code: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> Option<std::path::PathBuf> {
    const TEE_MAX_FILES: usize = 50;
    let tee_dir = crate::prism_data_dir().join("tee");
    std::fs::create_dir_all(&tee_dir).ok()?;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut slug: String = std::iter::once(cmd.to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join("_")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    slug.truncate(60);
    let path = tee_dir.join(format!("{}_{}.log", ts, slug.trim_matches('_')));

    let header = format!(
        "# prism cmd {} {}\n# exit: {}\n",
        cmd,
        args.join(" "),
        code.map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into())
    );
    let body = if stderr.is_empty() {
        format!("{}{}", header, stdout)
    } else {
        format!(
            "{}{}\n# --- stderr ---\n{}",
            header,
            stdout.trim_end_matches('\n'),
            stderr
        )
    };
    std::fs::write(&path, body).ok()?;

    // Rotate: drop oldest beyond the cap
    if let Ok(rd) = std::fs::read_dir(&tee_dir) {
        let mut files: Vec<std::path::PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|e| e == "log").unwrap_or(false))
            .collect();
        if files.len() > TEE_MAX_FILES {
            files.sort();
            for old in &files[..files.len() - TEE_MAX_FILES] {
                let _ = std::fs::remove_file(old);
            }
        }
    }
    Some(path)
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

/// Report the state of `~/.config/prism/filters/*.yaml`.
///
/// A rule file that fails to parse is skipped rather than fatal — one typo must not
/// disable the rules that do load — so the only way a user learns about it is here.
fn print_filter_rules() {
    let rules = crate::filter::loaded_rules();
    let n = rules.tools().len();
    println!();
    println!("  Filter rules ({})", n);
    println!("  {}", "─".repeat(40));
    if n == 0 && rules.errors.is_empty() {
        println!("  none — drop a YAML file in ~/.config/prism/filters/ to add one");
    }
    for tool in rules.tools() {
        println!("  {}", tool);
    }
    for e in &rules.errors {
        eprintln!("  error: {}", e);
    }
}

// --- shim (PATH interception for every client) ---
pub async fn shim(cmd: ShimCmd) -> Result<()> {
    use crate::shim as sh;
    match cmd {
        ShimCmd::Install { path: true } => {
            // hook::install writes the shims and the marked rc block together.
            crate::hook::install(true).await?;
            let st = sh::status();
            println!("  Mode:         {:?} (PRISM_SHIM=auto|always|off)", st.mode);
            println!(
                "  auto filters only when stdout is a pipe, so your own terminal keeps raw output."
            );
        }
        ShimCmd::Install { path: false } => {
            let r = sh::install()?;
            println!("Installed {} shims in {}", r.written.len(), r.dir.display());
            if let Some(vol) = &r.volatile_binary {
                println!();
                println!("  WARNING: these shims point at a build directory:");
                println!("    {}", vol.display());
                println!("  `cargo clean` or moving the repo breaks every shimmed command.");
                println!("  Install to a stable path first:");
                println!(
                    "    cp {} ~/.local/bin/prism && ~/.local/bin/prism shim install --path",
                    vol.display()
                );
            }
            if !r.absent.is_empty() {
                println!("  skipped {} tools not installed here", r.absent.len());
            }
            let st = sh::status();
            if st.active {
                println!("  PATH: active");
            } else {
                println!();
                println!(
                    "  Not yet active — the shims must come first on PATH. Add this to your shell rc:"
                );
                println!("    {}", sh::path_line());
                println!();
                println!(
                    "  GUI-launched editors do not read your shell rc. Set it in the client instead:"
                );
                println!(
                    "    Cursor/VS Code settings.json   \"terminal.integrated.env.linux\": {{\"PATH\": \"{}:${{env:PATH}}\"}}",
                    sh::shim_dir().display()
                );
                println!("    anything else  export PATH before launching it");
                println!();
                // Deliberately not suggested: ~/.claude/settings.json {"env":
                // {"PATH": "<shims>:${PATH}"}}. Claude Code does not expand
                // ${PATH} there, so the agent's shell ends up with the shims
                // directory plus a literal "${PATH}" string and nothing else —
                // every command becomes "command not found". It then survives
                // uninstallation, because the shims directory is deleted while the
                // setting that points at it is not.
                println!("    Claude Code: launch it from a shell that already has the shims on");
                println!("                 PATH. Do not set env.PATH in ~/.claude/settings.json —");
                println!(
                    "                 ${{PATH}} is not expanded there, which leaves the agent"
                );
                println!("                 with no working PATH at all.");
            }
            println!();
            println!("  Mode: {:?} (PRISM_SHIM=auto|always|off).", sh::mode());
            println!(
                "  auto filters only when stdout is a pipe, so your own terminal keeps raw output."
            );
        }
        ShimCmd::Uninstall => {
            // Removal must mirror `shim install --path`, which writes both the shim
            // files and the rc PATH block. This used to delete only the files and then
            // print "Also remove the PATH line … if you added one", leaving every shell
            // with a PATH entry pointing at an empty directory. `hook::uninstall` does
            // both and already existed — it simply had no caller.
            crate::hook::uninstall(true).await?;
            println!();
            println!("  Shims and the PRISM PATH block are gone from your shell rc files.");
            println!("  Open a new shell for it to take effect.");
            println!("  Note: a PATH entry you added to an editor or agent config by hand is not");
            println!("  touched here — `prism uninstall` audits for those.");
        }
        ShimCmd::Status => {
            let st = sh::status();
            println!("\n  PRISM shims");
            println!("  {}", "═".repeat(40));
            println!("  Directory: {}", st.dir.display());
            println!("  Installed: {}", st.installed);
            println!(
                "  On PATH:   {}",
                if st.active {
                    "yes (first — shims win)"
                } else {
                    "no (or not first — shims are inert)"
                }
            );
            println!("  Mode:      {:?}", st.mode);
            if st.installed > 0 && !st.active {
                println!();
                println!("  Add to your shell rc:  {}", sh::path_line());
            }
        }
        ShimCmd::Path => println!("{}", sh::shim_dir().display()),
    }
    Ok(())
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    /// `prism cache stats` must say *disabled*, and it must not fall through to the
    /// printer: the whole failure this gate had to avoid is a paid feature that is off
    /// reporting `Total entries: 0`, which is indistinguishable from a cache that is
    /// simply empty.
    #[tokio::test]
    async fn cache_stats_reports_disabled_rather_than_empty() {
        let err = cache_gated(
            CacheCmd::Stats { json: false },
            Some(crate::cache::Unavailable::Disabled),
        )
        .await
        .expect_err("a disabled cache must fail the command, not print zero entries")
        .to_string();

        assert!(err.contains("disabled"), "{err}");
        assert!(err.contains("prism hub enroll"), "{err}");
        assert!(
            !err.contains("Total entries") && !err.contains("locked"),
            "neither an empty cache nor a lock failure: {err}"
        );
    }

    /// The other reason, still distinct. `--json` too: a script must get a non-zero
    /// exit and a reason, not `{"total_entries": 0}`.
    #[tokio::test]
    async fn cache_stats_still_distinguishes_a_locked_store() {
        let err = cache_gated(
            CacheCmd::Stats { json: true },
            Some(crate::cache::Unavailable::Unopenable(
                "/tmp/x/cache/sled: could not acquire lock",
            )),
        )
        .await
        .expect_err("a locked store must fail the command")
        .to_string();

        assert!(err.contains("could not acquire lock"), "{err}");
        assert!(err.contains("prism serve"), "{err}");
        assert!(
            !err.contains("prism hub enroll"),
            "a lock has nothing to do with enrolment: {err}"
        );
    }

    /// All nine `prism graph` subcommands share one gate, and it fires before the
    /// `--json` arms — which would otherwise print `null` and exit 0.
    #[tokio::test]
    async fn every_graph_subcommand_reports_disabled() {
        for cmd in [
            GraphCmd::Query {
                query: "anything".to_string(),
                graph: None,
                graphify: false,
                json: true,
            },
            GraphCmd::Stats { graph: None },
            GraphCmd::Index {
                path: std::path::PathBuf::from("."),
                from_graphify: None,
            },
        ] {
            let err = graph_gated(cmd, false)
                .await
                .expect_err("a disabled graph command must fail")
                .to_string();
            assert!(err.contains("disabled"), "{err}");
            assert!(err.contains("prism hub enroll"), "{err}");
            assert!(
                !err.contains("prism graph index"),
                "indexing is not the fix when the feature is off: {err}"
            );
        }
    }

    /// The free set is not touched by any of this: no gate, no hub, no policy read.
    #[tokio::test]
    async fn the_free_set_is_unaffected_by_community_defaults() {
        let community = crate::config::PrismConfig::default();
        assert!(community.toon_enabled(), "TOON stays on");
        assert!(!community.tron_enabled(), "TRON stays off, as before");
        assert_eq!(
            community.filters,
            crate::config::FilterLimits::default(),
            "the filter caps that do the actual saving are untouched"
        );

        // `prism read`, `count`, `compress` and `toon` work with nothing configured.
        let compressed = crate::compress::compress("one two three four five six", 0.5);
        assert!(compressed.compressed_tokens > 0);
        assert!(
            crate::encode::encode_json_to_toon(&serde_json::json!([{"a": 1}, {"a": 2}])).is_ok()
        );
        // …and the filters dispatch with no policy read at all.
        let raw = "?? a.txt\n?? b.txt\n";
        let filtered = crate::filter::filter_output(raw, "git", &["status".to_string()]);
        assert!(
            filtered.contains("a.txt"),
            "filters need no policy to dispatch: {filtered}"
        );
    }
}
