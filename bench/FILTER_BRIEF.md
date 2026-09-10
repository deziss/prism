# PRISM filter rewrite — shared brief for implementation agents

Repo: /home/anshukushwaha/95095/Backup/Desktop/learn/prism (Rust 2021, crate `prism`).
Goal: make `prism cmd <tool> …` the best token filter for every command — fewer tokens than rtk
(https://github.com/rtk-ai/rtk, v0.45 installed at ~/.local/bin/rtk) AND higher fidelity.

## Why we're doing this (benchmark findings, 100 live tests, tiktoken cl100k)
Overall prism 65.5% vs rtk 45.4% saved, BUT prism's lead came from silent data destruction:
- filter_npm `.take(30)`: 1495 lines → 30, no pointer.
- filter_find dropped any line containing target/node_modules/.git/dist → `find target/debug …` returned EMPTY (reported as "100% saved").
- filter_ls kept only month + name (dropped perms, size, symlink target).
- filter_grep was a stub (strip ANSI only) → 0% on every grep.
- git: status 4% (rtk 48%), log -5 2.6% (rtk 63%), show 0%, diff 3.6%.
- docker ps 14% (rtk 44%), images 16% (rtk 71%), volume ls 0%, inspect 0%.
- cargo test 88% (rtk 98%: `cargo test: 49 passed (3 suites, 0.80s)`), cargo tree 0%, fmt --check 0%, test --list 10%.
- jq: 0% on most (rtk 33-90%).
- Most filters are line-greps (`l.contains("error")`) with hard-coded `take(N)` caps — replace them with real parsers.

## Fidelity contract (non-negotiable)
1. NEVER drop information silently. Anything cut is announced with `common::more(n, "what")`
   → `[+N more files]`. `prism cmd` detects that marker, tees the raw output to disk and appends
   `↳ full output: <path>`. Only marker-announced cuts are allowed.
2. Caps come from `common::limits()` (struct `FilterLimits` in src/config.rs: grep_max_per_file=25,
   grep_max_results=200, grep_line_width=160, find_max_per_dir=40, find_max_dirs=80, ls_max_entries=200,
   list_max_lines=200, json_max_lines=300, json_max_array=50, log_tail=100, passthrough_max_lines=400,
   diff_context=2, status_max_files=30, test_max_failures=10, max_diagnostics=40). Never hard-code `take(N)`.
3. Structure compression is preferred over deletion: group by file/dir, factor common prefixes,
   drop decoration (box-drawing, separators, repeated headers, `(use "git …")` hints), collapse
   repeated lines with `(×N)`, replace verbose tables with `key: value` or `a|b|c` rows.
4. Errors/failures/warnings are ALWAYS shown in full (up to `test_max_failures` / `max_diagnostics`,
   then a marker). Success paths collapse to one line with counts.
5. Output must stay useful to an LLM agent: keep file paths, line numbers, identifiers, exit reasons.
   When you drop columns (e.g. ls owner/group, docker CREATED), they must be low-value to an agent.
6. Never panic on odd input: empty output, ANSI, unicode, tabs, Windows CRLF, huge lines, missing
   columns, headers absent (e.g. `docker ps --format`). Use `.get()`, `char_indices`, `saturating_sub`.
7. Unknown subcommands of your tool → `generic(output)` (from common), not `output.to_string()`.

## Shared helpers — src/filter/common.rs (read it; do NOT edit it)
`more`, `has_truncation`, `cap_lines`, `cap_vec`, `tail_lines`, `strip_ansi` (already applied before
dispatch), `truncate` (char-safe, `…`), `collapse_blank`, `dedupe_consecutive` (`line  (×N)`),
`squeeze_ws`, `flatten_tree` (box-drawing tree → 1 space per level), `human_size`, `parse_size`,
`find_subcommand`, `has_flag`, `flag_value`, `parse_json` (doc or stream), `compact_json`
(YAML-like: unquoted keys, 1-space indent, inline scalar arrays, uniform object arrays as `k1|k2`
tables, announced caps), `compact_json_output(output, fallback)`, `scalar_str`, `generic`, `limits()`.
If you need a helper that's missing, write it PRIVATE in your own module and list it in your
report under `wanted_common_helpers` — never edit common.rs or mod.rs.

## Ownership & rules
- You own exactly ONE file under src/filter/ (or src/reader.rs). Edit nothing else.
  Other agents are editing sibling files concurrently.
- Dispatch (src/filter/mod.rs) is fixed: every `pub(crate) fn filter_*` your module exports is already
  wired, including stubs marked `// TODO(agent): implement`. Keep every existing function name and
  signature (`pub(crate) fn filter_x(args: &[&str], output: &str) -> String` or `(output: &str)`).
  You may add private helpers freely. Remove `#[allow(unused_variables)]` from stubs you implement.
- `cargo check` and `cargo test --lib filter::<yourmodule>` are the only cargo commands to run
  (no `cargo build`, no `--release`). cargo may block on the target-dir lock — wait.
  If compile errors point at a file you don't own, another agent is mid-edit: wait ~30s, retry.
  Do not "fix" their file.
- WRITE YOUR FILE EARLY AND OFTEN. A previous run of this task was killed mid-way and every agent that
  had not yet written its file lost all its work. Write a first compiling version within your first
  few steps, then iterate.
- Tests: add `#[cfg(test)] mod tests` in your file with realistic fixtures (inline &str). For every
  filter: (a) a golden test asserting the compact shape, (b) a fidelity test — every path/id/name
  from the fixture appears in the output OR is covered by a `[+N more …]` count, (c) an empty-input
  test, (d) a failure/error-path test where applicable. Real raw outputs to base fixtures on are in
  /home/anshukushwaha/95095/Backup/Desktop/learn/prism/bench/out/NNN.raw (index: bench/results.csv →
  id,type,mode,workdir,command,…; ids 001-010 git, 011-020 grep, 021-030 find, 031-040 ls, 041-050 docker,
  051-060 cargo, 061-070 pytest, 071-080 npm, 081-090 jq, 091-100 read). They are being (re)generated
  by bench/bench.sh while you start — if a file is missing, continue from your own knowledge of the tool.
- Token budget yardstick: cl100k ≈ 1 token per 3-4 ASCII chars; box-drawing chars, `│`, `├──`, long
  hashes, timestamps, and repeated column headers are expensive; short ASCII identifiers are cheap.
  Leading indentation is cheap (a run of spaces is one token).
- Comment density: match the existing code (short `//` notes where the parsing rule isn't obvious).
- Do not touch Cargo.toml (no new deps). regex, serde_json, serde_yaml, chrono are already available.

## rtk reference formats (beat these on tokens, match or exceed on fidelity)
- `rtk git status` → `* main...origin/main\nclean — nothing to commit` ; dirty → `* main` + ` M path` lines.
- `rtk git log -5` → one line per commit `5b641e3 subject (3 days ago) <Author>`.
- `rtk git show HEAD` → header line, stat lines, then per file `path` + indented `@@` hunks with +/- lines.
- `rtk grep` → `83 matches in 1 files:` then `path:line:content` lines, 25/file cap, then
  `+58 more in src/filter.rs [see remaining: tail -n +26 <tee>]`, lines truncated at 115 chars.
- `rtk ls -la src` → `600  cli.rs  18.3K` (octal perms, name, size; dirs `775  knowledge/`).
- `rtk docker ps -a` → `[docker] 2 running:` / `  id name (image) Up 53 minutes (healthy) [5173]` / `[docker] 10 stopped/exited:` …
- `rtk docker images` → `[docker] 92 images (55.5GB)` then `  repo:tag [size]`.
- `rtk cargo test` (all green) → `cargo test: 49 passed (3 suites, 0.80s)`.
- `rtk cargo check` (clean) → `cargo check (0 crates compiled)\nFinished dev profile … in 0.43s`.
- `rtk npm ls` → passthrough (0%). `rtk pytest` → failures only + summary.

## Reporting (return as structured output)
summary of what each filter now does, tests added (count, all passing?), wanted_common_helpers,
any behavior you deliberately left lossy (must be marker-announced) and why, open risks.
