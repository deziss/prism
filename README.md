# PRISM — Prompt Reduction, Indexing & Semantic Memory

PRISM is an enterprise-grade AI token optimizer, transparent HTTP/HTTPS MITM proxy, and Model Context Protocol (MCP) server engineered in high-performance Rust.

It intercepts LLM API traffic across your system — from IDEs (Claude Code, Cursor, Windsurf), terminal CLI agents (Aider), Python/Node SDKs, or background scripts — reducing prompt and output token consumption by **50% to 90%** while maintaining zero data loss and 100% prompt cache stability.

```
Without PRISM: App / Agent ────────────────────────────────► api.openai.com  (2,000 tokens, $0.010)
With PRISM:    App / Agent → PRISM Proxy → Compress/Cache ──► api.openai.com    (600 tokens, $0.003)
                                    │
               ┌────────────────────┴────────────────────┐
               ▼                                         ▼
   TurboVec Semantic Cache                  Analytics & Token Savings
   (<1ms ANN vector hits)                   (Local JSONL + Dashboard)
```

---

## Key Highlights

- **AST Smart Code Reader (`prism read`)**: Parses source code into AST skeletons, function signatures, and imports across Rust, Python, TypeScript/JavaScript, and Go — slashing context window consumption by **55% to 93%**.
- **Transparent MITM Proxy (`:27181`)**: Transparent HTTP `CONNECT` tunnel generating per-domain certificates via local root CA. Automatically handles streaming SSE responses chunk-by-chunk with zero latency overhead.
- **Prefix-Preserving & Invariant-Compliant Prompt Caching**: Strictly preserves system prompts and conversation prefixes while dynamically enforcing Anthropic cache ordering invariants (auto-promotes preceding breakpoints to `1h` when later blocks use `1h` to prevent HTTP 400 errors, strictly enforces Anthropic's 4-breakpoint limit, and honors caller-defined caching strategies). Guarantees **90% Anthropic prompt cache discounts** and **50% OpenAI discounts**.
- **Anthropic Context Pruning**: Opts long agent runs into server-side `clear_tool_uses` context pruning, preventing stale tool results from accumulating across long agent interactions.
- **TurboVec Quantized Semantic Cache (`prism cache`)**: 16-dimensional SIMD quantized vector embeddings enabling sub-millisecond local ANN semantic response retrieval.
- **GraphRAG & Graphify Integration (`prism graph`)**: Ingests and queries codebase dependency graphs (`graphify-out/graph.json` or custom graphs) for architectural explanations, shortest path tracing, and god-node detection.
- **Failure Tee Mechanism (`prism cmd`)**: Strips terminal noise from build and test commands while automatically preserving raw, unstripped stdout/stderr in `~/.local/share/prism/tee/` whenever a process exits non-zero.
- **XDG Base Directory Compliance**: Clean multi-user POSIX isolation adhering to FreeDesktop.org standards.

---

## Quick Installation

### Option 1: Automated Script with Interactive User Guide (Recommended)

```bash
git clone https://github.com/PRISM-Team/prism.git
cd prism
./scripts/install.sh --guide
```
*The installer compiles the release binary with Link-Time Optimization (LTO), initializes local XDG directories, generates the MITM CA certificate, sets up helper aliases, and launches the interactive User Guide.*

### Option 2: Cargo Install

```bash
cargo build --release
cp target/release/prism ~/.local/bin/prism

# Initialize XDG directories & root CA certificate
prism init --global --guide
```

---

## 1-Command System Interception

PRISM includes zero-friction system toggle scripts for managing background execution:

```bash
# Enable PRISM as the system default interceptor
prism-enable     # (or alias: prism-on)

# Disable PRISM and revert to direct internet
prism-disable    # (or alias: prism-off)
```

### What `prism-enable` Configures:
1. **Systemd User Daemons**:
   - `prism-proxy.service`: Transparent MITM Proxy active on `http://127.0.0.1:27181`
   - `prism-mcp.service`: Model Context Protocol server active on `http://127.0.0.1:27182`
2. **Environment Injection (`~/.config/environment.d/10-prism.conf` & `~/.bashrc`)**:
   - Sets `HTTP_PROXY` and `HTTPS_PROXY` to `http://127.0.0.1:27181`
   - Sets `NO_PROXY=localhost,127.0.0.1,::1`
   - Injects PRISM root CA into Node.js (`NODE_EXTRA_CA_CERTS`), Python (`REQUESTS_CA_BUNDLE`), and Curl (`SSL_CERT_FILE`)
3. **CLI Aliases**:
   - `p` $\to$ `prism`
   - `pread` $\to$ `prism read`
   - `prtk` $\to$ `prism cmd`
   - `pgain` $\to$ `prism gain`
   - `pgraph` $\to$ `prism graph`

---

## Interactive User Guide

PRISM includes a built-in terminal guide with clean, tabular typography:

```bash
prism guide              # Display guide table of contents and menu
prism guide quickstart   # Fast 2-minute setup & toggles
prism guide architecture # Hexagonal Ports & Adapters system design
prism guide storage      # Complete XDG storage map & config hierarchy
prism guide agents       # Setup for Claude Code, Cursor, Windsurf, Aider
prism guide proxy        # MITM proxy mechanics, TLS CA, prompt caching
prism guide commands     # AST reader modes, graph queries, cache lookup
prism guide troubleshoot # Port resolution (:27181), SSL trust & failure tees
prism guide all          # Comprehensive documentation start-to-finish
```

---

## File & Configuration Storage (XDG Standard)

PRISM strictly complies with the **FreeDesktop.org XDG Base Directory Specification**:

| Storage Area | Environment Variable | Default Path on Linux | Purpose |
| :--- | :--- | :--- | :--- |
| **Global Config** | `$XDG_CONFIG_HOME/prism/` | `~/.config/prism/config.yaml` | User preferences, enabled engines, port mappings |
| **Project Config** | `<project_root>/` | `.prismrc` or `.prism/config.yaml` | Local repository overrides (precedes global config) |
| **Persistent Data** | `$XDG_DATA_HOME/prism/` | `~/.local/share/prism/` | Persistent databases and stateful assets: |
| ↳ *CA Certificates* | `$XDG_DATA_HOME/prism/ca/` | `~/.local/share/prism/ca/` | Root CA private key (`ca.key`, mode 0600) and cert (`ca.crt`) |
| ↳ *Semantic Cache* | `$XDG_DATA_HOME/prism/cache/` | `~/.local/share/prism/cache/` | TurboVec quantized SIMD vector embeddings cache |
| ↳ *Knowledge Graph*| `$XDG_DATA_HOME/prism/graph/` | `~/.local/share/prism/graph/` | GraphRAG Petgraph serialized node relations |
| ↳ *Memory Palace* | `$XDG_DATA_HOME/prism/memory/`| `~/.local/share/prism/memory/` | Sled database for 7-tier associative memory |
| ↳ *Analytics* | `$XDG_DATA_HOME/prism/analytics/`| `~/.local/share/prism/analytics/` | Token logs and `proxy_events.jsonl` |
| **State / Tees** | `$XDG_STATE_HOME/prism/` | `~/.local/share/prism/tee/` | Raw crash and failure logs from `prism cmd` |
| **Runtime Daemons** | Systemd User Units | `~/.config/systemd/user/` | `prism-proxy.service` (:27181) and `prism-mcp.service` (:27182) |

#### Configuration Resolution Hierarchy (12-Factor App):
```
[1] CLI Flags          (--port 27181, --guide)
      ↓
[2] Environment Vars   (HTTP_PROXY, PRISM_NO_CONTEXT_EDITING)
      ↓
[3] Project Config     (<workspace>/.prismrc)
      ↓
[4] Global Config      (~/.config/prism/config.yaml)
      ↓
[5] Defaults           (PrismConfig::default())
```

---

## AI Agent & IDE Integration

### 1. Claude Code (Anthropic)
Register PRISM as an HTTP MCP server:
```bash
claude mcp add prism --transport http http://localhost:27182
```
Or manually configure in `~/.claude.json`:
```json
{
  "mcpServers": {
    "prism": {
      "type": "http",
      "url": "http://localhost:27182"
    }
  }
}
```

### 2. Cursor IDE & Windsurf
Add PRISM MCP endpoint in **Cursor Settings $\to$ Features $\to$ MCP**:
- **Name**: `prism`
- **Type**: `HTTP / SSE`
- **URL**: `http://localhost:27182`

Generate native VS Code extension scaffold:
```bash
prism vscode --output ~/.vscode/extensions/prism
```

### 3. Python & Node.js AI SDKs (OpenAI, LangChain, LlamaIndex)
Zero code changes required. Route traffic through environment variables:
```bash
export HTTP_PROXY=http://127.0.0.1:27181
export HTTPS_PROXY=http://127.0.0.1:27181
export REQUESTS_CA_BUNDLE=~/.local/share/prism/ca/ca.crt
export NODE_EXTRA_CA_CERTS=~/.local/share/prism/ca/ca.crt
```

---

## CLI Command Reference

### AST Code Reader (`prism read`)
```bash
prism read src/lib.rs --mode signatures    # Function signatures, structs, traits (72% token savings)
prism read src/main.rs --mode skeleton      # Outline of symbols without bodies (77% token savings)
prism read src/proxy.rs --mode imports      # External dependencies & imports (93% token savings)
prism read src/cli.rs --lines 50-120 -n     # Line range slice with line numbers (68% token savings)
```

### Filtered Command Runner & Failure Tee (`prism cmd`)
```bash
prism cmd cargo test                       # Strip noisy compiler ANSI progress, track token spend
prism cmd git status                       # On failure, dumps raw stderr to ~/.local/share/prism/tee/
```

### GraphRAG & Codebase Intelligence (`prism graph`)
```bash
prism graph query "how does proxy work?"   # Query auto-detected Graphify knowledge graph
prism graph explain proxy_server           # Explain specific node and connected edges
prism graph path cli proxy                 # Find shortest dependency path between symbols
prism graph god-nodes --top 5              # Identify architectural bottleneck nodes
```

### Semantic Cache & Analytics
```bash
prism cache query "prompt query"           # Sub-millisecond ANN vector lookup
prism cache stats                          # View cache hit rate, size, and entries
prism gain                                 # Real-time token and dollar savings dashboard
prism gain --history                       # View detailed historical command log
```

---

## System Architecture

```
Clients / IDEs / Agents (Claude Code, Cursor, Windsurf, Aider)
         │
         ├── HTTP CONNECT tunnel (:27181) ──────► PRISM Proxy
         │                                         ├── Per-domain TLS cert (signed by PRISM CA)
         │                                         ├── Detect AI provider by hostname
         │                                         ├── Prefix-Preserving prompt cache protection
         │                                         ├── Anthropic cache TTL order normalization & 4-breakpoint cap
         │                                         ├── BM25 tail-message compression (code-safe)
         │                                         ├── Anthropic clear_tool_uses beta pruning
         │                                         └── Non-AI hosts ──► Raw passthrough
         │                                                   │
         │                                                   ▼
         │                                      api.openai.com / api.anthropic.com
         │
         ├── JSON-RPC 2.0 (:27182) ─────────────► PRISM MCP Server
         │                                         ├── prism_read (AST Smart Reader)
         │                                         ├── prism_graph_query (GraphRAG)
         │                                         ├── prism_memory_search (7-tier Sled KV)
         │                                         └── prism_compress (BM25 Compressor)
         │
         └── CLI Terminal ─────────────────────► PRISM Command Gateway
                                                   ├── AST Reader (`prism read`)
                                                   ├── Noise Filter (`prism cmd`)
                                                   └── Failure Tee (`~/.local/share/prism/tee/`)
```

---

## License

GNU Affero General Public License v3.0 (AGPL-3.0). Copyright (c) 2026 PRISM Team. See [LICENSE](LICENSE) for details.
