# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **`prism shim` — PATH interception for every client.** The filters are prism's only
  lever whose savings compound (a 40k tool result cut to 4k is saved again on every later
  turn that re-sends the conversation), and prism's own telemetry over 159.7M input
  tokens shows the proxy side is already near its ceiling: the provider prompt cache
  covers 99.4% of input, wire compression measures 3.7%, and exact-key response caching
  tops out at 0.7%. So the highest-leverage work was not another proxy feature — it was
  making the filters actually run.
  - Rather than one integration per client, `prism shim install` writes a tiny `exec
    prism cmd <tool> "$@"` script per filtered tool into `<data>/shims` and puts that
    directory first on `PATH`. Every client that runs shell commands is covered by the
    same code, including ones that do not exist yet. Measured with no client config:
    `ls -laR src` 1,588 → 456 tokens, `find . -name '*.rs'` 1,291 → 115.
  - **Interactive commands are safe.** `PRISM_SHIM=auto` (default) filters only when
    stdout is a pipe — an agent reading — and passes the real tool through when stdout is
    a terminal, so a human's `git rebase -i`, colours and progress bars are untouched.
    `always` and `off` override.
  - Recursion is prevented by removing the shim directory from the child's `PATH` (plus
    `PRISM_SHIM_DEPTH` as an independent guard). Resolution stays dynamic rather than
    baked in at install time, so `nvm`/`rbenv`/`pyenv` keep choosing the binary.
  - `prism cmd` now inherits stdin, so `echo '{}' | jq .` works through a shim;
    `Command::output()` had been handing children an empty stdin.
  - Shims hard-code the path of the binary that installed them, so installing from
    `target/debug` or `target/release` would let a later `cargo clean` break every
    command on the machine. `shim::is_build_artifact` detects that (including
    cross-compiled `target/<triple>/release`) and prints the correct install order
    instead of failing silently months later.
  - `filter::FILTERED_TOOLS` lists the 102 shimmable commands and a test parses the
    dispatch table out of `filter/mod.rs` to prove the two cannot drift.

