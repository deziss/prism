# PRISM vs rtk vs LeanCTX

**Updated 2026-09-14.** PRISM 0.4.0 · rtk 0.45.0 (installed locally) · LeanCTX (public repo).

Every number in the **Measured** sections was produced on this machine on this date and
can be reproduced with the commands given. Everything in the **Claimed** sections comes
from the projects' own material and is labelled as such. The previous revision of this
document mixed the two, asserted capabilities that do not have a caller, and carried a
star count nobody could source — that is the failure mode this split exists to prevent.

---

## 1. Measured: token savings

Medians of 7 runs each, on the prism repo, tokens counted with `prism count`
(cl100k_base). Reproduce with `scripts/benchmark_vs_rtk.py` — note that the committed
harness hard-codes a repo path and needs editing for another checkout.

| Case | Raw tokens | rtk | rtk saved | PRISM | PRISM saved | Winner |
|---|---:|---:|---:|---:|---:|---|
| `git status` | 16 | 13 | 18.8% | **9** | **43.8%** | **PRISM** |
| `git log -n 20` | 10,761 | 1,792 | 83.3% | **687** | **93.6%** | **PRISM** |
| `git diff HEAD~3` | 9,186 | 5,697 | 38.0% | **85** | **99.1%** | **PRISM** |
| `ls -la src` | 889 | 251 | 71.8% | **249** | **72.0%** | PRISM (marginal) |
| `grep -rn 'fn ' src` | 31,230 | 4,562 | 85.4% | **3,859** | **87.6%** | **PRISM** |
| `find src -name '*.rs'` | 215 | **108** | **49.8%** | 110 | 48.8% | rtk (marginal) |

**`git diff` was the standout gap and is now the standout win.** It previously saved
2.0% where rtk saved 67.6%, because hunk compaction trimmed context but never bounded the
*number* of hunks. Past `diff_max_lines` the diff now collapses to a synthesized
`path | +added -removed` stat, announced with a `[+N more …]` marker, with the full diff
still written to the failure tee so nothing is actually lost.

---

## 2. Measured: latency, and where PRISM's time goes

Interleaved, 25 runs each, to cancel drift:

| | median | overhead vs raw |
|---|---:|---:|
| `git status` raw | 2.26 ms | — |
| `prism cmd` filtering | **5.46 ms** | 3.20 ms |
| `rtk git status` | 9.56 ms | 7.30 ms |

**PRISM is now faster per command than rtk.** It was not: at the previous measurement
`prism cmd` took 14.04 ms against rtk's 10.14 ms. Almost all of that was one thing —
`record_command_timed` read `history.json`, appended one entry and rewrote the whole
file, which on a 1.2 MB history cost **7.7 ms per command**, twenty times the cost of the
filter pass it existed to measure. Entries are appended to a journal now and folded in
periodically.

PRISM's own filter costs **365 µs/run** across 4.6K real invocations (`prism gain`), so
filtering was never the expensive part in either direction.

---

## 3. What is actually live

Claims here are backed by a caller, not by a module existing.

| Capability | Status | Evidence |
|---|---|---|
| CLI output filters | **Live — 150 tools** | `filter/mod.rs`; dispatch/list parity enforced by a test |
| MITM proxy (CONNECT, TLS, keep-alive, SSE) | **Live** | `proxy.rs`, verified against multiple providers |
| Prompt compression (BM25, code-safe) | **Live** | `compress.rs` |
| Anthropic cache breakpoints + context editing | **Live** | `proxy.rs::apply_anthropic_caching` |
| File read modes (`prism read`) | **Live — 7 modes** | `reader.rs`, with PathJail + secret-path denial |
| MCP server | **Live — 17 tools** | `mcp.rs`, stdio + Streamable HTTP + bearer auth |
| Semantic cache | **Live** | `cache.rs`, sled + turbovec; populated on this machine |
| CRAG (corrective retrieval) | **Live** | caller at `knowledge/mod.rs:165` |
| GraphRAG indexer | **Live** | `knowledge/graph_rag.rs` — imports resolve to files, communities span modules |
| Memory search | **Live — BM25-led, vector may now retrieve** | `memory.rs` — see §5 |
| Hub fleet control plane | **Live** | `prism-hub`, three channels + RBAC + billing |
| TRON renderer | **Live, opt-in** | `prism toon tron`, gated on `tron_enabled` (default off) |

