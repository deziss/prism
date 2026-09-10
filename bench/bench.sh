#!/usr/bin/env bash
# rtk vs prism live token-optimization benchmark: 10 types x 10 tests = 100
# Usage: PRISM=<binary under test> bash bench/bench.sh   (default: target/debug/prism)
B="$(cd "$(dirname "$0")" && pwd)"
OUT="$B/out"; rm -rf "$OUT"; mkdir -p "$OUT"
CSV="$B/results.csv"
PRISM=${PRISM:-$B/../target/debug/prism}
TOK=/home/anshukushwaha/.local/bin/prism          # neutral tokenizer (tiktoken cl100k), release build
RTK=/home/anshukushwaha/.local/bin/rtk
LEARN=/home/anshukushwaha/95095/Backup/Desktop/learn
P="$LEARN/prism"
HB="$LEARN/prism-hub/backend"
export PATH="$B/bin:$PATH" GIT_PAGER=cat PAGER=cat
TMO=90

echo "id,type,mode,workdir,command,raw_tok,rtk_tok,prism_tok,raw_bytes,rtk_bytes,prism_bytes,raw_lines,rtk_lines,prism_lines,raw_ms,rtk_ms,prism_ms,rtk_pct,prism_pct,rtk_exit,prism_exit" > "$CSV"

tok() { [ -s "$1" ] && "$TOK" count -f "$1" 2>/dev/null | tr -d '[:space:]' || echo 0; }
pct() { awk -v r="$1" -v f="$2" 'BEGIN{ if(r<=0){print "0.0"} else {printf "%.1f", (r-f)*100.0/r} }'; }

ID=0
run() { # type mode workdir command...
  local type="$1" mode="$2" wd="$3"; shift 3
  ID=$((ID+1))
  local i; i=$(printf "%03d" "$ID")
  local cmdstr="$*"
  local fr="$OUT/${i}.raw" fk="$OUT/${i}.rtk" fp="$OUT/${i}.prism"
  local t0 t1 rms kms pms rex kex pex
  if [ "$mode" = "read" ]; then
    t0=$(date +%s%N); ( cd "$wd" && timeout $TMO cat "$@" ) > "$fr" 2>&1; rex=$?; t1=$(date +%s%N); rms=$(( (t1-t0)/1000000 ))
    t0=$(date +%s%N); ( cd "$wd" && timeout $TMO "$RTK" read "$@" ) > "$fk" 2>&1; kex=$?; t1=$(date +%s%N); kms=$(( (t1-t0)/1000000 ))
    t0=$(date +%s%N); ( cd "$wd" && timeout $TMO "$PRISM" read "$@" ) > "$fp" 2>&1; pex=$?; t1=$(date +%s%N); pms=$(( (t1-t0)/1000000 ))
  else
    t0=$(date +%s%N); ( cd "$wd" && timeout $TMO "$@" ) > "$fr" 2>&1; rex=$?; t1=$(date +%s%N); rms=$(( (t1-t0)/1000000 ))
    t0=$(date +%s%N); ( cd "$wd" && timeout $TMO "$RTK" "$@" ) > "$fk" 2>&1; kex=$?; t1=$(date +%s%N); kms=$(( (t1-t0)/1000000 ))
    t0=$(date +%s%N); ( cd "$wd" && timeout $TMO "$PRISM" cmd "$@" ) > "$fp" 2>&1; pex=$?; t1=$(date +%s%N); pms=$(( (t1-t0)/1000000 ))
  fi
  local rt kt pt; rt=$(tok "$fr"); kt=$(tok "$fk"); pt=$(tok "$fp")
  printf '%s,%s,%s,%s,"%s",%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n' \
    "$i" "$type" "$mode" "$(basename "$wd")" "${cmdstr//\"/}" \
    "$rt" "$kt" "$pt" "$(wc -c < "$fr")" "$(wc -c < "$fk")" "$(wc -c < "$fp")" "$(wc -l < "$fr")" "$(wc -l < "$fk")" "$(wc -l < "$fp")" \
    "$rms" "$kms" "$pms" "$(pct "$rt" "$kt")" "$(pct "$rt" "$pt")" "$kex" "$pex" >> "$CSV"
  echo "[$i] $type: $cmdstr  raw=$rt rtk=$kt prism=$pt" >&2
}

