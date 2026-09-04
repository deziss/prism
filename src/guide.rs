//! PRISM Interactive User Guide & Architectural Reference
//!
//! Provides comprehensive documentation on system design, file storage,
//! agent integration, proxy operation, and developer workflows.

use colored::Colorize;

pub fn show_guide(topic: Option<&str>) {
    let t = topic.unwrap_or("menu").trim().to_lowercase();
    match t.as_str() {
        "menu" => print_menu(),
        "1" | "quickstart" | "start" => print_quickstart(),
        "2" | "arch" | "architecture" | "system-design" => print_architecture(),
        "3" | "storage" | "config" | "xdg" | "files" => print_storage_layout(),
        "4" | "agents" | "mcp" | "claude" | "cursor" => print_agent_integration(),
        "5" | "proxy" | "mitm" | "network" => print_proxy_mechanics(),
        "6" | "commands" | "cli" | "tools" => print_cli_reference(),
        "7" | "troubleshoot" | "faq" | "debug" => print_troubleshooting(),
        "8" | "all" | "full" => {
            print_quickstart();
            print_architecture();
            print_storage_layout();
            print_agent_integration();
            print_proxy_mechanics();
            print_cli_reference();
            print_troubleshooting();
        }
        _ => {
            println!("\n  {}", format!("Unknown topic '{}'.", t).red().bold());
            println!("  Available topics:\n");
            println!("    {:<16} {}", "quickstart".green().bold(), "Quick setup & 1-toggle controls");
            println!("    {:<16} {}", "architecture".green().bold(), "System design & Hexagonal Ports/Adapters");
            println!("    {:<16} {}", "storage".green().bold(), "XDG specs, config files, cache & databases");
            println!("    {:<16} {}", "agents".green().bold(), "Claude Code, Cursor, Windsurf, Aider, SDKs");
            println!("    {:<16} {}", "proxy".green().bold(), "Transparent MITM Proxy, TLS CA, prompt caching");
            println!("    {:<16} {}", "commands".green().bold(), "Full CLI reference: AST reader, filter, graph");
            println!("    {:<16} {}", "troubleshoot".green().bold(), "Port conflicts, certificates, failure tee logs");
            println!("    {:<16} {}", "all".green().bold(), "View entire guide sequentially\n");
            println!("  Usage: {} <topic>\n", "prism guide".cyan());
        }
    }
}

fn print_header(title: &str, category: &str) {
    println!("\n  {}  {}  {}", "PRISM GUIDE".bold().cyan(), "›".dimmed(), category.to_uppercase().yellow().bold());
    println!("  {}", title.bold().white());
    println!("  {}\n", "─".repeat(70).dimmed());
}

fn print_menu() {
    println!("\n  {}  {}", "PRISM".bold().cyan(), "v0.1.0".dimmed());
    println!("  {}", "Prompt Reduction, Indexing & Semantic Memory".bold().white());
    println!("  {}", "─".repeat(70).dimmed());
    println!("  PRISM is an enterprise-grade AI token optimizer, transparent MITM proxy,");
    println!("  and Model Context Protocol (MCP) server engineered in high-performance Rust.\n");

    println!("  {}", "AVAILABLE TOPICS:".bold().yellow());
    println!("    {} {:<16} {}", "[1]".cyan(), "quickstart".green().bold(), "Get up and running in under 2 minutes");
    println!("    {} {:<16} {}", "[2]".cyan(), "architecture".green().bold(), "System design & Hexagonal Ports/Adapters");
    println!("    {} {:<16} {}", "[3]".cyan(), "storage".green().bold(), "POSIX & XDG storage layout, config hierarchy");
    println!("    {} {:<16} {}", "[4]".cyan(), "agents".green().bold(), "Claude Code, Cursor, Windsurf, Aider, LangChain");
    println!("    {} {:<16} {}", "[5]".cyan(), "proxy".green().bold(), "Transparent MITM Proxy, TLS CA, prefix caching");
    println!("    {} {:<16} {}", "[6]".cyan(), "commands".green().bold(), "Full CLI command suite (AST reader, graph, cache)");
    println!("    {} {:<16} {}", "[7]".cyan(), "troubleshoot".green().bold(), "Port resolution, SSL trust, failure tee recovery");
    println!("    {} {:<16} {}", "[8]".cyan(), "all".green().bold(), "Read the comprehensive documentation start-to-finish");
    println!("");
    println!("  {}", "USAGE:".bold().yellow());
    println!("    $ {} <topic>      {} Run `prism guide quickstart`", "prism guide".cyan().bold(), "#".dimmed());
    println!("    $ {} all            {} View full documentation\n", "prism guide".cyan().bold(), "#".dimmed());
}

