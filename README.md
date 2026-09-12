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
- **Transparent MITM Proxy (`:27181`, loopback-only)**: Transparent HTTP `CONNECT` tunnel generating per-domain certificates via local root CA. Binds `127.0.0.1` by default — widen with `prism serve --bind` only deliberately, since this process holds a CA private key and mints certificates for any host a client asks for. Automatically handles streaming SSE responses chunk-by-chunk with zero latency overhead.
- **Prefix-Preserving & Invariant-Compliant Prompt Caching**: Strictly preserves system prompts and conversation prefixes while dynamically enforcing Anthropic cache ordering invariants (auto-promotes preceding breakpoints to `1h` when later blocks use `1h` to prevent HTTP 400 errors, strictly enforces Anthropic's 4-breakpoint limit, and honors caller-defined caching strategies). Guarantees **90% Anthropic prompt cache discounts** and **50% OpenAI discounts**.
- **Anthropic Context Pruning**: Opts long agent runs into server-side `clear_tool_uses` context pruning, preventing stale tool results from accumulating across long agent interactions.
- **TurboVec Quantized Semantic Cache (`prism cache`)**: 16-dimensional SIMD quantized vector embeddings enabling sub-millisecond local ANN semantic response retrieval. *Requires policy from a licensed [PRISM Hub](#fleet-telemetry--policy-prism-hub).*
- **GraphRAG & Graphify Integration (`prism graph`)**: Ingests and queries codebase dependency graphs (`graphify-out/graph.json` or custom graphs) for architectural explanations, shortest path tracing, and god-node detection. *Requires policy from a licensed [PRISM Hub](#fleet-telemetry--policy-prism-hub).*
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
prism-disable    # (or alias: prism-off) — stops the daemons, keeps the install

# Complete removal, and the only reliable way to check what is actually left:
prism uninstall              # audits and changes nothing; exits non-zero if traces remain
prism uninstall --remove
```

### What `prism-enable` Configures:
1. **Systemd User Daemons**:
   - `prism-proxy.service`: Transparent MITM Proxy active on `http://127.0.0.1:27181`
   - `prism-mcp.service`: Model Context Protocol server active on `http://127.0.0.1:27182`
2. **Safe Proxy Scoping (No Global Environment Hijacking)**:
   - Global `HTTP_PROXY` is NOT injected into desktop environment files or shell startup files, preventing IDE/browser connectivity failure if the proxy is stopped.
   - Run any command through PRISM proxy: `prism proxy <command>` (e.g. `prism proxy claude`).
   - Enable proxy for current shell session only: `eval $(prism proxy env)`.
   - Disable proxy in current shell session: `eval $(prism proxy off)`.
   - Adds the PRISM root CA for Node.js (`NODE_EXTRA_CA_CERTS=ca.crt`) and combined bundle for clients that replace the store (`ca-bundle.crt`).
   - Registers the CA in the NSS databases used by Chromium/Electron apps and Firefox (requires `libnss3-tools`).

   If `certutil` was missing when you ran `prism-enable`, or an app (VS Code,
   Antigravity, Chrome, Firefox) still shows certificate errors, register it manually:
   ```bash
   sudo apt install libnss3-tools
   mkdir -p ~/.pki/nssdb
   certutil -A -d sql:$HOME/.pki/nssdb -n "PRISM Local CA" -t "C,," \
            -i ~/.local/share/prism/ca/ca.crt
   ```
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
| **Project Config** | `<project_root>/` | `.prismrc` | Local repository overrides (precedes global config) |
| **Persistent Data** | `$XDG_DATA_HOME/prism/` | `~/.local/share/prism/` | Persistent databases and stateful assets: |
| ↳ *CA Certificates* | `$XDG_DATA_HOME/prism/ca/` | `~/.local/share/prism/ca/` | Root CA private key (`ca.key`, mode 0600) and cert (`ca.crt`) |
| ↳ *Semantic Cache* | `$XDG_DATA_HOME/prism/cache/` | `~/.local/share/prism/cache/` | TurboVec quantized SIMD vector embeddings cache |
| ↳ *Knowledge Graph*| `$XDG_DATA_HOME/prism/graph/` | `~/.local/share/prism/graph/` | GraphRAG Petgraph serialized node relations |
| ↳ *Memory Palace* | `$XDG_DATA_HOME/prism/memory/`| `~/.local/share/prism/memory/` | JSONL database for 3-tier associative memory (recall/core/archive) |
| ↳ *Analytics* | `$XDG_DATA_HOME/prism/analytics/`| `~/.local/share/prism/analytics/` | Token logs and `proxy_events.jsonl` |
| **State / Tees** | `$XDG_STATE_HOME/prism/` | `~/.local/share/prism/tee/` | Raw crash and failure logs from `prism cmd` |
| **Runtime Daemons** | Systemd User Units | `~/.config/systemd/user/` | `prism-proxy.service` (:27181) and `prism-mcp.service` (:27182) |

#### Configuration Resolution Hierarchy (12-Factor App):
```
[1] CLI Flags          (--port 27181, --guide)
      ↓
[2] Environment Vars   (HTTP_PROXY, PRISM_NO_CONTEXT_EDITING)
      ↓
[3] Hub Policy         (<data>/hub-config.yaml, written by `prism hub config`)
      ↓
[4] Project Config     (<workspace>/.prismrc)
      ↓
[5] Global Config      (~/.config/prism/config.yaml)
      ↓
[6] Defaults           (PrismConfig::default())
```

A layer wins wherever it *states* a value, so hub policy can switch a feature both on
and off. `graph_enabled` and `cache_enabled` default to `false` and are enabled by hub
policy — see [`prism graph`](#graphrag--codebase-intelligence-prism-graph) and
[Semantic Cache](#semantic-cache--analytics).

---

## AI Agent & IDE Integration

### 1. Claude Code (Anthropic)

PRISM's MCP server is built on `rmcp` (the official Rust SDK) at protocol revision
**2026-07-28**, and offers two transports. For a same-machine agent prefer **stdio** —
no port, no listener, no certificate:

```bash
claude mcp add prism -- prism mcp --stdio
```
Or manually in `~/.claude.json`:
```json
{
  "mcpServers": {
    "prism": {
      "command": "prism",
      "args": ["mcp", "--stdio"]
    }
  }
}
```

Streamable HTTP remains available for a remote consumer such as PRISM Hub:
```bash
prism mcp --port 27182                          # 127.0.0.1 only
claude mcp add prism --transport http http://localhost:27182
```

> **The HTTP transport binds `127.0.0.1` by default.** Serving it on other interfaces
> requires both `--bind` and a bearer token, because the tool surface includes
> `prism_read_file`:
> ```bash
> prism mcp --bind 0.0.0.0 --auth-token "$(openssl rand -hex 32)"
> ```
> `--bind` without a token is refused outright rather than starting an unauthenticated
> file-reading server on the LAN.

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
prism-env python my_agent.py    # sets all of the below for one command only
```

`prism-env` is the safer form: it scopes everything to the process it launches, so a
stopped proxy cannot strand the rest of your session. It also refuses to run when
nothing is listening on `:27181`, rather than handing the command a proxy that does not
answer. To do it by hand:

```bash
export HTTP_PROXY=http://127.0.0.1:27181
export HTTPS_PROXY=http://127.0.0.1:27181
export NODE_EXTRA_CA_CERTS=~/.local/share/prism/ca/ca.crt         # adds a CA
export REQUESTS_CA_BUNDLE=~/.local/share/prism/ca/ca-bundle.crt   # replaces the store
```

Exports like these outlive the proxy. A shell — or a login session — that still has them
set after PRISM is removed points every client at a dead port and at CA files that no
longer exist, and nothing can retract them from a process that is already running. See
`prism uninstall`, which finds and reports exactly that.

`REQUESTS_CA_BUNDLE`, `SSL_CERT_FILE` and `CURL_CA_BUNDLE` replace the CA set instead of
extending it — always give them `ca-bundle.crt` (system roots + PRISM CA), never the bare
`ca.crt`, or every host PRISM does not intercept fails verification.

---

## CLI Command Reference

### AST Code Reader (`prism read`)
```bash
prism read src/lib.rs --mode signatures    # Function signatures, structs, traits (72% token savings)
prism read src/main.rs --mode skeleton      # Outline of symbols without bodies (77% token savings)
prism read src/proxy.rs --mode map          # Structural outline map of symbols & ranges (93% token savings)
prism read src/cli.rs --lines 50-120 -n     # Line range slice with line numbers (68% token savings)
```

### Filtered Command Runner & Failure Tee (`prism cmd`)
```bash
prism cmd cargo test                       # Strip noisy compiler ANSI progress, track token spend
prism cmd git status                       # On failure, dumps raw stderr to ~/.local/share/prism/tee/
```

#### Making every agent use the filters (`prism shim`)

A filter that never runs saves nothing, and the filters are the only lever whose savings
*compound*: a tool result cut from 40k to 4k tokens is not saved once, it is saved again
on every later turn that re-sends the conversation.

```bash
prism shim install --path   # shims + the PATH line in every shell rc you have
prism shim status           # installed? actually first on PATH?
prism shim uninstall        # shims only; `prism uninstall` covers the whole machine
```

Do not put the shim directory in `~/.claude/settings.json` under `env.PATH`. Claude Code
does not expand `${PATH}` there, so the agent ends up with the shim directory plus a
literal string and no working PATH at all — and it survives uninstallation, because the
shims are deleted while the setting pointing at them is not. Launch Claude Code from a
shell that already has the shims on PATH instead.

Install prism to a stable location **before** running this. The shims hard-code the path
of the binary that wrote them, so installing from `target/release` means a later
`cargo clean` breaks every shimmed command on the machine. prism refuses to do that
silently — it warns and prints the correct command — but the right order is:

```bash
cargo build --release
cp target/release/prism ~/.local/bin/prism
~/.local/bin/prism shim install --path
exec $SHELL
```

`prism init --global` also does this, but it additionally turns on the HTTP proxy
environment; use `shim install --path` if you only want the filters.

Rather than one integration per client, the shims put a directory of tiny executables
ahead of the real tools on `PATH`. Every client that runs shell commands is covered by
the same code — Claude Code, Cursor, Windsurf, Codex, Aider, Cline, OpenCode, and any
client that does not exist yet.

Measured, with no client configuration at all:

| command | raw | via shim |
|---|---|---|
| `ls -laR src` | 1,588 tok | **456** |
| `find . -name '*.rs'` | 1,291 tok | **115** |

**Your own terminal is unaffected.** `PRISM_SHIM=auto` (the default) filters only when
stdout is a pipe — an agent reading — and hands the real tool straight through when
stdout is a terminal, so `git rebase -i`, colours and progress bars behave normally.
`PRISM_SHIM=always` filters regardless; `PRISM_SHIM=off` makes the shims transparent.

Exit codes propagate, stdin is passed through (`echo '{}' | jq .` still works), and
prism removes the shim directory from the child's `PATH` before spawning the real tool —
so there is no recursion, and resolution stays dynamic, which keeps `nvm`, `rbenv` and
`pyenv` working.

GUI-launched editors do not read your shell rc. Set `PATH` in the client instead:

```jsonc
// ~/.claude/settings.json
{ "env": { "PATH": "/home/you/.local/share/prism/shims:${PATH}" } }

// Cursor / VS Code settings.json
"terminal.integrated.env.linux": { "PATH": "/home/you/.local/share/prism/shims:${env:PATH}" }
```

`prism shim path` prints the directory for any other client's config.

#### Adding a tool without recompiling

`prism cmd` has Rust filters for 108 commands. For a tool it does not know, drop a YAML
file in `~/.config/prism/filters/`:

```yaml
tool: nomad
subcommands:
  status:      { shape: table, cap: list_max_lines }
  alloc-logs:  { shape: logs }
  job-inspect: { shape: json }
  run:         { shape: verb-group, verbs: [started, updated] }
  version:     { shape: raw }        # already minimal — leave it alone
default:       { shape: generic }
```

`shape` names an existing primitive, so a rule inherits the fidelity contract: cuts still
emit `[+N more …]`, caps still come from `filters:`/`PRISM_FILTER_*`, and `prism cmd`
still tees the raw output. Available shapes: `table` `logs` `json` `describe` `tree`
`verb-group` `dedupe` `tail` `errors` `generic` `raw`.

What YAML deliberately cannot do is express the parsing. A regex keep/drop language would
be more expressive and strictly worse — regex can drop a line but cannot *count* what it
dropped, and every marker in prism is arithmetic over parsed structure. So a rule handles
a tool that prints a shape prism already understands; a genuinely new shape still needs a
Rust filter.

A rule only applies where the built-in dispatch would have fallen through to `generic`.
Add `override: true` to take precedence over a built-in filter. `prism config --show`
lists the rules that loaded and the files that failed, so a typo is reported rather than
silently doing nothing.

### GraphRAG & Codebase Intelligence (`prism graph`)

**Requires a licensed PRISM Hub.** `graph_enabled` defaults to `false`; hub policy sets
it to `true`. Without it every `prism graph` subcommand exits non-zero saying the feature
is disabled and how to enable it — it does not report an empty or missing graph.

```bash
prism graph query "how does proxy work?"   # Query auto-detected Graphify knowledge graph
prism graph explain proxy_server           # Explain specific node and connected edges
prism graph path cli proxy                 # Find shortest dependency path between symbols
prism graph god-nodes --top 5              # Identify architectural bottleneck nodes
```

The gate is the resolved `graph_enabled` flag and nothing else: prism performs no licence
check, holds no key, and makes no network call to decide. It reads the configuration
layer a hub wrote and honours it. The five `prism_graph_*` MCP tools return the same
message rather than empty results, so an agent cannot mistake "off" for "nothing found".

### Semantic Cache & Analytics

The **semantic cache requires a licensed PRISM Hub**: `cache_enabled` defaults to `false`
and hub policy sets it to `true`. `prism gain` and the analytics are free and unaffected.

```bash
prism cache query "prompt query"           # Lexical re-rank over stored prompts, thresholded
prism cache stats                          # Entries and store location
prism gain                                 # Real-time token and dollar savings dashboard
prism gain --history                       # View detailed historical command log
```

With the cache disabled, `prism cache` exits non-zero reporting *disabled* — not
`Total entries: 0`, and not the store-locked error, which is a different problem with a
different fix. `prism serve` logs the same once at startup and relays without recording
or replaying; nothing else about the proxy changes.

The proxy fills the cache as it relays. Recording and serving are separate switches
because they carry different risk:

| variable | default | effect |
|---|---|---|
| `PRISM_CACHE_RECORD=0` | recording **on** | stop writing prompts and responses to `~/.local/share/prism/cache/` |
| `PRISM_CACHE_SERVE=deterministic` | serving **off** | replay a cached response when the caller pinned `temperature: 0` |
| `PRISM_CACHE_SERVE=always` | serving **off** | replay whatever matches, at any temperature |
| `PRISM_CACHE_MIN_SIMILARITY` | 0.55 | floor for `prism cache query`; below it a match is noise |

**`cache_enabled` sits above all three.** They are preferences *within* a cache that
exists — a privacy kill switch and a replay opt-in — so none of them can reach a store
the configuration has not enabled, and `PRISM_CACHE_SERVE=always` is not a way to switch
the feature on. With the cache off the proxy never keys a request at all, so the serve
leg has nothing to look up, the record leg nothing to write, and
`PRISM_CACHE_MIN_SIMILARITY` (which only ranks inside a similarity search) is
unreachable. With the cache on, all three behave exactly as documented above.

Recording never changes what the client receives — it only fills a store that was
previously always empty. Serving *replaces* a live model call, which is why it is opt-in:
an LLM response is not a pure function of its request, so the same question at two points
in an agent run can have two different correct answers.

Four things are never replayed, regardless of the switch: a response containing
`tool_use`/`tool_calls` (replaying one makes the agent re-run a tool against a stale id),
a body whose framing does not match the request (SSE to a JSON caller or the reverse), a
non-200 response, and a body that outran the sniff buffer. Similarity hits are only ever
*shown* — `prism cache query` and the `prism_cache_lookup` MCP tool — never auto-served;
the serving path takes exact key matches only, where the key covers provider, model,
system prompt, full message list, tools and sampling.

### Fleet Telemetry & Policy (`prism hub`)

`prism hub` connects an agent to [PRISM Hub](https://github.com/deziss/prism-hub), the
fleet control plane: telemetry out, policy in.

```bash
prism hub enroll --url http://localhost:27183 --token <join-token>
prism hub status                           # spool depth, last flush, agent id
prism hub config                           # pull team policy into <data>/hub-config.yaml
prism hub flush                            # drain the spool now
prism hub test                             # post one synthetic event and report the result
```

Four event kinds ship: `proxy` (tokens, cost, cache and image savings, prompt-cache
hits), `command` (every filtered `prism cmd`/shim invocation), `cache`, and `session`.
The `command` stream is the one that compounds — a 40k tool result cut to 4k is saved
again on every later turn that re-sends the conversation.

**`prism cmd` never makes a network call.** It appends one line to
`<data>/analytics/hub_spool.jsonl`; the `serve` daemon batches and ships it. This is
deliberate: a tokenizer on that path once cost 0.57s per command, and a PATH shim fronts
every `git`, `ls` and `find` you run. The spool is truncated only after a 2xx, so a hub
outage loses nothing, and each event carries its own `ts` so a late-flushed backlog is
filed under when it happened.

Hub policy (`PrismConfig`, `FilterLimits`, YAML filter rules) merges as the top layer of
the [configuration hierarchy](#configuration-resolution-hierarchy-12-factor-app):
hub-enforced → project `.prismrc` → global `config.yaml` → defaults. A layer wins
wherever it states a value, so policy can switch a feature on as well as off.

**Two features need policy from a licensed hub to run at all:** GraphRAG
(`graph_enabled`, all of `prism graph` and the `prism_graph_*` MCP tools) and the
semantic cache (`cache_enabled`, `prism cache` and the proxy's record/replay legs). Both
default to `false`, and an agent with no hub policy therefore has them off.

Everything else is free and needs no hub, no account and no network: `prism cmd` and its
~110 filters, the PATH shims, `read`, `count`, `compress`, `toon`, `memory`, `gain`,
`mcp --stdio`, and the local `serve` proxy including its prompt-cache invariants, context
editing and image rightsizing.

prism itself contains **no licence check**. It has no key, verifies no signature, and
calls nothing to decide what it may do — it reads `graph_enabled`/`cache_enabled` out of
the resolved configuration and honours them. What a hub will and will not distribute is
the hub's business; prism only ever states which policy it is running under.

Events are typed structs with `#[serde(rename_all = "camelCase")]` rather than
hand-written JSON, and the shape is pinned by `tests/fixtures/hub-events.json`, which the
hub's own test suite validates against. That fixture exists because the previous
hand-built body used snake_case keys the hub read as camelCase: every field missed, every
stored row was zeros, and nothing errored for three months.

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
         │                                         ├── prism_memory_search (3-tier Memory Palace)
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