TRON was dead when the previous revision called it a moat — definition and its own test,
no caller, and `prism config --set-key tron_enabled` did not even accept the key. It is
reachable now as `prism toon tron`, deliberately opt-in: it is a *rendering* with no
decoder, so it should not be something a caller reaches when it wanted data it can read
back.

---

## 4. Feature matrix

Counts for rtk and LeanCTX are from their own public material (retrieved 2026-09-14) and
are not independently verified.

| Dimension | rtk 0.45 | LeanCTX | PRISM 0.4.0 |
|---|---|---|---|
| Stars | ~51k+ | ~2.4k | new |
| Language | Rust | Rust | Rust |
| Command coverage | 100+ | 60+ shell patterns | **150** |
| MCP tools | ❌ | **76** (README; older docs say 63–67) | 17 |
| File read modes | ❌ | **10** | 7 |
| Code graph | ❌ | property graph, 18 langs via AST | import/symbol graph, **tree-sitter (6 langs)** + matcher fallback |
| Memory | ❌ | session memory + graph | Memory Palace (3 layers) |
| Vector index | ❌ | embeddings + RRF | turbovec ANN, 256-dim sketch or **optional static model** |
| MITM proxy | HTTP proxy mode | ❌ | **CONNECT + TLS + SSE relay** |
| Fleet control plane | ❌ | ❌ | **prism-hub** |
| Editor integrations | Claude Code, Cursor, Copilot | 6+ editors | VS Code |
| Default install | opt-in | shell hook | **opt-in since 0.4.0** |

---

## 5. Honest assessment of the retrieval stack

This is where the previous revision was most misleading, so it gets its own section —
including a correction to that revision.

**The vector index is a feature-hashing sketch, not a learned embedding, and it was too
narrow to retrieve.** At 16 dimensions the signed sum carried enough variance that
unrelated text reached the same similarities as genuine paraphrase, so vector-only
retrieval was switched off entirely. Widening to 256 separated them
(`vector::embed_tests`, which runs on every `cargo test`, so these cannot go stale):

| | unrelated p90 | unrelated p99 | paraphrase p10 |
|---|---:|---:|---:|
| `TURBO_DIM = 16` | 0.459 | 0.664 | 0.876 |
| `TURBO_DIM = 256` | 0.297 | **0.411** | **0.904** |

The tail now falls below the paraphrase body, so `vector_only_min` defaults to 0.75 and
the vector half may retrieve a block the lexical half found no evidence for.

**A real embedding model is now optional but supported.** `model2vec-rs` is built
`local-only` — the `hf-hub` and `ureq` features are off, so prism cannot fetch a model at
runtime. Install one in `<data>/models/static/` and `embed` routes through it; install
nothing and the sketch runs as before. Measured with potion-base-8M on the same corpus:

| | unrelated p99 | paraphrase p10 | margin |
|---|---:|---:|---:|
| sketch | 0.411 | 0.904 | 0.493 |
| static model | **0.382** | **0.958** | **0.576** |

Per-pair the result is mixed and worth stating plainly: `postgres connection pool
exhausted` vs `pg client limit reached` improves 0.068 → **0.352**, `container image pull
failure` vs `docker registry fetch error` 0.066 → **0.293**, but `kubectl context
switching` vs `k8s namespace selection` **regresses** 0.244 → 0.210, because `k8s` is a
token the distillation barely saw. The test asserts the separation margin rather than any
one pair.

Both backends emit `TURBO_DIM`-wide vectors, so a width check cannot tell them apart.
Derived indexes key off `vector::backend_id()` instead — folded into the memory
fingerprint, stored on each cache entry — so installing or removing a model re-embeds
rather than silently mixing projections.

