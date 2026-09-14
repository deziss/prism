# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0] - 2026-09-13

### Added
- **Opt-in installation.** `prism init` gains `--shims` and `--trust-ca`, both off.
  `install.sh` no longer enables either. A plain install previously put prism in front
  of ~102 commands (`ls`, `grep`, `find`, `env`, `ps`, `systemctl` among them), wrote a
  PATH block into every shell rc file, and injected the MITM CA into every browser and
  Electron trust store — with no prompt.
- **`prism status`** — shims (and whether they actually lead PATH), CA, Claude Code MCP
  registration, hub enrolment, in one place. That question previously had no single
  answer, which mattered because several of those were enabled silently.
- **`prism hook uninstall`** removes the project-local `.prism/hooks/` templates, which
  `hook install` had no counterpart for. Only files carrying PRISM's marker are removed,
  so a hook you edited is left alone.
- **`prism serve --upstream`** now works, making prism usable as a base URL
  (`ANTHROPIC_BASE_URL=http://127.0.0.1:27181`). The flag had existed and been ignored.
- **Per-tool filter toggles** (`filter_toggles`), distributed as hub policy. Additive:
  the sixteen numeric filter caps stay free and local.
- **Dedicated MCP token**, minted at enrolment, so a containerised hub can drive the
  Memory and Graph pages. Separate from the agent token: MCP access must be revocable
  without re-enrolling.
- **`prism gain` rewritten** — human-readable counts, efficiency meter, impact bars, and
  a column rtk structurally cannot have: prism's own overhead (`filter_us`) beside the
  wrapped command's wall clock.
- **48 more filtered tools** (102 → 150), as defaults rather than config examples.
  Four new families: diagnostic linters that share the `path:line:col: message` shape
  (pylint, shellcheck, stylelint, yamllint, actionlint, markdownlint, cppcheck,
  clang-tidy, luacheck, vale, tflint), package managers (apt, dnf, brew, nix, gem,
  bundle, conda and kin), native build drivers (cmake, bazel, meson, buck2, xcodebuild,
  swift, zig) and JS bundlers (vite, webpack, rollup, esbuild, tsup, parcel, turbo, nx,
  lerna) — plus aliases of tools already covered (egrep, fgrep, ag, ack, fdfind, lsd,
  nerdctl, terragrunt, mvnd).

  `ninja`, `clang` and `gcc` are deliberately **not** included: prism filters when
  stdout is a pipe, which is exactly when a build system is consuming it, and those
  emit data as well as diagnostics. Shimming them would corrupt builds.

  These reuse the existing `max_diagnostics` / `list_max_lines` caps rather than adding
  config keys, because a new key has to reach every agent before any hub can push it.
- **The config poll reports the running version** (`?prismVersion=`). `prismVersion` was
  sent at enrolment and never again, so an agent upgraded in place reported its
  enrolment-time version for the life of the hub row — which also meant the fleet's
  version-skew badge compared every agent against a number that never moved.

### Changed
- **`prism shim uninstall` now strips the rc PATH block**, mirroring what
  `shim install --path` writes. It previously deleted only the shim files and printed
  "Also remove the PATH line … if you added one", leaving every shell with a PATH entry
  pointing at an empty directory.
- **Shims fall through to the real tool when the prism binary is missing.** The exec
  target is recorded when the shim is written, so a moved or deleted binary turned `ls`,
  `grep` and `ps` into "No such file or directory" — removing the tools needed to
  diagnose the breakage.
- **`prism serve` binds `127.0.0.1`** by default instead of `0.0.0.0`.
- **`prism init` no longer writes `PRISM_HUB_URL`** to three shell rc files. Nothing in
  prism has ever read it; the hub URL comes from the enrolment credentials.
- **Memory search scores a block's full content**, with BM25/IDF weighting, stemming,
  and a vector re-rank over the turbovec index already in the binary. It previously
  scored exact keyword-set intersection only, and `extract_keywords` drops every token
  of four characters or fewer — so `k8s`, `npm`, `ssh`, `git` and `aws` could never be
  matched even when present in the block's own text.

### Fixed
- **`prism gain` reported zero savings, always.** The byte counts were recorded
  correctly; nothing derived tokens from them at report time.
- **A unit test could write into, and truncate, the live telemetry spool.**
- **`history.json` corrupted itself under parallel use** — a non-atomic
  truncate-then-write on the hot path of every shimmed command.