fn print_quickstart() {
    print_header("Getting Started & 1-Command Controls", "Quickstart");
    println!("  PRISM operates as a transparent system-wide interceptor or on-demand service.\n");

    println!("  {}", "1. Enable Laptop-Wide Default Interception:".bold().white());
    println!("     $ {}", "prism-enable".cyan().bold());
    println!("     • Starts systemd user daemons for Proxy (:8081) and MCP (:3003)");
    println!("     • Injects proxy settings into environment.d and ~/.bashrc");
    println!("     • Mounts root CA certificate into Node.js, Python, and Curl trust stores\n");

    println!("  {}", "2. Disable / Revert to Direct Internet:".bold().white());
    println!("     $ {}", "prism-disable".cyan().bold());
    println!("     • Immediately stops background daemons");
    println!("     • Strips proxy environment variables with zero residual state\n");

    println!("  {}", "3. Standalone Foreground Execution:".bold().white());
    println!("     $ {:<30} {}", "prism serve --port 8081".cyan(), "# Run transparent MITM proxy".dimmed());
    println!("     $ {:<30} {}", "prism mcp   --port 3003".cyan(), "# Run JSON-RPC 2.0 MCP server".dimmed());
    println!("");
    println!("  {}", "4. Measured Token & Cost Savings:".bold().white());
    println!("     $ {}", "prism gain --history".cyan().bold());
}

fn print_architecture() {
    print_header("System Design & Hexagonal Ports/Adapters", "Architecture");
    println!("  PRISM implements a clean decoupled Ports & Adapters architecture:\n");

    println!("  {}", "1. INBOUND PORTS (Gateways & Interfaces)".bold().cyan());
    println!("     • {:<26} {}", "MITM Proxy (:8081)", "Transparent HTTP/HTTPS CONNECT interceptor");
    println!("     • {:<26} {}", "MCP Server (:3003)", "Model Context Protocol JSON-RPC 2.0 interface");
    println!("     • {:<26} {}", "CLI Runner (`prism cmd`)", "Noise-filtering process executor with failure tee");
    println!("     • {:<26} {}", "AST Reader (`prism read`)", "7-mode AST semantic file outline & reader");
    println!("     • {:<26} {}", "Lifecycle Hooks", "Repository file pre-read & post-write validation");
    println!("");

    println!("  {}", "2. CORE DOMAIN ENGINES (Pure Logic)".bold().yellow());
    println!("     • {:<26} {}", "TurboVec Engine", "16-dim quantized SIMD semantic cache (<1ms lookup)");
    println!("     • {:<26} {}", "Cache Preserver", "Prefix-preserving prompt caching (90% Anthropic discount)");
    println!("     • {:<26} {}", "BM25 Compressor", "Tail message prose compressor (code blocks untouched)");
    println!("     • {:<26} {}", "TOON Serializer", "Token-Optimized Object Notation (30-60% JSON reduction)");
    println!("     • {:<26} {}", "GraphRAG Pipeline", "Petgraph dependency traversal & native Graphify loader");
    println!("");

    println!("  {}", "3. OUTBOUND ADAPTERS (Persistence & Egress)".bold().magenta());
    println!("     • {:<26} {}", "AI Upstreams", "OpenAI, Anthropic, Gemini, Groq, Ollama");
    println!("     • {:<26} {}", "Sled KV Database", "Embedded ACID storage for 7-tier associative memory");
    println!("     • {:<26} {}", "XDG Storage Standard", "Strict POSIX/XDG compliant config and data isolation");
    println!("     • {:<26} {}", "Failure Tee Preserver", "Captures raw diagnostic logs on non-zero exit codes");
    println!("     • {:<26} {}", "PRISM Hub Egress", "Real-time telemetry streaming to local analytics hub");
    println!("");

    println!("  {}", "KEY SYSTEM GUARANTEES:".bold().white());
    println!("    • {} Upstream failures drop to raw fallback without lost requests", "Fault Isolation:".green().bold());
    println!("    • {} Zero memory leaks, bounded cache, pure asynchronous I/O", "Zero Overhead:".green().bold());
    println!("    • {} System prompts and code blocks are never modified or corrupted", "Semantic Safety:".green().bold());
}

