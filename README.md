# PRISM — Personal Reasoning & Intelligence System for Models

PRISM intercepts every LLM API call on your system — from browser, terminal, scripts, or any app — trims prompts before sending, and tracks token usage per user. No API keys stored. No code changes required. One command setup.

It is an explicit HTTP `CONNECT`/MITM proxy: clients reach it because `HTTP_PROXY`/`HTTPS_PROXY` point at it, not through kernel packet redirection. Streaming (SSE) is relayed chunk-by-chunk, so `"stream": true` behaves exactly as it does without the proxy.

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
- Registers PRISM as an MCP server in `~/.claude.json` (merged, not overwritten)

---

## Quick Install — Docker (all-in-one)

```bash
# Start PRISM Hub (dashboard + API) + proxy + MCP server
cd prism-hub
docker compose --profile tools up -d

# Services:
#   localhost:5174   Dashboard (frontend)
#   localhost:3002   API (backend)
#   localhost:8080   MITM proxy
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

Or set it manually in `~/.claude.json` — note this file, **not** `settings.json`;
Claude Code does not read MCP server definitions from `settings.json`:
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
| `cache_hit` | Reserved for the semantic cache (not yet wired — always false) |
| `compression_ratio` | `sent/orig` — lower = more compression |

Events are written to `~/.local/share/prism/analytics/proxy_events.jsonl` and also
POSTed to `$PRISM_HUB_URL/analytics/proxy-event` when `PRISM_HUB_URL` is set. The
Hub mounts its API under `/api`, so a bare host gets `/api` appended
automatically. `prism gain` reports measured savings straight from this log.

---

## Compression Pipeline

Two rules constrain what PRISM is willing to rewrite. Both exist because the
naive version of this feature costs more than it saves.

**Rule 1 — the cacheable prefix is never touched.**
OpenAI caches implicitly on a stable prefix of >=1024 tokens; Anthropic caches at
`cache_control` breakpoints. A cache read bills at **10% of input** on Anthropic
(50% on OpenAI). Rewriting a system prompt to shave 25% off it turns a 90%
discount into a full-price miss — a large net loss on any repeated context. So
PRISM leaves the system prompt and all earlier messages byte-identical and adds
cache breakpoints there instead. Only the last 2 messages are eligible for
compression, which is where the bulk of new tokens actually is.

**Rule 2 — code is never compressed.**
BM25 drops whole lines. Applied to a fenced code block it would silently delete
lines from the middle and forward broken code to the model. Fenced blocks,
indented blocks, and diff hunks are segmented out and passed through verbatim;
only prose is ever scored.

For an eligible prose span:

1. **Token count** — tiktoken cl100k
2. **Skip if small** — unchanged below 500 tokens; compression is not worth the semantic risk
3. **BM25 sentence scoring** — TF-IDF weight per line, boosting definitions, headers, and error lines
4. **Greedy selection** to hit `compression_ratio` (default 0.75; set in `~/.prism/config.yaml`)
5. **Re-join in original order**
6. **Never grows the payload** — if the "compressed" text is not smaller, the original is sent

On Anthropic, up to 4 `cache_control` breakpoints are placed: one on the system
prompt, the rest spread across the stable part of the conversation so long
sessions keep a live breakpoint inside the cache lookback window instead of
ageing out and re-paying full price every turn.

---

## Architecture

```
Browser / terminal / scripts / any app
         │
         │  HTTP_PROXY=http://localhost:8080
         │  HTTPS_PROXY=http://localhost:8080
         ▼
PRISM Proxy  :8080
   ├── HTTP CONNECT tunnel, keep-alive (many requests per tunnel)
   ├── Per-domain TLS cert (signed by PRISM CA)
   ├── Detect AI provider by hostname
   ├── Compress tail messages only (BM25, code-safe)
   ├── Place Anthropic cache_control breakpoints
   ├── Forward with original headers (API key unchanged)
   ├── Relay response streaming — SSE flushed chunk-by-chunk
   ├── Log telemetry → PRISM Hub + local JSONL
   └── Non-AI hosts → raw passthrough, untouched
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