- **Local models were billed at gpt-4o rates**, inventing spend for free inference.
- **PathJail refused only files that looked secret.** `/etc/shadow`, `/etc/sudoers`,
  `/proc/<pid>/environ`, `~/.ssh/work` (a key with an ordinary name), and prism's own
  `~/.config/prism/hub.json` all passed. Now resolves the path first, then applies
  directory, filename and extension rules. MCP reads get a stricter list again —
  browser profiles, shell history, keyrings, `~/.claude` — because an MCP path is
  chosen by whatever drives the model, not by the user.
- **The MCP server rejected authenticated off-loopback requests** with "Host header is
  not allowed". rmcp's DNS-rebinding protection allows loopback only; the bind address
  is now in `allowed_hosts`. Every unit test passed while this was broken.

### Security
- `prism_read_file` and `prism_filter_cmd` are refused to off-loopback MCP callers. The
  hub needs neither to render a stats page.


## [0.3.0] - 2026-09-12

### Added

- **`prism uninstall`** — audits, or with `--remove` deletes, every trace of PRISM
  on the machine. Read-only by default and exits non-zero while anything remains,
  so "is it actually gone?" is finally a safe question to ask repeatedly. It
  embeds `remove-prism.sh` and `scripts/prism-manifest.sh` with `include_str!` and
  runs them, so the copy it executes is byte-identical to the repository's and it
  works without a checkout present. Deliberately not a reimplementation: a fourth
  removal path with its own idea of what installation created is the bug being
  fixed, not a feature.
- **`prism serve --bind`**, defaulting to `127.0.0.1`.
- **`scripts/prism-manifest.sh`** — one declarative list of every path, port, unit,
  marker, trust entry and environment variable PRISM writes outside its own tree.
  `prism-enable`, `prism-disable` and `remove-prism.sh` all source it and carry no
  private lists.
- **`scripts/prism-env`** — run a single command through the proxy without writing
  anything to the shell, to `environment.d`, or to any config file.
- **`tests/uninstall_symmetry.rs`** — scans the install side for host writes and
  fails the build when a target is not declared in the manifest.

### Changed

- **`prism serve` binds loopback instead of `0.0.0.0`.** It previously bound every
  interface unconditionally, with no flag to narrow it: an intercepting TLS proxy
  holding a CA private key and minting certificates on demand, reachable from the
  whole network. Pass `--bind 0.0.0.0` to restore the old behaviour deliberately;
  a non-loopback bind now logs a warning.
- **`prism init --global` no longer writes the OS trust store.** It used to install
  `/usr/local/share/ca-certificates/prism.crt` and run `update-ca-certificates`,
  and nothing ever removed it — while uninstalling *did* delete the CA private key,
  leaving the machine trusting an interception root nobody could audit. NSS already
  covers the browsers and Electron IDEs that need it, and NSS entries are removable.
  The commands to add and to undo the system anchor are both printed instead.
- **`prism shim` no longer advises setting `env.PATH` in `~/.claude/settings.json`.**
  Claude Code does not expand `${PATH}` there, so following that advice left the
  agent with the shims directory plus a literal string on PATH — and once the shims
  directory was removed, every command became "command not found". No uninstaller
  looked at that file. The help now explains why, and points at launching from a
  shell that already has the shims.
- **`remove-prism.sh` rewritten as an auditor that can also remove.** Removal is now
  opt-in behind `--remove`; `--verbose` lists affected processes individually. Audit
  and removal share one detector, so the closing verdict cannot disagree with
  reality the way the old self-confirming grep did.
- `scripts/prism-env` sets `SSL_CERT_FILE` to `ca-bundle.crt`, not the bare CA.
  Those variables replace the trust store rather than extending it, so the previous
  value left the wrapped command trusting PRISM and nothing else.

- **BREAKING: GraphRAG and the semantic cache are now hub-policy features, off by
  default.** `graph_enabled` and `cache_enabled` default to `false`. An agent with no
  hub policy — which is every agent today — loses `prism graph` (all nine subcommands,
  and the `prism_graph_*` MCP tools) and `prism cache` (including the proxy's record and
  replay legs) on upgrade. A licensed PRISM Hub distributes policy setting them `true`;
  that lands in `<data>/hub-config.yaml` via `prism hub config` and resolves as the top
  configuration layer, so `prism hub enroll --url <hub> --token <join-token>` restores
  them. Nothing else changes: `prism cmd` and its ~110 filters, the PATH shims, `read`,
  `count`, `compress`, `toon`, `memory`, `gain`, `mcp --stdio` and the local `serve`
  proxy — prompt-cache invariants, context editing and image rightsizing included — are
  unaffected, need no hub, and make no network call.