fn print_storage_layout() {
    print_header("POSIX & XDG Base Directory Storage Standard", "Storage");
    println!("  PRISM strictly follows the XDG specification to eliminate home-directory clutter:\n");

    println!("  {}", "1. Configuration ($XDG_CONFIG_HOME/prism/ -> ~/.config/prism/):".bold().white());
    println!("     • {:<16} Global YAML configuration, port bindings, enabled engines", "config.yaml".cyan());
    println!("     • Resolution: CLI Arguments > Env Vars > .prismrc > config.yaml > Defaults\n");

    println!("  {}", "2. Persistent Data ($XDG_DATA_HOME/prism/ -> ~/.local/share/prism/):".bold().white());
    println!("     • {:<16} Root CA private key (mode 0600) and certificate for MITM", "ca/".cyan());
    println!("     • {:<16} Sub-millisecond SIMD quantized vector embeddings cache", "cache/".cyan());
    println!("     • {:<16} GraphRAG serialized Petgraph nodes and edge relations", "graph/".cyan());
    println!("     • {:<16} Sled embedded database for 7-tier memory palace", "memory/".cyan());
    println!("     • {:<16} Audit records and proxy_events.jsonl", "analytics/".cyan());
    println!("");

    println!("  {}", "3. State & Failure Logs ($XDG_STATE_HOME/prism/ -> ~/.local/share/prism/):".bold().white());
    println!("     • {:<16} Preserves raw stdout/stderr dumps whenever commands fail", "tee/".cyan());
    println!("");

    println!("  {}", "4. Project-Local Overrides (<project_root>/):".bold().white());
    println!("     • {:<16} Local project configuration (overrides global settings)", ".prismrc".cyan());
    println!("     • {:<16} Repository lifecycle hooks and repeated-read tracking", ".prism/".cyan());
}

fn print_agent_integration() {
    print_header("AI Coding Agent & SDK Integration", "Agents");

    println!("  {}", "1. Claude Code (Anthropic):".bold().white());
    println!("     Register PRISM as an MCP server:");
    println!("     $ {}\n", "claude mcp add prism --transport http http://localhost:3003".cyan());
    println!("     Or configure directly in `~/.claude.json`:");
    println!("     {}", "{\n       \"mcpServers\": {\n         \"prism\": { \"type\": \"http\", \"url\": \"http://localhost:3003\" }\n       }\n     }".dimmed());
    println!("");

    println!("  {}", "2. Cursor IDE & Windsurf:".bold().white());
    println!("     Add PRISM endpoint in Cursor Settings -> Features -> MCP:");
    println!("     • Name:  prism");
    println!("     • Type:  HTTP / SSE");
    println!("     • URL:   http://localhost:3003\n");
    println!("     Generate native VS Code extension scaffold:");
    println!("     $ {}\n", "prism vscode --output ~/.vscode/extensions/prism".cyan());

    println!("  {}", "3. Python & Node.js AI SDKs (OpenAI, LangChain, LlamaIndex):".bold().white());
    println!("     Zero code changes needed. Route traffic through environment variables:");
    println!("       export HTTP_PROXY=http://127.0.0.1:8081");
    println!("       export HTTPS_PROXY=http://127.0.0.1:8081");
    println!("       export REQUESTS_CA_BUNDLE=~/.local/share/prism/ca/ca.crt");
    println!("       export NODE_EXTRA_CA_CERTS=~/.local/share/prism/ca/ca.crt");
}

