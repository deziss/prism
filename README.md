# PRISM

**Personal Reasoning & Intelligence System for Models**

Enterprise-grade token optimizer CLI for AI coding tools. Single Rust binary, zero runtime dependencies, 60–90% token reduction on common dev operations.

---

## Features

| Category | Capability |
|----------|-----------|
| **RTK Filters** | 30+ command-specific output filters (git, cargo, pytest, kubectl, docker, gh, tsc, jest, vitest, …) |
| **Proxy** | HTTP reverse proxy with TOON/TRON encoding, semantic cache, response compression |
| **Memory Palace** | 3-layer hierarchical persistence (Recall → Core → Archive) backed by JSONL + sled |
| **GraphRAG** | Cross-file codebase dependency analysis, Obsidian export |
| **CRAG** | Corrective Retrieval-Augmented Generation — auto re-queries below relevance threshold |
| **TurboVec** | Google TurboQuant ANN index (10M docs / 4GB, AVX-512BW SIMD, 4× compression) |
| **MCP Server** | 5-tool Model Context Protocol server over HTTP (axum) |
| **Token Counter** | tiktoken-rs `cl100k_base` — accurate token counts per model |
| **Analytics** | Gain dashboard, command history, session discovery |
| **VS Code Extension** | 5 commands, proxy port config, integrated terminal launch |

---

## Quick Start

```bash
# Build
cargo build --release

# Install
cp target/release/prism ~/.local/bin/

# Initialize (writes CLAUDE.md hook instructions)
prism init --global

# Token savings dashboard
prism gain

# Run any command through PRISM filters
prism git status
prism cargo test
prism pytest

# Count tokens in text or file
prism count --string "Hello world"
prism count --file src/main.rs --model gpt-4
```

---

## Subcommands

```
prism init [--global]              Write RTK hook instructions to CLAUDE.md
prism gain [--history]             Token savings dashboard / command history
prism discover                     Analyze Claude Code sessions for missed savings
prism proxy <cmd...>               Run command without filters (debug)
prism serve [--port 8080]          Start HTTP proxy (upstream: OpenAI-compatible API)
  [--upstream http://...]
prism mcp [--port 9090]            Start MCP server (5 tools)
prism memory search <query>        Search Memory Palace
prism memory save <key> <value>    Save to Memory Palace
prism memory list                  List all memory blocks
prism memory stats                 Memory layer statistics
prism memory compact               Evict oldest Recall blocks
prism graph query <query>          Query knowledge graph
prism graph extract <url|file>     Extract knowledge from source
prism graph export [--output dir]  Export to Obsidian vault
prism graph stats                  Graph statistics
prism toon encode <json>           Encode JSON to TOON tabular format
prism toon decode <toon>           Decode TOON back to JSON
prism count [--string|-f <file>]   Count tokens
  [--model cl100k_base]
prism <any command>                Pass-through with RTK filtering
```

---

## Command Filters (RTK-compatible)

```bash
prism git status / diff / log / branch / add / commit / push / pull
prism cargo build / test / check / clippy / doc
prism pytest / jest / vitest / rspec / rake test / go test
prism tsc / eslint / lint / prettier --check
prism docker ps / images / logs
prism kubectl get / logs
prism gh pr view / pr checks / run list / issue list
prism pnpm / npm / npx
prism grep / find / ls
prism aws / psql / dotnet / mypy / ruff / rubocop / pip
prism next build / prisma migrate
```

---

## Architecture

