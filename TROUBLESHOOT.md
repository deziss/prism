# PRISM Troubleshooting Guide

## Table of Contents
- [BLAS / cblas_sgemm Linker Error](#blas--cblas_sgemm-linker-error)
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

---

## BLAS / cblas_sgemm Linker Error

### Symptom
```
error: linking with `cc` failed: exit status: 1
  = note: rust-lld: error: undefined symbol: cblas_sgemm
```
or
```
  = note: rust-lld: error: unable to find library -lopenblas
```

### Root Cause
`turbovec` (TurboQuant vector index) depends on `ndarray` with BLAS acceleration on Linux.
BLAS provides `cblas_sgemm` (matrix multiplication). Without a CBLAS-compatible library,
the linker fails.

### Fix — Option 1 (Recommended): Install libopenblas-dev

**Ubuntu / Debian:**
```bash
sudo apt-get install libopenblas-dev
```

**Fedora / RHEL / AlmaLinux:**
```bash
sudo dnf install openblas-devel
```

**Arch Linux:**
```bash
sudo pacman -S openblas
```

**macOS:**
```bash
brew install openblas
export LDFLAGS="-L$(brew --prefix openblas)/lib"
export PKG_CONFIG_PATH="$(brew --prefix openblas)/lib/pkgconfig"
```

Then rebuild:
```bash
cargo clean && cargo build
```

### Fix — Option 2: Use libgsl-dev (lighter, no tuning)

If you can't install openblas, GSL (GNU Scientific Library) ships `libgslcblas` which
implements the CBLAS interface. PRISM's `build.rs` auto-detects and uses it.

**Ubuntu / Debian:**
```bash
sudo apt-get install libgsl-dev
cargo build    # build.rs creates .blas-link/libopenblas.so -> libgslcblas.so.0 automatically
```

### Fix — Option 3: Manual Symlink (no sudo required)

If you have `libgslcblas.so.0` already installed at runtime but not the dev headers:

```bash
mkdir -p /home/anshukushwaha/95095/Backup/Desktop/learn/prism/.blas-link
ln -sf /usr/lib/x86_64-linux-gnu/libgslcblas.so.0 \
       /home/anshukushwaha/95095/Backup/Desktop/learn/prism/.blas-link/libopenblas.so
cargo build
```

> **Note:** Replace the path with the actual location of `libgslcblas.so.0` on your system.
> Find it with: `find /usr -name "libgslcblas*" 2>/dev/null`

### How build.rs Auto-Detects

PRISM's `build.rs` tries in order:
1. `pkg-config openblas` — works if `libopenblas-dev` is installed
2. `pkg-config cblas` — works on some distros
3. Searches common paths for `libgslcblas.so.0` / `libblas.so.3` and creates symlink
4. Prints install instructions if nothing found

The `.blas-link/` directory is machine-local and gitignored. Each developer/server
gets its own symlink created at first `cargo build`.

### Verifying BLAS is Found

Check build.rs output after `cargo build`:
```bash
cat target/debug/build/prism-*/output
# Should show:
# cargo:rustc-link-search=native=/path/to/.blas-link
# cargo:rustc-link-lib=openblas
```

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
`prism mcp` (default port 3003) exits immediately or prints address already in use.

### Fix
```bash
# Check if port is occupied
ss -tlnp | grep 3003

# Kill existing process
fuser -k 3003/tcp

# Use different port
prism mcp --port 3004
```

---

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
curl -N --proxy http://localhost:8080 --cacert ~/.local/share/prism/ca/ca.crt \
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

# Runtimes that use their own bundle
export NODE_EXTRA_CA_CERTS=~/.local/share/prism/ca/ca.crt   # Node
export REQUESTS_CA_BUNDLE=~/.local/share/prism/ca/ca.crt    # Python requests
export SSL_CERT_FILE=~/.local/share/prism/ca/ca.crt         # OpenSSL

# Firefox and Chrome keep separate stores — import ca.crt in their settings.
```

Note `--upstream` on `prism serve` is accepted but ignored. PRISM is a MITM
`CONNECT` proxy that routes by request hostname, not a reverse proxy to one
upstream.

---

## Telemetry never reaches PRISM Hub

### Symptom
`SELECT count(*) FROM "ProxyEvent"` stays 0 while the proxy logs traffic.

### Cause
The backend mounts its API under a global `/api` prefix. Posting to
`/analytics/proxy-event` 404s silently — telemetry is fire-and-forget, so nothing
surfaces.

### Fix
Fixed: a bare `PRISM_HUB_URL` now has `/api` appended automatically. Verify:
```bash
PRISM_HUB_URL=http://localhost:3002 prism serve --port 8080
# after one proxied request:
curl http://localhost:3002/api/analytics/proxy-stats?hours=1
```
The proxy also always writes `~/.local/share/prism/analytics/proxy_events.jsonl`,
so `prism gain` reports savings even with no Hub running.

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
prism mcp --port 3003 &
claude mcp add prism --transport http http://localhost:3003
claude mcp list
```
Check the server is actually up first:
```bash
curl -s http://localhost:3003/health
# {"status":"ok","mcp":true,"version":"2024-11-05"}
```
Note `prism mcp` defaults to 3003, matching what `init` registers.