- **prism contains no licence check.** It holds no key, verifies no signature and calls
  nothing to decide what it may do; it reads two booleans out of the resolved
  configuration and honours them. "Community" is simply *no policy saying otherwise*.
  This is the only shape that makes sense for an AGPL-3.0 binary whose source is
  published: an in-binary gate would be both legally removable and pointless, so
  entitlement decisions live entirely in the hub.
- The four feature flags previously had **zero read sites outside `config.rs`** —
  settable, mergeable and completely inert, which meant a hub could distribute flags
  nothing honoured. They are now read at the two chokepoints that front everything:
  `knowledge::find_active_graph` (the mandatory first step of every read-side graph
  operation) and `cache::global_cache` (which every module-level cache helper routes
  through). When the cache is disabled the sled store is never opened, so no directory
  is created and no lock taken.
- **A disabled feature says so, distinguishably.** Both chokepoints return `Option`, so
  a naive gate would make "off" indistinguishable from "no graph indexed" or "cache
  unreachable" — the argument `cache::open_error` already made for a locked store, now
  extended: `GraphUnavailable::{Disabled, NotFound}` and
  `cache::Unavailable::{Disabled, Unopenable}`. `prism cache stats` reports *disabled*
  rather than `Total entries: 0` or a misleading lock error; `prism graph query` says
  the feature is off rather than sending the user to `prism graph index`, which would
  have changed nothing. Every message names the feature, carries the `prism hub enroll`
  command, and lists what still works. `prism serve` logs it once at startup instead of
  relaying silently uncached, and the MCP tools return it as an error so an agent cannot
  read "off" as "nothing found".
- `cache_enabled` sits **above** `PRISM_CACHE_RECORD`, `PRISM_CACHE_SERVE` and
  `PRISM_CACHE_MIN_SIMILARITY`. Those are preferences within a cache that exists — a
  privacy kill switch and a replay opt-in — so none may reach a store policy has not
  enabled, and `PRISM_CACHE_SERVE=always` is not a way to switch the feature on. The
  gate sits on the cache *key*: with no key the serve leg has nothing to look up, the
  record leg nothing to write, and `PRISM_CACHE_MIN_SIMILARITY` (which only ranks inside
  a similarity search) is unreachable. With the cache on, all three behave exactly as
  before.
