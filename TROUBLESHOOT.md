# PRISM Troubleshooting Guide

## Table of Contents
- [BLAS / cblas_sgemm Linker Error (obsolete since 0.2.0)](#blas--cblas_sgemm-linker-error)
- [OpenSSL Not Found](#openssl-not-found)
- [Sled Database Corruption](#sled-database-corruption)
- [MCP Server Not Starting](#mcp-server-not-starting)
- [Proxy: TLS handshake fails / SSL_ERROR_SYSCALL](#proxy-tls-handshake-fails--ssl_error_syscall)
- [Proxy: requests hang, or streaming never appears](#proxy-requests-hang-or-streaming-never-appears)
- [Proxy: certificate errors from clients](#proxy-certificate-errors-from-clients)
- [Telemetry never reaches PRISM Hub](#telemetry-never-reaches-prism-hub)
- [tiktoken slow on large requests](#tiktoken-slow-on-large-requests)
- [Memory Palace Empty After Restart](#memory-palace-empty-after-restart)
- [Claude Code does not see PRISM tools](#claude-code-does-not-see-prism-tools)
- [Anthropic 400 Error: cache_control.ttl ordering (1h after 5m)](#anthropic-400-error-cache_controlttl-ordering-1h-after-5m)

---

## BLAS / cblas_sgemm Linker Error

**No longer applicable as of 0.2.0.** `turbovec` 1.0 dropped its BLAS/CBLAS
dependency, which in turn made `build.rs` — whose only job was locating a system
`libopenblas`/`libcblas`/`libgslcblas` — dead code. Both are gone, so there is no
CBLAS symbol to fail to link and nothing to install.

If you hit `undefined symbol: cblas_sgemm` or `unable to find library -lopenblas`,
you are building a pre-0.2.0 checkout. Update, or `cargo clean` first — a stale
`.blas-link/` directory and its cached link flags can survive the upgrade.

---

## OpenSSL Not Found

### Symptom
```
error: failed to run custom build command for `openssl-sys`
Could not find directory of OpenSSL installation
```

### Fix
```bash
# Ubuntu/Debian
sudo apt-get install pkg-config libssl-dev

# Fedora/RHEL
sudo dnf install openssl-devel

# macOS
brew install openssl
export PKG_CONFIG_PATH="$(brew --prefix openssl)/lib/pkgconfig"
```

---

## Sled Database Corruption

### Symptom
```
thread 'main' panicked at 'called `Result::unwrap()` on an `Err` value: Io(Os { code: 22, ... })'
```
or cache/memory commands hang.

### Fix
```bash
# Clear sled database (data is lost, but PRISM rebuilds it)
rm -rf ~/.local/share/prism/cache/sled/
rm -rf ~/.local/share/prism/memory/sessions/

# Restart PRISM
prism memory list
```

---

## MCP Server Not Starting

### Symptom
`prism mcp` (default port 27182) exits immediately or prints address already in use.

### Fix
```bash
# Check if port is occupied
ss -tlnp | grep 27182

# Kill existing process
fuser -k 27182/tcp

# Use different port
prism mcp --port 27184
```

---

## Shims: a command hangs, or every command broke

### Symptom

After `prism shim install`, an interactive command (`git rebase -i`, `docker run -it`)
produces no output — or every command fails with "No such file or directory".

### Root Cause

Two different problems.

A hanging interactive command means output is being captured when a person is reading it.
`PRISM_SHIM=auto` (the default) only filters when stdout is a *pipe*, so this should not
happen at a terminal; it will happen if you set `PRISM_SHIM=always`.

"No such file or directory" on every command means the shims point at a binary that no
longer exists — almost always because they were installed from `target/debug` or
`target/release` and the repo was later cleaned or moved. `prism shim install` warns when
it detects this, but the warning is easy to miss.

### Fix

```bash
# interactive command captured:
PRISM_SHIM=off git rebase -i        # one command
export PRISM_SHIM=off               # this shell

# shims point at a deleted binary — recover PATH first, then reinstall from a stable path:
export PATH=$(echo "$PATH" | tr ':' '\n' | grep -v '/prism/shims$' | paste -sd:)
cargo build --release
cp target/release/prism ~/.local/bin/prism
~/.local/bin/prism shim install --path
```

`prism shim status` reports whether the shims are installed and whether they are actually
first on `PATH`. `prism shim uninstall` removes them and the shell-rc block.

## Cache: "cache unavailable ... could not acquire lock"

### Symptom

`prism cache stats` or `prism cache query` exits non-zero with a lock error.

### Root Cause

sled takes an exclusive lock on its directory, and `prism serve` holds it for the whole
time the proxy is running. This is reported rather than silently shown as an empty cache,
because "no entries" and "cannot read the store" are different answers.

### Fix

Stop `prism serve`, or query through the proxy's own MCP endpoint (`prism_cache_lookup`)
instead of the CLI.

## Proxy: TLS handshake fails / SSL_ERROR_SYSCALL

### Symptom
```
* OpenSSL SSL_connect: SSL_ERROR_SYSCALL in connection to api.openai.com:443
```
and the proxy log shows:
```
Could not automatically determine the process-level CryptoProvider from Rustls
crate features.
```

### Cause
`reqwest`'s rustls-tls pulls in `aws-lc-rs` alongside our `ring`, so rustls 0.23
refuses to pick a backend on its own.

### Fix
Already fixed in `proxy.rs` — `install_crypto_provider()` runs at startup. If you
see this, your binary predates that; rebuild.

---

## Proxy: requests hang, or streaming never appears

### Symptom
A proxied request sits for 60+ seconds, or `"stream": true` produces nothing
until the response is complete.

### Cause
Fixed. The old response loop buffered until upstream EOF, which never arrives on
a keep-alive connection.

### Fix
Rebuild. Confirm with a streaming request — tokens must appear incrementally:
```bash
curl -N --proxy http://localhost:27181 --cacert ~/.local/share/prism/ca/ca.crt \
  https://api.anthropic.com/v1/messages \
  -H "x-api-key: $ANTHROPIC_API_KEY" -H "anthropic-version: 2023-06-01" \
  -d '{"model":"claude-3-5-haiku-20241022","max_tokens":100,"stream":true,
       "messages":[{"role":"user","content":"count to 20 slowly"}]}'
```

---

## Proxy: certificate errors from clients

### Symptom
`SSL certificate problem: unable to get local issuer certificate`, or the browser
warns about an untrusted certificate for an AI provider domain.

### Cause
PRISM presents a certificate it signs itself. Clients must trust the PRISM CA.

### Fix
```bash
# System trust store (done by `prism init --global`)
sudo cp ~/.local/share/prism/ca/ca.crt /usr/local/share/ca-certificates/prism.crt
sudo update-ca-certificates

# Runtimes that use their own bundle.
# NODE_EXTRA_CA_CERTS *adds* a CA, so it takes the bare cert:
export NODE_EXTRA_CA_CERTS=~/.local/share/prism/ca/ca.crt          # Node
# The rest *replace* the trust store, so they take the combined bundle
# (system roots + PRISM CA) that `prism init --global` writes:
export REQUESTS_CA_BUNDLE=~/.local/share/prism/ca/ca-bundle.crt    # Python requests
export CURL_CA_BUNDLE=~/.local/share/prism/ca/ca-bundle.crt        # curl
export SSL_CERT_FILE=~/.local/share/prism/ca/ca-bundle.crt         # OpenSSL

# Chromium/Electron apps (VS Code, Antigravity, Chrome) and Firefox read NSS —
# not the variables above and not the system store:
sudo apt install libnss3-tools
certutil -A -d sql:$HOME/.pki/nssdb -n "PRISM Local CA" -t C,, \
         -i ~/.local/share/prism/ca/ca.crt
```

> **Pointing `SSL_CERT_FILE` at the bare `ca.crt` breaks every client.** Those
> variables replace the CA set rather than extending it, so with only the PRISM CA
> installed, any host PRISM does *not* intercept (an IDE's auth, telemetry, update
> and marketplace endpoints) fails verification. That is the classic
> "works after `prism-off` + restart" symptom. `prism-enable` and
> `prism init --global` now always write `ca-bundle.crt` here.

### Only some apps fail
Loopback traffic is tunnelled untouched except on local model ports (`11434`,
`1234`; override with `PRISM_LOCAL_AI_PORTS`). If a local service still breaks,
check that its port is not in that list.

Note `--upstream` on `prism serve` is accepted but ignored. PRISM is a MITM
`CONNECT` proxy that routes by request hostname, not a reverse proxy to one
upstream.

---

## Telemetry never reaches PRISM Hub

### Symptom
`SELECT count(*) FROM "ProxyEvent"` stays 0 while the proxy logs traffic — or rows
arrive but every numeric column is 0 and `apiKeyHash` is `unknown`.

### Cause
The all-zeros case was the contract bug fixed in 0.2.0: prism built the body as a
hand-written JSON literal with snake_case keys (`orig_tokens`, `api_key_hash`)
while the hub read camelCase behind `?? 0` / `?? 'unknown'` fallbacks. Every field
missed and nothing errored. Events are now typed structs with
`#[serde(rename_all = "camelCase")]`, and both repos pin the shape with a shared
fixture (`tests/fixtures/hub-events.json`), so this cannot silently recur.

For zero rows, the usual causes are, in order:

1. **The agent was never enrolled.** Ingest requires a bearer token; an
   unauthenticated POST is rejected, not silently accepted.
2. **`PRISM_HUB_URL` is unset**, so the shipper never starts. Events still land in
   the local spool and JSONL.
3. **Only `prism cmd` ran.** By design it never touches the network — it appends to
   the spool and a `serve` daemon (or `prism hub flush`) drains it. This keeps the
   shim's per-command overhead at ~0.05s instead of the 0.57s a tokenizer cost.

### Fix
```bash
prism hub enroll --url http://localhost:27183 --token <join-token>   # once per machine
prism hub status                                                     # spool depth, last flush
prism hub test                                                       # send one synthetic event
prism hub flush                                                      # force a drain now
```
Then, after one proxied request:
```bash
curl 'http://localhost:27183/api/analytics/proxy-stats?hours=1'
```
Non-zero `origTokens`/`sentTokens`/`costUsd` and a real provider and model mean the
pipe is healthy.

Nothing is lost while the hub is down: the shipper spools to
`~/.local/share/prism/analytics/hub_spool.jsonl` and truncates it only after a 2xx.
Each event carries its own `ts`, so a backlog flushed after an outage is filed
under when it happened rather than when it arrived. The proxy also always writes
`~/.local/share/prism/analytics/proxy_events.jsonl`, so `prism gain` reports
savings with no hub at all.

---

## tiktoken slow on large requests

### Symptom
A large proxied request adds tens of seconds of latency that the proxy's own
logged `latency_ms` does not account for.

### Cause
`cl100k_base()` parses a ~1.7MB BPE table, and the scorer called it once per
sentence — so the table was rebuilt hundreds of times per request.

### Fix
Fixed: the tokenizer is built once per process (`analytics::bpe`). Note the table
is compiled into the binary, so there is no download and no network dependency.

A debug build still pays roughly a second on the first call, and far more on a
cold start — debug is 10-50x slower at this. Do not read latency numbers off a
debug build; use `cargo build --release`.

---

## Memory Palace Empty After Restart

### Symptom
`prism memory list` shows 0 blocks after restarting.

### Cause
Memory is stored in `~/.local/share/prism/sessions/` as JSONL files.
If running inside Docker without a volume mount, data is lost on container restart.

### Fix
```bash
# Check if data dir exists
ls ~/.local/share/prism/sessions/

# Docker: mount the data dir
docker run -v ~/.local/share/prism:/root/.local/share/prism prism

# Verify data is being written
prism memory save mykey "test value"
cat ~/.local/share/prism/sessions/core/blocks.jsonl
```

---

## Common Environment Variables

| Variable | Purpose | Example |
|----------|---------|---------|
| `RUST_LOG` | Log level for proxy/MCP | `RUST_LOG=debug` |
| `PRISM_DATA_DIR` | Override data directory | `PRISM_DATA_DIR=/opt/prism/data` |
| `OPENBLAS_NUM_THREADS` | Limit BLAS thread count | `OPENBLAS_NUM_THREADS=1` |
| `PKG_CONFIG_PATH` | Help pkg-config find libs | See OpenSSL / BLAS fixes above |

---

## Getting Help

1. Run `prism --help` or `prism <subcommand> --help`
2. Check build output: `cargo build 2>&1 | grep error`
3. Enable verbose logging: `RUST_LOG=prism=debug prism <cmd>`

---

## Claude Code does not see PRISM tools

### Symptom
`claude mcp list` does not show `prism`, or the tools never appear in a session.

### Cause
MCP servers are configured in `~/.claude.json`. An entry in
`~/.claude/settings.json` is silently ignored — earlier versions of
`prism init --global` wrote to the wrong file.

### Fix
```bash
prism mcp --port 27182 &
claude mcp add prism --transport http http://localhost:27182
claude mcp list
```
Check the server is actually up first:
```bash
curl -s http://localhost:27182/health
# {"status":"ok","mcp":true,"version":"2024-11-05"}
```
Note `prism mcp` defaults to 27182, matching what `init` registers.

---

## Anthropic 400 Error: cache_control.ttl ordering (1h after 5m)

### Symptom
When running agentic workloads (Claude Code, Cursor, Windsurf, or Anthropic SDK clients) across long multi-turn sessions with tools and system prompts, the API returns:
```text
API Error: 400 messages.0.content.3.content.0.cache_control.ttl: a ttl='1h' cache_control block must not come after a ttl='5m' cache_control block. Note that blocks are processed in the following order: tools, system, messages.
```

### Root Cause
Anthropic requires prompt cache blocks to follow a non-increasing TTL order across the evaluation sequence: `tools` → `system` → `messages`. If an earlier block (e.g., in `system` or an earlier turn) uses the default 5-minute TTL (`5m` or omitted `ttl`), and a later block (e.g., in a tool result or recent message) specifies `ttl='1h'`, Anthropic rejects the entire request with an HTTP 400 error.

### PRISM Solution
PRISM transparently enforces Anthropic caching invariants inside `src/proxy.rs`:
1. **Automatic TTL Promotion**: If any breakpoint specifies `ttl='1h'`, all preceding breakpoints (with `5m` or default TTL) are automatically upgraded to `ttl='1h'`.
2. **Strict 4-Breakpoint Limit**: Anthropic permits at most 4 active breakpoints across the request. PRISM automatically prunes excess breakpoints from tail messages.
3. **Client Cache Strategy Preservation**: If an upstream agent (e.g., Claude Code or Cursor) already manages its own cache breakpoints, PRISM preserves the caller's strategy without injecting redundant duplicate breakpoints.

### Verification & Monitoring
To inspect dynamic cache modifications in real time:
```bash
journalctl --user -u prism-proxy.service -f
```
Example diagnostic log output:
```text
WARN cache_control: promoting /system/0/cache_control from ttl=5m (default) to ttl=1h (block 0 of 2, 1h block at index 1)
INFO cache_control: promoted 1 breakpoint(s) to ttl=1h to fix TTL ordering
```

To disable PRISM automatic cache injection while retaining request normalization:
```bash
export PRISM_NO_CACHE_CONTROL=1
```
