# PRISM Troubleshooting Guide

## Table of Contents
- [BLAS / cblas_sgemm Linker Error](#blas--cblas_sgemm-linker-error)
- [OpenSSL Not Found](#openssl-not-found)
- [Sled Database Corruption](#sled-database-corruption)
- [MCP Server Not Starting](#mcp-server-not-starting)
- [Proxy 502 / Upstream Connection Refused](#proxy-502--upstream-connection-refused)
- [tiktoken-rs Slow First Run](#tiktoken-rs-slow-first-run)
- [Memory Palace Empty After Restart](#memory-palace-empty-after-restart)

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
`prism mcp --port 9090` exits immediately or prints address already in use.

### Fix
```bash
# Check if port is occupied
ss -tlnp | grep 9090

# Kill existing process
fuser -k 9090/tcp

# Use different port
prism mcp --port 9091
```

---

## Proxy 502 / Upstream Connection Refused

### Symptom
```
prism serve --port 8080 --upstream http://localhost:11434
```
Returns 502 for all requests.

### Fix
- Verify upstream is running: `curl http://localhost:11434/v1/models`
- Check firewall: `sudo ufw status`
- Try with explicit IP: `--upstream http://127.0.0.1:11434`
- Check proxy logs: `RUST_LOG=debug prism serve --port 8080`

---

## tiktoken-rs Slow First Run

### Symptom
First `prism count` or `prism gain` takes 5-10 seconds.

### Cause
`tiktoken-rs` downloads the `cl100k_base` BPE vocabulary on first use and caches it.

### Fix
This is normal. Subsequent runs use the cache. If offline:
```bash
# Pre-download on a machine with internet, copy cache to offline server
ls ~/.cache/huggingface/hub/  # tiktoken cache location
```

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