- `toon_enabled` (on) and `tron_enabled` (off) are unchanged.
- **Toolchain moved to Rust 1.98.1 and pinned.** The build machine's `rustup stable`
  had been left on 1.92.0 (2025-12-08) while upstream stable was 1.98.1 (2026-09-01),
  and neither the repo nor CI pinned anything — so a GitHub runner used current stable
  while local builds used 1.92, and a clippy lint introduced after 1.92 would fail CI
  while passing locally. `rust-toolchain.toml` now pins `channel = "stable"` with
  `rustfmt` and `clippy`, so both resolve identically; CI's manual `rustup component
  add` step became redundant and was dropped. `rust-version = "1.85"` is unchanged and
  unrelated — it is the MSRV floor (edition 2024's minimum), not the toolchain in use.
- Clippy 1.98 surfaced 8 lints its 1.92 counterpart did not, which is exactly the skew
  the pin prevents. Six `unnecessary_sort_by` became `sort_by_key` + `Reverse` after
  confirming every sort key is a `Copy` numeric, so no clone is introduced. The two
  `manual_checked_ops` findings (a lint new in 1.98) were annotated rather than
  rewritten: in `filter/vcs.rs` the `bar == 0` arm is a distinct rendering branch
  ("git drew no bar, so the split is unknown → ±N") rather than a division guard, and
  in `image.rs` a single `n > 0` branch guards four channel divisions in a box-filter
  inner loop, where `checked_div` would add four `Option` checks per destination pixel
  and obscure that an unsampled pixel is meant to stay at its zero default.

### Removed

- **12 dependencies with no reference anywhere in `src/`**, verified by scanning every
  `.rs` file for `<crate>::` and `use <crate>`: `indicatif`, `toml`, `tower`,
  `tower-http`, `hyper`, `hyper-util`, `http-body-util`, `tracing-appender`, `home`,
  `futures`, `uuid`, and the `criterion` dev-dependency (no `benches/` directory and no
  `[[bench]]` target exist). `tower-http` had `cors`/`compression-gzip`/`trace`
  features enabled but was never referenced, so no layer was ever applied and no
  runtime behaviour changes. `[dev-dependencies]` is now empty and gone.
- Cargo.lock: **440 → 374 packages**.

### Fixed

- **Removal missed most of what installation created.** Three independent "off"
  switches — `remove-prism.sh`, `scripts/prism-disable`, and cleanup inside
  `prism init` — each knew a different subset. Every one reported success. What
  survived a "COMPLETE REMOVAL":
  - `prism-mcp.service`, because the remover named `prism-proxy.service` and a
    `prism-bridge.service` that nothing creates.
  - NSS trust entries in `~/.pki/nssdb` and every Firefox profile — handled by
    `prism-disable` and by nothing else.
  - `~/.claude.json` `mcpServers.prism`, pointing at the binary removal deletes,
    leaving a permanently failing MCP server.
  - Two of three `~/.config/Antigravity*` profile directories, because the remover
    targeted one hardcoded literal path and then printed "OK: Antigravity settings
    are clean".
  - `~/.local/bin/prism-{enable,disable,on,off,env}`, each able to re-apply
    everything that had just been removed.
  - The system trust anchor described above.
- **Removal could delete the files a surviving daemon regenerates.** Unit files were
  removed early without verifying, processes killed several steps later, and the
  data directory deleted after that — long enough for a `Restart=always` unit to
  respawn the proxy and recreate the CA. Removal now confirms nothing is running
  before deleting anything it would rebuild.
- **`certutil` missing made NSS cleanup a silent no-op.** It now fails loudly and
  says the CA is still trusted, rather than reporting success.
- `src/guide.rs` claimed `prism-enable` "injects proxy settings into environment.d
  and ~/.bashrc". It does the opposite — it removes stale entries and runs
  `systemctl --user unset-environment` on the proxy variables.

- The 7 dependency specs still on loose carets (`anyhow`, `tokio-util`, `axum`,
  `rustls`, `tokio-rustls`, `rustls-pemfile`, `sled`) are now pinned exactly, matching
  the `=` style used for the rest. All 28 remaining dependencies are at latest stable.
- **Docs corrected against the code.** `TROUBLESHOOT.md`'s BLAS / `cblas_sgemm`
  section described a subsystem that no longer exists — turbovec 1.0 dropped its
  BLAS/CBLAS dependency, which made `build.rs` dead and removed the only native link
  step — and its telemetry section still described the pre-0.2.0
  `/analytics/proxy-event` endpoint; both were rewritten. `COMPARISON.md` still
  advertised `build.rs` BLAS auto-detection and 19 subcommands (now 20, with `hub`).
  `README.md` documented neither `prism hub` nor the MCP stdio transport nor the
  loopback-bind change.

### Note — what no uninstaller can fix

Environment is copied into a process at `exec`. `unset` changes only the calling
shell, and `systemctl --user unset-environment` only affects units started
afterwards. A login session that was poisoned before PRISM was removed keeps
handing dead variables to everything it spawns, which is why removal has appeared
to fail: it succeeded every time, and the damage lived in processes it could not
reach. `prism uninstall` now names those processes, flags dangling paths, and
calls out a poisoned session leader specifically — restarting individual
applications cannot fix that case. Logging out and back in can.

## [0.2.0] - 2026-09-10

This release contains **breaking changes**. Pre-1.0, so the minor version carries them:

- **MCP protocol `2024-11-05` → `2026-07-28`**, server rewritten on the official
  `rmcp` SDK. Clients pinned to the old revision must be updated.
- **`prism mcp` now binds `127.0.0.1` by default** instead of `0.0.0.0`. Reaching it
  from another host requires an explicit `--bind` *and* a bearer token; previously the
  tool server — including `prism_read_file` — was reachable unauthenticated from the
  whole LAN.
- **Telemetry contract replaced.** `POST /analytics/proxy-event` (single event,
  snake_case keys) → `POST /api/ingest/events` (batched, camelCase, bearer-authed,
  disk-spooled). The old body silently landed as all-default rows on the hub.
- **MSRV / edition:** now edition 2024, `rust-version = 1.85`.
- **Removed:** the `graphrag` and `candle` Cargo features (GraphRAG community
  detection is now unconditional rather than feature-gated), the unused `thiserror`
  dependency, and `build.rs` (turbovec 1.0 dropped its BLAS requirement, leaving the
  CBLAS-locating shim dead).

### Added
- **Single source of truth for the version.** `Cargo.toml`'s `package.version` is now
  the only place the version is written; every runtime string derives from it via
  `env!("CARGO_PKG_VERSION")` — the guide banner, both `prism gain`/`discover`
  dashboard headers, the generated VS Code extension manifest, the MCP `serverInfo`,
  the `prism hub enroll` payload's `prismVersion` (which the hub's Fleet view uses to
  flag version skew, so a stale literal there would have silently defeated the
  feature), and the `/health` probes on both the proxy and MCP ports, which now report
  `"version"` so a running daemon self-identifies. Two tests pin the invariant: one
  asserts the generated manifest tracks the crate version, the other scans `src/` and
  fails on any hard-coded occurrence. Filter and reader fixtures are exempt — their
  version strings are captured `cargo`/`npm` output used as parser input, i.e. test
  data rather than this crate's identity.

### Added
- **PRISM Hub wire contract, fixed.** `record_proxy_event` built the outbound telemetry
  body as a hand-written `serde_json::json!` literal with snake_case keys
  (`orig_tokens`, `api_key_hash`, `cost_usd`, ...); the hub reads camelCase with `??`
  fallbacks, so every field has silently defaulted since the feature shipped. `src/hub.rs`
  replaces the literal with a typed `HubEvent` enum (`Proxy`/`Command`/`Cache`/`Session`),
  every variant `#[serde(rename_all = "camelCase")]`, so the naming contract is enforced
  by the type system. `tests/fixtures/hub-events.json`, regenerated by
  `cargo test hub::contract_fixture_matches_camel_case_shape`, is the committed source of
  truth a hub-side schema should validate against.
  - `prism cmd` / shims still never touch the network: events are appended to
    `<data>/analytics/hub_spool.jsonl` (one `write_all` under a mutex). The `serve`
    daemon owns a bounded `mpsc` + background task (`HubSender`) that batches up to 200
    events or every 2s, POSTs `/api/ingest/events` with a bearer agent token, retries
    with backoff, and only truncates the spool after a 2xx.
  - New fields the old event never carried: `cacheServed` (PRISM's own semantic cache
    served the request, no upstream call made) distinct from `cacheHit` (the *upstream
    provider's* prompt cache reported a hit), `imageSavedTokens`, `promptCacheReadTokens`
    / `promptCacheWriteTokens`, and `status`.
- **`prism hub <enroll|status|flush|config|test>`.** Exchanges a join token for a durable
  agent token (`enroll`), reports spool depth (`status`), forces an immediate
  drain-and-send (`flush`), fetches hub-enforced policy into `<data>/hub-config.yaml`
  (`config`), and sends one synthetic event to prove the wire contract end to end
  (`test`, with an optional `--url` to point at a scratch HTTP listener when there is no
  live hub to test against).
- **`--json` on `gain`, `cache stats`, `memory search`/`stats`, and all four `graph`
  subcommands.** Backed by newly-shared, struct-returning functions
  (`analytics::compute_gains`, `memory::search_blocks`/`stats_data`,
  `knowledge::query_graph_data`/`explain_node_data`/`shortest_path_data`/
  `god_nodes_data`) that the human-text printer, `--json`, and the MCP tools of the same
  name all call — previously `mcp.rs` carried private `*_str` duplicates of this exact
  logic.
- **Hub policy as a config layer.** `config::resolve()` folds hub-enforced policy over
  project `.prismrc` over global `config.yaml` over defaults, wired into
  `filter::common::limits()` and the proxy's compression-ratio lookups, which previously
  only ever read the global file.
- **MCP server rewritten on `rmcp` 3.2.0** (the official Rust SDK), targeting protocol
  **2026-07-28**. All 17 tools kept, now backed by the same struct-returning functions
  above. Two transports: `prism mcp --stdio` (new — the transport a local Claude Code /
  same-machine agent should prefer) and `prism mcp --port <p>` (streamable HTTP, for a
  remote/hub client).

### Security
- **MCP server no longer binds `0.0.0.0` unauthenticated by default.** It now binds
  `127.0.0.1` unless `--bind` explicitly asks for something wider, and going wider now
  *requires* a bearer token (`--auth-token`, or the hub agent token from
  `prism hub enroll`) — the server refuses to start off-loopback without one. Previously
  every tool, including `prism_read_file`, was reachable from the whole LAN with no
  authentication at all.

### Fixed
- `config::load_project` (`.prismrc`) no longer silently swallows a YAML parse failure —
  it returned `Option<PrismConfig>` via `if let Ok(...)`, indistinguishable from "no
  project config here". Now `Result<Option<PrismConfig>, String>`; callers decide how
  loud to be, matching how `filter::rules::load_from` already reports a bad rule file.
- `sha2` 0.11's `finalize()` returns digest's new `Array` type instead of the old
  `generic-array` `GenericArray<u8, N>`, which doesn't implement `LowerHex` — hex-encode
  the byte slice by hand in `reader::compute_sha256` instead.
- A repeated-sequence-collapsing regex in `compress.rs`, `` (.{4,})\1{3,} ``, has been a
  silent no-op since it was written: the `regex` crate has never supported
  backreferences, so `Regex::new` always returned `Err` here. Removed.
- `filter/js.rs`: a `.replace(" in ", " in ")` no-op, and an `if`/`else` whose branches
  had become identical, both removed (no behavior change, just dead branching).

### Changed
- **Edition 2021 → 2024**, `rust-version = "1.85"`. Fallout: `gen` is now a reserved
  keyword (renamed a test-local variable), `std::env::set_var`/`remove_var` are `unsafe
  fn` (wrapped the 8 test-only call sites), and a tail-expression temporary-drop-order
  change in `proxy.rs::start_server` (bound a `PathBuf` to a name instead of formatting
  it inline).
- **Dependency modernization**, 10 risk-ordered groups, `cargo check` + `cargo test --lib`
  green after each: trivial patch/minor bumps (tokio, clap, serde, serde_json, regex,
  flate2, uuid, walkdir, futures, hyper, hyper-util, tracing*); dropped the unused
  `thiserror` dependency (manifest said 2.0, lockfile had a stale 1.0.69, imported
  nowhere); cosmetic majors (colored 3.1, indicatif 0.18, criterion 0.8, toml 1.1, dirs
  7); `serde_yaml` → `serde_yaml_ng` (deprecated upstream; drop-in fork, aliased so no
  call site changed); `sha2` 0.11; `petgraph` 0.8; `reqwest` 0.13 (its rustls feature was
  renamed `rustls-tls` → `rustls`) + `tower-http` 0.7 + `webpki-roots` 1.0; `tiktoken-rs`
  0.12; `turbovec` 1.0 (its `IdMapIndex::new`/`add_with_ids` both became fallible; it also
  dropped turbovec's own BLAS/CBLAS dependency entirely — see below); `rcgen` 0.14 (the
  highest-risk bump: `Certificate::from_params` is gone, replaced by
  `CertificateParams::self_signed`/`signed_by(&Issuer)` — rewrote `proxy.rs`'s CA and
  per-domain cert generation, verified against a real TLS handshake through the MITM
  proxy to `api.anthropic.com`), which in turn let the `time = "=0.3.36"` pin (only ever
  needed for an rcgen 0.12 / time coherence conflict) come out.
- **`build.rs` removed.** It existed solely to locate a system BLAS/CBLAS library for
  turbovec 0.2's `ndarray` backend; turbovec 1.0 dropped that dependency entirely (pure
  SIMD now — confirmed via its own `Cargo.lock` entry, which carries no cblas/ndarray/blas
  crate at all). Verified by building with `build.rs` removed: unaffected.