# ── TYPE 1: git ──
run git cmd "$P" git status
run git cmd "$P" git log --oneline -20
run git cmd "$P" git log -5
run git cmd "$P" git log --stat -3
run git cmd "$P" git diff HEAD~3 HEAD
run git cmd "$P" git show HEAD
run git cmd "$P" git branch -a
run git cmd "$P" git log --graph --oneline -15
run git cmd "$P" git diff --stat HEAD~5 HEAD
run git cmd "$P" git status --short --branch
# ── TYPE 2: grep/rg ──
run grep cmd "$P" grep -rn "pub fn" src/
run grep cmd "$P" grep -rn "fn filter" src/
run grep cmd "$P" grep -rn "use " src/
run grep cmd "$P" grep -rn "Result" src/
run grep cmd "$P" grep -rn "async" src/
run grep cmd "$P" grep -rn "String" src/
run grep cmd "$P" grep -rn "impl" src/
run grep cmd "$P" rg -n "serde" src/
run grep cmd "$P" rg -n "match" src/
run grep cmd "$P" rg -n "tokio" src/
# ── TYPE 3: find ──
run find cmd "$P" find . -name "*.rs"
run find cmd "$P" find src -type f
run find cmd "$P" find . -name "*.toml"
run find cmd "$P" find . -name "*.md"
run find cmd "$P" find . -maxdepth 2 -type d
run find cmd "$P" find src -name "*.rs" -size +10k
run find cmd "$P" find target/debug -maxdepth 1 -type f
run find cmd "$P" find . -name "*.json" -not -path "./target/*"
run find cmd "$P" find extensions -type f
run find cmd "$LEARN" find prism-hub -maxdepth 3 -type d
# ── TYPE 4: ls ──
run ls cmd "$P" ls -la
run ls cmd "$P" ls -la src
run ls cmd "$P" ls -la target/debug
run ls cmd "$LEARN" ls -la
run ls cmd "$P" ls -laR src
run ls cmd "$P" ls -la extensions
run ls cmd "$P" ls -la scripts
run ls cmd /usr ls -la /usr/bin
run ls cmd "$HOME" ls -la .cargo/bin
run ls cmd "$HOME" ls -la .claude
# ── TYPE 5: docker ──
run docker cmd "$P" docker ps
run docker cmd "$P" docker ps -a
run docker cmd "$P" docker images
run docker cmd "$P" docker network ls
run docker cmd "$P" docker volume ls
run docker cmd "$P" docker stats --no-stream
run docker cmd "$P" docker logs --tail 100 conman-server
run docker cmd "$P" docker version
run docker cmd "$P" docker info
run docker cmd "$P" docker inspect conman-server
# ── TYPE 6: cargo ──
run cargo cmd "$P" cargo check --offline
run cargo cmd "$P" cargo build --offline
run cargo cmd "$P" cargo test --offline
run cargo cmd "$P" cargo tree
run cargo cmd "$P" cargo tree -d
run cargo cmd "$P" cargo metadata --format-version 1 --no-deps
run cargo cmd "$P" cargo fmt --check
run cargo cmd "$P" cargo test --offline -- --list
run cargo cmd "$P" cargo check --offline --all-targets
run cargo cmd "$P" cargo build --offline --all-targets
# ── TYPE 7: pytest ──
PY="$B/pyproj"
run pytest cmd "$PY" pytest -q
run pytest cmd "$PY" pytest -v
run pytest cmd "$PY" pytest --tb=long
run pytest cmd "$PY" pytest --tb=short
run pytest cmd "$PY" pytest -x
run pytest cmd "$PY" pytest test_a.py
run pytest cmd "$PY" pytest test_b.py -v
run pytest cmd "$PY" pytest --tb=native
run pytest cmd "$PY" pytest -rA
run pytest cmd /home/anshukushwaha/Desktop/learn/podman/app pytest -q
# ── TYPE 8: npm/pnpm ──
run npm cmd "$HB" npm ls --depth=0
run npm cmd "$HB" npm ls --depth=1
run npm cmd "$HB" npm ls --all
run npm cmd "$HB" npm run
run npm cmd "$HB" npm config list
run npm cmd "$HB" npm ls --json --depth=0
run npm cmd "$HB" pnpm list --depth=0
run npm cmd "$HB" pnpm list --depth=1
run npm cmd "$HB" pnpm why typescript
run npm cmd "$HB" npm ls --depth=2
# ── TYPE 9: jq/json ──
run jq cmd "$P" jq . benchmark_results.json
run jq cmd "$P" jq keys benchmark_results.json
run jq cmd "$HB" jq . package.json
run jq cmd "$HB" jq . tsconfig.json
run jq cmd "$HB" jq . nest-cli.json
run jq cmd "$HB" jq .dependencies package.json
run jq cmd "$HB" jq -c . package.json
run jq cmd "$HOME/.claude" jq . settings.json
run jq cmd "$P" jq . graphify-out/manifest.json
run jq cmd "$HB" jq .packages package-lock.json
# ── TYPE 10: read ──
run read read "$P" src/filter/common.rs
run read read "$P" src/proxy.rs
run read read "$P" src/cli.rs
run read read "$P" src/mcp.rs
run read read "$P" src/reader.rs
run read read "$P" src/analytics.rs
run read read "$P" src/compress.rs
run read read "$P" src/memory.rs
run read read "$HB" package.json
run read read "$P" README.md
echo "DONE: $ID tests" >&2