**Correction to the previous revision: it claimed "nothing computes communities". That
was wrong** — `detect_communities` existed and ran. The accurate criticism was narrower,
and has been fixed: it was connected components over *outgoing* edges only, so on a
file→symbol graph it recovered each file's own symbols and told you nothing the `path`
field did not. Indexing prism's own `src/` gave 42 communities for 42 files.

The deeper problem was underneath it: **imports were never edges**. An import became a
leaf node hanging off the file that declared it, so the graph had no cross-file structure
at all — a "cross-file dependency graph" that was really a set of per-file symbol lists.
Imports now resolve to file nodes by suffix, and communities come from label propagation
over the undirected file graph with symbols inheriting their file's label.

| | before | after |
|---|---:|---:|
| file→file import edges | 1 | **33** |
| communities | 42 (one per file) | **24** |
| largest community | 1 file | **16 files** (all of `filter/*`) |

**Symbol extraction was prefix matching and is now a parser.** `starts_with("pub fn ")`
and five more prefixes missed `pub(crate) fn`, `async fn`, `pub const fn`, `extern "C"
fn`, Python `async def`, Go methods with a receiver, `export default function` and arrow
bindings — and it indexed `// fn commented_out()` as a real symbol, which at query time
is indistinguishable from a real one. Measured on prism's own `src/`: 1,168 → 1,330
function nodes, with comment and docstring state now tracked across lines.

**Extraction is a real parse now.** tree-sitter grammars for Rust, Python, JavaScript,
TypeScript, TSX and Go are compiled in; the hand-written matcher remains the fallback for
languages without one (C, Ruby, Java, PHP, shell). On prism's own `src/`: 1,168 functions
under the original prefix rules, 1,330 under the hand parser, **1,388** under
tree-sitter — and 61 of 62 Rust signatures that open their parameter list on the next
line, which a line matcher cannot see at all.

## 6. Where each tool wins

**Choose rtk** for zero-config CLI compression with the lowest per-command overhead, a
large user base, and the best large-diff summarisation.

**Choose LeanCTX** for depth of context tooling: real AST parsing across many languages,
10 read modes, the largest MCP surface, and multi-agent coordination.

**Choose PRISM** when you need what neither offers: **HTTPS interception of the model
API itself** (not just CLI output), and **fleet management** — telemetry ingest, policy
distribution and licence-gated controls across many machines, with team-scoped RBAC.

They compose: rtk or PRISM on the CLI, PRISM on the wire.

---

## 7. Backlog

Everything the previous revision listed as High or Medium has been done, with the
measurement that motivated it and the one that closed it:

| Item | Then | Now |
|---|---|---|
| `git diff` summary mode | 2.0% vs rtk's 67.6% | **99.1%**, announced, raw kept in the tee |
| Async per-command bookkeeping | 7.7 ms rewriting `history.json` | append-only journal; 14.04 ms → **5.46 ms** |
| Replace the 16-dim sketch | unrelated p99 0.664, could not retrieve | 256-dim, p99 **0.411**, retrieval enabled at 0.75 |
| Better symbol extraction | prefix matching, indexed comments | declaration parser, +14% symbols, comment-aware |
| Decide TRON's fate | dead code sold as a moat | reachable as `prism toon tron`, opt-in |
| Collapse `pseudo_embedding` | byte-identical copy of `vector::embed` | delegates to it; the method name is kept |
| tree-sitter extraction | blocked on disk | 6 grammars compiled in; 1,330 → **1,388** symbols |
| Real embeddings | sketch had no semantics | optional `local-only` static model; margin 0.493 → **0.576** |

Still open:

| Priority | Item | Why |
|---|---|---|
| **Medium** | Ship or document a default model | The model path works but the operator must supply the file; prism deliberately will not download one |
| **Low** | Import resolution beyond suffix matching | No build system means `mod.rs` and re-exports are heuristic |
| **Low** | Grammars beyond the six compiled in | C, Ruby, Java, PHP and shell still use the line matcher |

---

*Method: single machine (Intel i7-1355U, 12 threads, Linux 7.0.0), medians of 7 runs for
tokens and 25 interleaved runs for latency, warm page cache, prism 0.4.0 release build,
rtk 0.45.0. Star counts retrieved 2026-09-14 from public listings and are approximate.*