- Removed the `candle` Cargo feature and its `vector::candle_embed` placeholder module —
  no `candle-core`/`candle-nn` dependency has ever existed in the tree, so it could not
  have compiled if enabled.
- Removed the `graphrag` Cargo feature *declaration* — correcting the plan that
  originated this cleanup, it is not dead code: it gates GraphRAG's petgraph-backed
  community detection, called from every graph build/load path. It has been default-on
  since introduction and nothing ever builds without it, so the flag was the only dead
  part; the 8 `#[cfg(feature = "graphrag")]` gates came out, making that path
  unconditional, with the code and behavior otherwise unchanged.
- `cargo clippy --all-targets -- -D warnings`: 122 pre-existing findings → 0 (mechanical
  auto-fixes via `cargo clippy --fix`, plus 17 fixed by hand). `cargo fmt` run repo-wide
  (40 files; the tree had never been formatted consistently before).
- Corrected stale subcommand/MCP-tool counts in `COMPARISON.md` and `AGENTS.md` (14 → 19
  subcommands, 5/6/12 → 17 MCP tools in various places), and `AGENTS.md`'s description of
  `prism-hub` (was "wrapper scripts + compiled binaries"; it is NestJS + React).

### CI
- Added `.github/workflows/ci.yml`: `cargo fmt --check`, `cargo clippy --all-targets -- -D
  warnings`, `cargo test --lib`.
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
