# Dead code report — prism

Re-verified 2026-09-14 against `prism 0.5.0`. **Report only: nothing here has been
removed.** Every row was confirmed by call-site search across `src/` including tests;
where a symbol's only references are its own definition and its own test, that is stated.

Read the [false positives](#false-positives--do-not-delete) section before acting on any
row. Several plausible-looking candidates in this codebase are reachable only through
proc-macros or serde, and grep cannot see either.

---

## Confirmed unreachable

| # | Item | Location | ~LOC | Evidence |
|---|---|---|---|---|
| 1 | **TRON encoder** | `src/encode.rs:20-33,72-160,177-222` | ~160 | `tron_encode` has exactly 2 references: its definition and its own test at `:323` |
| 2 | **`SessionState`** | `src/hooks.rs:8-73` | 66 | Nothing constructs it. The 4 references are the struct, its `Default`, its `impl`, and a mention inside a hook template string |
| 3 | **`src/utils.rs`, whole module** | `src/utils.rs:1-61` | 61 | `pub mod utils;` at `src/lib.rs:20` is the only reference; `utils::` appears at zero call sites |
| 4 | **`check_do_not_repeat`** | `src/hooks.rs:383` | 33 | 1 reference: the definition |
| 5 | **`Cache::evict_lru`** | `src/cache.rs:431` | ~20 | 1 reference: the definition. The cache has no eviction path at all — nothing bounds it |
| 6 | **`install_ca_system`** | `src/proxy.rs` | ~30 | Carries `#[allow(dead_code)]` and is uncalled since CA trust became opt-in (`prism init --trust-ca`) |

`SessionState` is the one worth reading before deleting: it is the "71% repeated-read
blocker" described in the hook docs. The feature was designed and never wired up, so
removing it deletes the design too.

## Write-only configuration

`PrismConfig` accepts these keys, `prism config --set-key` writes them, and nothing
reads them back. A user who sets one gets no error and no effect.

| Key | Status |
|---|---|
| `cache_dir` | 0 reads anywhere |
| `llmlingua_path` | 0 reads anywhere |
| `memory_tier` | 0 reads anywhere |
| `log_level` | 0 reads anywhere |
| `mcp_port` | Settable at `src/cli.rs:900`; never read. `prism mcp --port` is a CLI flag and does not consult it |
| `tiktoken_model` | Settable at `src/cli.rs:903`; never read |
| `toon_enabled` | Has an accessor, exercised only by a test (`src/cli.rs:1540`) |
| `tron_enabled` | Has an accessor, exercised only by a test (`src/cli.rs:1541`) |

For contrast, the keys that **do** work: `compression_ratio` (read in `proxy.rs` at
`:2334`, `:2384`, `:2463`), `cache_enabled` (`cache.rs:197`, `proxy.rs:2492`),
`graph_enabled` (`knowledge/mod.rs:56`, `cli.rs:661,667`), and `filter_toggles`
(added in 0.4.0).

This is the highest-value row in the report. The others waste bytes; this one takes a
user's explicit instruction and silently discards it.

## Duplication, not dead code

`cache.rs` carries a private `pseudo_embedding` that is a byte-for-byte equivalent of
`vector::embed`. They must stay in step — an index written by one and read by the other
compares incomparable vectors — which is exactly the kind of invariant that should be a
function call rather than a convention. Documented in the `vector::embed` doc comment.

---

## False positives — do NOT delete

Verified reachable despite looking unused:

- **The 15 `*Request` structs in `src/mcp.rs:228-355`** are consumed by the
  `#[tool_router]` proc-macro. Grep sees no constructor because the macro writes it.
- **`src/image.rs` `*_for_test` helpers** cross a module boundary into `proxy.rs` tests.
- **`Gain*`, `MemoryStats`, `CacheStats`, `HubStatus`** are serde payloads for `--json`.
  Their fields are "written" by serialization only.

## Fixed since the first pass

Listed so the report is not re-derived from a stale copy:

- `hook::uninstall` was dead with zero callers. It is now reachable as
  `prism hook uninstall` and from `prism shim uninstall` (0.4.0).