fn print_proxy_mechanics() {
    print_header("Transparent MITM Proxy & Prompt Caching", "Proxy");

    println!("  {}", "How the Transparent Proxy Operates:".bold().white());
    println!("    1. Clients establish standard HTTP CONNECT tunnels to port 8081.");
    println!("    2. PRISM dynamically generates TLS certs on-the-fly signed by the PRISM CA.");
    println!("    3. Requests are matched against AI endpoints (OpenAI, Anthropic, Gemini).");
    println!("    4. Non-AI hosts pass through raw with zero overhead or interception.\n");

    println!("  {}", "Prefix-Preserving Optimization Rules:".bold().white());
    println!("    • {:<20} System prompts & conversation history remain byte-identical.", "Rule 1 (Stability)".green().bold());
    println!("                           Guarantees 90% Anthropic and 50% OpenAI prompt cache discounts.");
    println!("    • {:<20} Fenced code blocks, backticks, and diff hunks are untouched.", "Rule 2 (Code Safety)".green().bold());
    println!("    • {:<20} Prose lines in the latest 2 messages are BM25 compressed.", "Rule 3 (Compression)".green().bold());
    println!("    • {:<20} Long agent traces receive `clear_tool_uses` context pruning.", "Rule 4 (Agent Beta)".green().bold());
}

fn print_cli_reference() {
    print_header("Developer CLI Command Suite", "Commands");

    println!("  {}", "AST Code Reader (Saves 55% - 93% Tokens):".bold().yellow());
    println!("    $ {:<44} {}", "prism read src/lib.rs --mode signatures".cyan(), "# Extract signatures & types".dimmed());
    println!("    $ {:<44} {}", "prism read src/main.rs --mode skeleton".cyan(), "# Outline symbols without bodies".dimmed());
    println!("    $ {:<44} {}", "prism read src/proxy.rs --mode imports".cyan(), "# Module imports only".dimmed());
    println!("    $ {:<44} {}", "prism read src/cli.rs --lines 50-120 -n".cyan(), "# Line slice with line numbers".dimmed());
    println!("");

    println!("  {}", "Filtered Command Runner & Failure Tee:".bold().yellow());
    println!("    $ {:<44} {}", "prism cmd cargo test".cyan(), "# Filter noise, track tokens".dimmed());
    println!("    $ {:<44} {}", "prism cmd git status".cyan(), "# Saves raw logs to tee/ on error".dimmed());
    println!("");

    println!("  {}", "GraphRAG & Knowledge Queries:".bold().yellow());
    println!("    $ {:<44} {}", "prism graph query \"how does caching work?\"".cyan(), "# Query codebase knowledge graph".dimmed());
    println!("    $ {:<44} {}", "prism graph explain proxy_server".cyan(), "# Detailed node explanation".dimmed());
    println!("    $ {:<44} {}", "prism graph path cli proxy".cyan(), "# Shortest dependency path".dimmed());
    println!("    $ {:<44} {}", "prism graph god-nodes --top 5".cyan(), "# Find architectural god-nodes".dimmed());
    println!("");

    println!("  {}", "Semantic Cache & Analytics:".bold().yellow());
    println!("    $ {:<44} {}", "prism cache query \"prompt\"".cyan(), "# Sub-millisecond ANN lookup".dimmed());
    println!("    $ {:<44} {}", "prism cache stats".cyan(), "# Cache hit rate and entry count".dimmed());
    println!("    $ {:<44} {}", "prism gain --history".cyan(), "# Cumulative token & dollar savings".dimmed());
}

fn print_troubleshooting() {
    print_header("Troubleshooting & Diagnostics", "Troubleshoot");

    println!("  {}", "1. Port Conflicts (Port 8080 In Use):".bold().white());
    println!("     • If port 8080 is used by Apache, Nginx, or PHP Spark, PRISM uses port {}\n", "8081".green().bold());
    println!("     • Start manually on any custom port: `prism serve --port 8085`\n");

    println!("  {}", "2. SSL/TLS Certificate Warnings:".bold().white());
    println!("     • Python:   export REQUESTS_CA_BUNDLE=~/.local/share/prism/ca/ca.crt");
    println!("     • Node.js:  export NODE_EXTRA_CA_CERTS=~/.local/share/prism/ca/ca.crt");
    println!("     • System:   sudo cp ~/.local/share/prism/ca/ca.crt /usr/local/share/ca-certificates/ && sudo update-ca-certificates\n");

    println!("  {}", "3. Command Diagnostic Logs on Non-Zero Exit:".bold().white());
    println!("     • PRISM automatically dumps unstripped stdout/stderr to:");
    println!("       `~/.local/share/prism/tee/<cmd>_<timestamp>.log`\n");

    println!("  {}", "4. Checking Background Daemons:".bold().white());
    println!("     • Service status: `systemctl --user status prism-proxy prism-mcp`");
    println!("     • Live logs:      `journalctl --user -u prism-proxy -f`");
}