```
prism/
├── src/
│   ├── main.rs          — CLI entry point (clap, 14 subcommands)
│   ├── lib.rs           — Module exports + prism_data_dir()
│   ├── filter.rs        — 30+ RTK-compatible output filters
│   ├── cli.rs           — Subcommand handlers
│   ├── proxy.rs         — axum HTTP proxy + TOON encoding
│   ├── mcp.rs           — MCP server (5 tools)
│   ├── cache.rs         — SemanticCache: sled + TurboVec ANN
│   ├── vector.rs        — TurboVecIndex (IdMapIndex wrapper, dim=16)
│   ├── memory.rs        — MemoryPalace 3-layer + async API
│   ├── analytics.rs     — tiktoken-rs token counting + gain dashboard
│   ├── encode.rs        — TOON/TRON encoding/decoding
│   ├── compress.rs      — Output compression
│   ├── hook.rs          — Shell hook install/uninstall
│   ├── config.rs        — Configuration management
│   ├── utils.rs         — Shared utilities
│   ├── vscode.rs        — VS Code extension types
│   └── knowledge/
│       ├── mod.rs       — GraphRAG API + async wrappers
│       ├── graph_rag.rs — Dependency graph analysis
│       └── crag.rs      — Corrective RAG (re-query below threshold)
├── extensions/vscode/   — VS Code extension
├── build.rs             — Portable BLAS detection for turbovec
├── .cargo/config.toml   — Build config
└── .blas-link/          — Machine-local BLAS symlinks (gitignored)
```

---

## Data Directory Layout

```
~/.local/share/prism/
├── sessions/
│   ├── recall/blocks.jsonl    # Short-term memory
│   ├── core/blocks.jsonl      # Mid-term memory
│   └── archive/blocks.jsonl   # Long-term memory
├── cache/
│   └── sled/                  # TurboVec + sled semantic cache
├── graph/
│   ├── entities.jsonl
│   └── relationships.jsonl
├── analytics/
│   └── commands.jsonl         # Command history for gain dashboard
└── hooks/                     # Shell hook scripts
```

---

## MCP Tools

The `prism mcp` server exposes these tools to Claude / other MCP clients:

| Tool | Description |
|------|-------------|
| `prism_memory_search` | Search Memory Palace by query string |
| `prism_memory_save` | Save key-value pair to Memory Palace |
| `prism_graph_query` | Query knowledge graph |
| `prism_toon_encode` | Encode JSON to TOON tabular format |
| `prism_count_tokens` | Count tokens with tiktoken-rs |

Connect from Claude Code:
```json
// .claude/settings.json
{
  "mcpServers": {
    "prism": {
      "type": "http",
      "url": "http://localhost:9090"
    }
  }
}
```

---

## System Requirements

| Requirement | Minimum |
|-------------|---------|
| Rust | 1.75+ (2021 edition) |
| OS | Linux x86_64, macOS (Apple Silicon or Intel) |
| BLAS | `libopenblas-dev` or `libgsl-dev` (for turbovec) |
| OpenSSL | `libssl-dev` (for reqwest TLS) |

### Install system dependencies

**Ubuntu / Debian:**
```bash
sudo apt-get install libopenblas-dev libssl-dev pkg-config
```

**Fedora / RHEL:**
```bash
sudo dnf install openblas-devel openssl-devel pkgconfig
```

**macOS:**
```bash
brew install openblas openssl
```

> See [TROUBLESHOOT.md](TROUBLESHOOT.md) if `cargo build` fails with BLAS or OpenSSL errors.

---

## Key Dependencies

| Crate | Purpose |
|-------|---------|
| `turbovec = "0.2"` | TurboQuant ANN index (Google Research, low-memory) |
| `tiktoken-rs = "0.6"` | OpenAI-compatible token counting |
| `axum = "0.8"` | HTTP server for proxy + MCP |
| `sled = "0.34"` | Embedded key-value store for semantic cache |
| `clap = "4.5"` | CLI argument parsing |
| `colored = "2.1"` | Colored terminal output |
| `tokio = "1.40"` | Async runtime |
| `petgraph = "0.7"` | Knowledge graph data structure |
| `tiktoken-rs = "0.6"` | `cl100k_base` BPE tokenizer |

---

## prism-hub

`prism-hub` is a companion NestJS/PostgreSQL service for team deployments.
It stores session metadata, team/project configs, user settings, and analytics aggregates.
Local PRISM data (memory JSONL, TurboVec index, knowledge graph) stays on each machine.

See `/home/anshukushwaha/Desktop/learn/prism-hub/` for the hub backend + frontend.

**Auth:** JWT + argon2 password hashing  
**DB:** PostgreSQL via Prisma ORM  
**API:** NestJS with Swagger docs at `/api/docs`

---

## License

UNLICENSED — private project.
