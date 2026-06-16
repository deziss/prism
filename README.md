# PRISM — Personal Reasoning & Intelligence System for Models

PRISM intercepts every LLM API call on your system — from browser, terminal, scripts, or any app — compresses prompts before sending, and tracks token usage per user. No API keys stored. No code changes required. One command setup.

```
Before: Your app ──────────────────────────────────► api.openai.com  (2000 tokens, $0.01)
After:  Your app → PRISM proxy → compress → forward ► api.openai.com  (600 tokens, $0.003)
                                  ↓
                              Dashboard (token savings, cost, cache hits)
```

---

## Quick Install — System (Rust binary)

```bash
# 1. Build and install
cargo install --path .

# 2. One-time setup: CA cert + shell env + Claude Code MCP config
sudo prism init --global

# 3. Reload shell
source ~/.bashrc   # or ~/.zshrc

# 4. Start the proxy (intercepts all HTTPS traffic to AI providers)
prism serve --port 8080

# 5. Start the MCP server (for Claude Code tool integration)
prism mcp --port 3003
```

`prism init --global` does all of:
- Generates PRISM CA certificate at `~/.local/share/prism/ca/ca.crt`
- Installs CA cert to system trust store (`update-ca-certificates` on Linux, `security` on macOS)
- Appends to `~/.bashrc` and `~/.zshrc`:
  ```bash
  export HTTP_PROXY=http://localhost:8080
  export HTTPS_PROXY=http://localhost:8080
  export NO_PROXY=localhost,127.0.0.1
  export NODE_EXTRA_CA_CERTS=~/.local/share/prism/ca/ca.crt
  export REQUESTS_CA_BUNDLE=~/.local/share/prism/ca/ca.crt
  export SSL_CERT_FILE=~/.local/share/prism/ca/ca.crt
  ```
- Writes Claude Code MCP config to `~/.claude/settings.json`

---

## Quick Install — Docker (all-in-one)

```bash
# Start PRISM Hub (dashboard + API) + proxy + MCP server
cd prism-hub
docker compose --profile tools up -d

# Services:
#   localhost:5174   Dashboard (frontend)
#   localhost:3002   API (backend)
#   localhost:8080   Transparent proxy
#   localhost:3003   MCP server

# One-time: trust the CA cert from Docker volume
cat prism_data/ca/ca.crt | sudo tee /usr/local/share/ca-certificates/prism.crt
sudo update-ca-certificates

# Set proxy for current shell
export HTTP_PROXY=http://localhost:8080
export HTTPS_PROXY=http://localhost:8080
export REQUESTS_CA_BUNDLE=$PWD/prism_data/ca/ca.crt
```

---

## Browser Setup

After starting the proxy, configure your browser to use `localhost:8080` as HTTP/HTTPS proxy:

- **Chrome/Chromium**: Settings → System → Open proxy settings → set HTTP and HTTPS proxy to `localhost:8080`
- **Firefox**: Settings → Network → Manual proxy → HTTP proxy: `localhost:8080`
- **System-wide** (Linux GNOME): Settings → Network → Proxy → Manual → HTTP/HTTPS: `localhost:8080`

The PRISM CA cert must be trusted in the browser too:
- **Chrome**: Settings → Privacy → Manage certificates → Import `~/.local/share/prism/ca/ca.crt`
- **Firefox**: Settings → Privacy → Certificates → Import → select `ca.crt`, trust for websites

---

## Claude Code MCP Integration

```bash
# Add PRISM as an MCP server in Claude Code
claude mcp add prism --transport http http://localhost:3003

# Verify tools are visible
claude mcp list
```

Or set it manually in `~/.claude/settings.json`:
```json
{
  "mcpServers": {
    "prism": {
      "type": "http",
      "url": "http://localhost:3003"
    }
  }
}
```

---

## Available MCP Tools

| Tool | Description | Example |
|------|-------------|---------|
| `prism_count_tokens` | Count tokens in text (tiktoken cl100k) | `{"text": "hello world"}` → `2 tokens` |
| `prism_compress` | Compress text using BM25 scoring | `{"text": "...", "ratio": 0.5}` → compressed text |
| `prism_memory_search` | Search memory palace by query | `{"query": "authentication flow"}` |
| `prism_memory_save` | Save text to memory palace | `{"content": "key fact", "tags": ["auth"]}` |
| `prism_graph_query` | Query knowledge graph | `{"query": "what modules use auth?"}` |
| `prism_toon_encode` | TOON semantic encoding for vectors | `{"text": "concept to encode"}` |

---

## How Token Monitoring Works

Every request through the proxy is logged with:

| Field | Description |
|-------|-------------|
| `api_key_hash` | First 8 chars of hashed API key — never stored in full |
| `provider` | `openai`, `anthropic`, `gemini`, `mistral`, `cohere` |
| `model` | e.g. `gpt-4o-mini`, `claude-3-5-sonnet` |
| `orig_tokens` | Tokens in original request |
| `sent_tokens` | Tokens actually sent (after compression) |
| `resp_tokens` | Tokens in response |
| `cost_usd` | Estimated cost using public pricing |
| `latency_ms` | Round-trip time |
| `cache_hit` | Whether response was served from semantic cache |
| `compression_ratio` | `sent/orig` — lower = more compression |

Events are written to `~/.local/share/prism/analytics/proxy_events.jsonl` and also sent to PRISM Hub (if `PRISM_HUB_URL` is set) for dashboard display.

---

## Compression Pipeline

For each request body (JSON with `messages[].content`):

1. **Token count** — tiktoken cl100k accurate count
2. **Skip if small** — pass through unchanged if < 500 tokens
3. **BM25 sentence scoring** — TF-IDF weight per sentence, boost code definitions (`fn`, `def`, `class`), headers (`#`), error lines
4. **Greedy selection** — keep top sentences to hit `target_ratio` (default 0.75)
5. **Re-join in original order** — preserves flow
6. **Anthropic cache injection** — add `cache_control: {"type":"ephemeral"}` on system prompts automatically (90% cost reduction on repeated contexts)

Typical compression: 30-50% reduction. For large repeated system prompts with Anthropic: 90% reduction via caching.

---

## Architecture

```
Browser / terminal / scripts / any app
         │
         │  HTTP_PROXY=http://localhost:8080
         │  HTTPS_PROXY=http://localhost:8080
         ▼
PRISM Proxy  :8080
   ├── HTTP CONNECT tunnel
   ├── Per-domain TLS cert (signed by PRISM CA)
   ├── Detect AI provider by hostname
   ├── Compress messages[] content (BM25)
   ├── Inject Anthropic cache_control
   ├── Forward with original headers (API key unchanged)
   ├── Log telemetry → PRISM Hub + local JSONL
   └── Non-AI hosts → raw passthrough
         │
         ▼
api.openai.com / api.anthropic.com / etc.

PRISM MCP  :3003
   └── JSON-RPC 2.0 → Claude Code tools

PRISM Hub  :3002 / :5174
   └── Dashboard, analytics, per-user tracking
```

---

## CLI Reference

```bash
prism init --global          # One-time setup (CA cert + shell env + MCP config)
prism serve --port 8080      # Start transparent HTTPS proxy
prism mcp --port 3003        # Start MCP server
prism compress --file f.txt --ratio 0.5    # Compress a file
prism compress --string "..." --ratio 0.7  # Compress inline text
prism memory search "query"  # Search memory
prism memory save "content"  # Save to memory
prism graph query "question" # Query knowledge graph
```