- **YAML filter rules (`src/filter/rules.rs`).** `~/.config/prism/filters/*.yaml` can now
  route a tool prism has no Rust filter for, so adding one costs a file instead of a
  release:
  ```yaml
  tool: nomad
  subcommands:
    status:     { shape: table, cap: list_max_lines }
    alloc-logs: { shape: logs }
    run:        { shape: verb-group, verbs: [started, updated] }
  default:      { shape: generic }
  ```
  `shape` names an existing primitive — `table logs json describe tree verb-group dedupe
  tail errors generic raw` — so a rule inherits the fidelity contract rather than
  reimplementing it: `mount` under a `table` rule with `list_max_lines=20` goes 3,167 →
  609 tokens *and* emits `[+47 more rows]` plus the tee path, with no marker code in the
  rules module.
  - YAML declares routing, not parsing. A regex keep/drop language would be more
    expressive and strictly worse: regex can drop a line but cannot **count** what it
    dropped, and every `[+N more …]`, `(×N)` and `(N entries, collapsed)` in this crate
    is arithmetic over parsed structure. Rules cover a tool that prints a shape prism
    already understands; a genuinely new shape (`find`'s ancestor-stack prefix factoring,
    pytest's frame vendoring) still needs Rust.
  - A rule applies only where the built-in dispatch would have fallen through to
    `generic`, so a stray file cannot quietly downgrade a filter that already parses
    properly; `override: true` opts into precedence. Unknown shapes, unknown caps and
    misspelled keys are load errors listed by `prism config --show`, not silent no-ops,
    and one bad file does not disable the rules that parsed.
  - `PRISM_FILTER_RULES_DIR` overrides the search path.
- **The proxy now fills the semantic cache.** `cache_response` had no call sites, so the
  store was always empty and `find_similar` could never return anything however good its
  scoring became. `serve_session` records every complete 200 response under a key built
  from provider, model, system prompt, full message list, tools and sampling.
  - Recording is on by default (`PRISM_CACHE_RECORD=0` disables it) because it cannot
    change what a client receives. Serving is off by default
    (`PRISM_CACHE_SERVE=deterministic|always`) because it replaces a live model call, and
    an LLM response is not a pure function of its request.
  - Never replayed, whatever the switch: a response containing `tool_use`/`tool_calls`
    (replaying one makes the agent re-run a tool against a stale id and corrupts the
    conversation), a body whose framing does not match the request, a non-200 response,
    and a body that outran the sniff buffer — a partial body would replay as a truncated
    answer. Similarity hits remain display-only; the serving path takes exact key matches.

- **Image rightsizing (`src/image.rs`).** Vision payloads are the largest single item in
  a request — one 1600×1200 screenshot bills at ~2500 Anthropic tokens — so the proxy now
  finds every base64 image in a request, prices it against the provider's own rules, and
  shrinks the oversized ones in flight. Handles all three payload shapes: Anthropic
  `source.data`, OpenAI `image_url.url` (`data:` URLs) and Gemini `inline_data`.
  - PNG is decoded, box-filter downscaled and re-encoded (the codec is `flate2`, already
    in the dependency tree — no new build cost). JPEG/WebP/GIF are measured and logged
    but never re-encoded, because re-wrapping bytes we cannot decode is not a resize.
  - `PRISM_IMAGE_MAX_EDGE` (default 1568) trims pixels the provider discards anyway:
    bytes drop, billed tokens do not change. `PRISM_IMAGE_TARGET_EDGE` goes below the
    provider cap and is what actually saves tokens; it is opt-in and every rewrite is
    logged with before/after dimensions, tokens and bytes. `PRISM_IMAGE_MAX_EDGE=0`
    disables image handling entirely.
  - Measured live through the proxy: 2400×1200 PNG → 1568×784, 168,158 → 131,032 bytes
    at the default; with `PRISM_IMAGE_TARGET_EDGE=1024`, 1640 → 700 tokens.

### Fixed
- **`prism cmd` loaded a tokenizer on every invocation.** It counted the filtered output
  with tiktoken purely to fill in a `prism gain` statistic, which measured **0.57s per
  command** — fatal for a PATH shim that fronts every `git`, `ls` and `find` a user runs.
  The history now records output *bytes*, which is free, and `CommandEntry::tokens()`
  approximates at report time (exact values on pre-existing entries are still used).
  `prism cmd git --version` went **0.57s → 0.05s**. Exact counting remains available
  where it is asked for: `prism count`, `prism compress`, the `prism_count_tokens` MCP
  tool.
- **The shell hook never worked.** `hook.rs` defined a `PRISM_HOOK()` function that
  nothing ever called; it dispatched to `prism <tool>`, which is not a subcommand (it is
  `prism cmd <tool>`); it exported `PRISM_DATA_DIR` pointing at the *hooks* directory,
  which relocates the entire store now that the variable is honoured; and `uninstall`
  removed the comment line while leaving the `source` line behind. Replaced with a single
  marked `PATH` block, written to every shell rc that exists (not just the one `$SHELL`
  names, since a user's terminal and IDE often differ) with fish syntax where
  appropriate. Upgrading strips the old broken lines.
- **`PRISM_DATA_DIR` only moved half the store.** `ca_dir`, analytics and the memory
  palace each re-derived `~/.local/share/prism` instead of calling `prism_data_dir()`, so
  relocating the store split it in two. All four now route through one function.
- **A local model reached none of the optimizations.** `detect_provider` claims Ollama on
  loopback, but Ollama speaks plain HTTP and `handle_plain_http` was a byte pipe —
  `tokio::io::copy` in both directions, past compression, telemetry, image rightsizing
  and the cache. `serve_session` is now generic over its client and upstream streams, so
  the plain-HTTP path runs the same loop as an intercepted CONNECT tunnel when the host
  is a recognised provider; unrecognised hosts still get the untouched byte pipe. Making
  the loop generic also made it testable: the session is now exercised end to end over
  in-memory pipes, request in, response out, entry recorded, replay served.
- **`prism cache stats` reported an empty cache when it could not open one.** sled takes
  an exclusive lock on its directory, so the CLI cannot read the store while
  `prism serve` holds it — and `SemanticCache::new` failing left `global_cache()` as
  `None`, which `get_cache_stats` rendered as `Total entries: 0`. An inaccessible store
  and an empty one are different answers. `cache::open_error()` now carries the reason;
  `prism cache` exits non-zero with it, and `prism_cache_lookup` returns an MCP error
  instead of "No cached entries found".
- **The semantic cache keyed entries by prompt text alone.** `key_hash` and `prompt_hash`
  were both `hash(prompt)`, so the same question asked of two models collided on one
  entry and whichever was written second answered for both. `put_keyed` separates the
  exact-lookup key from the text similarity compares: the key covers everything that
  determines the answer, `prompt` stays the part a human would recognise. Entries also
  store their `content_type` and whether they were an SSE stream, so a replay decision
  survives a restart.
- **The semantic cache returned unrelated entries as matches.** `pseudo_embedding` is a
  16-dimension random-sign hash of words and trigrams — not locality sensitive — and
  `find_similar` had no similarity floor, so it always returned its top-N neighbours
  whatever their distance. Asking "show me all containers in docker" returned a cached
  answer about Tokio's scheduler. `find_similar` now re-ranks candidates with
  `prompt_similarity` (word cosine blended with character-trigram cosine) and drops
  anything below `PRISM_CACHE_MIN_SIMILARITY` (default 0.55); hits carry their score, and
  `CacheEntry` stores the prompt text a comparison needs. Rewordings still hit (91% on
  "how do I list **the** docker containers"), unrelated prompts now miss.
- **Filters could cost more than the raw output.** `filter_output` now returns the
  original text whenever reformatting does not pay. Bytes are the only length available
  for free and they are not tokens — regrouping can shed bytes while *adding* tokens,
  because token boundaries fall wherever punctuation and spacing land. A large output
  therefore only has to get shorter; an output under 2048 bytes has to get at least 10%
  shorter, since on a short output there was little to save and the token risk buys
  nothing. `git branch -a`, `jq -c` and `npx tsc --noEmit` (which grew 64 → 71 tokens
  under the plain byte comparison) now pass through byte-identical.
- **`prism cmd` annotated failures it had not compacted**, spending ~30 tokens on a tee
  path for output it passed through verbatim (a 41-token `psql` error became 90). It tees
  and annotates only when something was actually held back.
- **`kubectl` glog storms** — the same API-probe error logged five times with a fresh
  timestamp and pid — now fold to one line with a `(×N)` count.
- **`find` repeated every parent path.** Directory groups are printed relative to the
  deepest already-printed ancestor and indented by depth, so a deep tree pays for each
  prefix once.
- **Enabling the proxy broke unrelated apps (Antigravity IDE, VS Code, Python, curl)**.
  Three independent causes:
  - `SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE` and `CURL_CA_BUNDLE` were set to the bare
    `ca/ca.crt`. Those variables *replace* the trust store rather than extending it, so
    every host PRISM does not intercept (an IDE's auth, telemetry, update and marketplace
    endpoints) failed certificate verification. `proxy::ensure_ca_bundle()` now writes
    `ca/ca-bundle.crt` — the system roots plus the PRISM CA — and both `prism-enable` and
    `prism init --global` point the replacing variables there. `NODE_EXTRA_CA_CERTS` keeps
    the bare cert, which is correct: it adds a CA.
  - Chromium/Electron applications and Firefox read NSS databases (`~/.pki/nssdb`,
    profile `cert9.db`), not the OpenSSL variables and not the system store, so they
    rejected every intercepted host. `proxy::install_ca_nss()` / `remove_ca_nss()` now
    register and remove the CA via `certutil` (needs `libnss3-tools`).
  - `detect_provider()` treated *any* loopback host on *any* port as Ollama, so PRISM
    intercepted local dev servers, IDE helper processes and its own MCP port. Loopback is
    now intercepted only on explicit local-model ports (11434, 1234), configurable with
    `PRISM_LOCAL_AI_PORTS`.

### Added
- `scripts/prism-enable` and `scripts/prism-disable` are now part of the repository and
  installed by `scripts/install.sh`; previously they existed only in `~/.local/bin` and
  were not version controlled. `prism-disable` also removes the CA from NSS.

### Changed
- **Output filters rewritten** (`src/filter.rs` → `src/filter/` — one module per tool
  family plus `common.rs`). Every filter parses its tool's output instead of grepping
  lines, and nothing is dropped silently: each cut emits a `[+N more …]` marker, and
  `prism cmd` tees the raw output to `~/.local/share/prism/tee/` and prints the path
  whenever a marker is present. Caps live in `config::FilterLimits` (`filters:` in
  `config.yaml`, or `PRISM_FILTER_*` environment variables).
- **`prism read` skeletons retain every declaration at any nesting depth.** The Rust
  skeletonizer gated declarations on brace depth ≤ 1, so functions inside `mod tests`,
  nested modules and nested `impl` blocks disappeared without any notice
  (`src/proxy.rs`: 68 functions → 16). Lexer state now survives line boundaries
  (multi-line string literals, lifetimes such as `&'static str`) and the same walker
  drives `--mode map`. Added skeletons for Markdown, JSON, YAML/TOML, shell, Dockerfile
  and Makefile.

## [0.1.0] - 2026-09-05

### Added
- **Anthropic Prompt Cache Invariant Normalization**:
  - Automatic TTL non-increasing order enforcement: automatically detects if a `ttl='1h'` cache breakpoint follows any `ttl='5m'` (or default ephemeral) block across the evaluation sequence (`tools` -> `system` -> `messages`), promoting preceding blocks to `1h` to eliminate Anthropic 400 Bad Request errors.
  - Breakpoint cap enforcement: strictly caps active cache breakpoints at Anthropic's maximum of 4 across tools, system, and messages, safely pruning excess breakpoints.
  - Diagnostic tracing: added structured `warn!` and `info!` logs when breakpoints are capped, promoted, or skipped.
- **Client Cache Strategy Awareness**:
  - Detects if upstream clients or IDE tools (such as Claude Code, Cursor, or custom SDK clients) have already configured their own `cache_control` blocks. PRISM preserves caller strategies without injecting redundant duplicate breakpoints.
- **Systemd User Service Integration**:
  - Packaged and enabled `prism-proxy.service` running on `:27181` with auto-restart policies and journald logging integration.
- **Comprehensive Unit Test Suite**:
  - Added unit test coverage for Anthropic cache TTL promotion, caller cache preservation, and 4-breakpoint limit invariants (48 total tests passing).

### Changed
- **Default Port Migration for Collision Avoidance**:
  - Reallocated default networking ports to high, collision-free ranges across the entire project:
    - Transparent MITM Proxy: `:27181` (formerly 8080/8081)
    - MCP JSON-RPC 2.0 Server: `:27182` (formerly 3003)
    - PRISM Hub & Telemetry Backend: `:27183` (formerly 3002)
  - Updated CLI command flags (`serve`, `mcp`), interactive terminal guide (`prism guide`), shell activation scripts, VS Code extension configuration, and all troubleshooting documentation.
- **Licensing**: Re-licensed the project under the GNU Affero General Public License v3.0 (`AGPL-3.0-only`), updating all metadata, Cargo manifests, and documentation.
- **Benchmark Suite**: Updated benchmark suite runner (`scripts/benchmark.sh`) to dynamically adapt to available sample files and avoid hardcoded document path dependencies.
- **Project Branding**: Standardized project nomenclature and expansion to *PRISM — Prompt Reduction, Indexing & Semantic Memory*.

### Fixed
- **Recursive Proxy Loop & EMFILE Crashes**: Added loopback connection guard in `src/proxy.rs` to block recursive self-referential CONNECT tunnels and plain HTTP calls targeting the proxy port, eliminating infinite file descriptor exhaustion (`os error 24: Too many open files`).
- **Resilient Socket Accept Backoff**: Introduced non-fatal backoff on `EMFILE`/`ENFILE` in the main TCP accept loop in `src/proxy.rs` instead of terminating the server process under temporary socket bursts.
- **Thread-Safe Atomic Telemetry Logging**: Protected `proxy_events.jsonl` writes in `src/analytics.rs` with a synchronization mutex and single atomic buffer flushes, eliminating concurrent torn writes and JSON line corruption.
- **Systemd File Descriptor Limits**: Increased `LimitNOFILE` to `65536` across user service units and generation templates.
- **Anthropic 400 Error in Long Sessions**: Fixed `a ttl='1h' cache_control block must not come after a ttl='5m' cache_control block` triggered during multi-turn agent sessions in Claude Code and VS Code.
- **MITM Proxy Streaming & TLS Interception**: Fixed HTTP CONNECT tunnel socket polling and TLS certificate generation to prevent handshake failures and connection drops during long-lived SSE streams.

### Security & Privacy
- Updated `.gitignore` to prevent tracking private design documents, agent prompt instructions (`AGENTS.md`, `PRISM_PLAN.md`), and local graph indexing exclusion configs (`.graphifyignore`).
