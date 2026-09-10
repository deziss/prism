#!/usr/bin/env bash
# Round 3: live bench of the NEW / extra filters (tools available locally). Same CSV schema as bench.sh.
B="$(cd "$(dirname "$0")" && pwd)"
OUT="$B/out2"; rm -rf "$OUT"; mkdir -p "$OUT"
CSV="$B/results2.csv"
PRISM=${PRISM:-$B/../target/debug/prism}
TOK=/home/anshukushwaha/.local/bin/prism
RTK=/home/anshukushwaha/.local/bin/rtk
LEARN=/home/anshukushwaha/95095/Backup/Desktop/learn
P="$LEARN/prism"; HB="$LEARN/prism-hub/backend"; TA="$LEARN/tasch"; NX="$LEARN/nexasaas"; TF="$LEARN/jitsi/terraform"; LS="$LEARN/local_stack"; AT="$LEARN/ai-tester"
export PATH="$B/bin:$PATH" MANPAGER=cat PAGER=cat GIT_PAGER=cat
TMO=120
echo "id,type,mode,workdir,command,raw_tok,rtk_tok,prism_tok,raw_bytes,rtk_bytes,prism_bytes,raw_lines,rtk_lines,prism_lines,raw_ms,rtk_ms,prism_ms,rtk_pct,prism_pct,rtk_exit,prism_exit" > "$CSV"
tok() { [ -s "$1" ] && "$TOK" count -f "$1" 2>/dev/null | tr -d '[:space:]' || echo 0; }
pct() { awk -v r="$1" -v f="$2" 'BEGIN{ if(r<=0){print "0.0"} else {printf "%.1f", (r-f)*100.0/r} }'; }
ID=100
run() { local type="$1" wd="$2"; shift 2; ID=$((ID+1)); local i; i=$(printf "%03d" "$ID"); local cmdstr="$*"
  local fr="$OUT/${i}.raw" fk="$OUT/${i}.rtk" fp="$OUT/${i}.prism" t0 t1 rms kms pms rex kex pex
  t0=$(date +%s%N); ( cd "$wd" && timeout $TMO "$@" ) > "$fr" 2>&1; rex=$?; t1=$(date +%s%N); rms=$(( (t1-t0)/1000000 ))
  t0=$(date +%s%N); ( cd "$wd" && timeout $TMO "$RTK" "$@" ) > "$fk" 2>&1; kex=$?; t1=$(date +%s%N); kms=$(( (t1-t0)/1000000 ))
  t0=$(date +%s%N); ( cd "$wd" && timeout $TMO "$PRISM" cmd "$@" ) > "$fp" 2>&1; pex=$?; t1=$(date +%s%N); pms=$(( (t1-t0)/1000000 ))
  local rt kt pt; rt=$(tok "$fr"); kt=$(tok "$fk"); pt=$(tok "$fp")
  printf '%s,%s,%s,%s,"%s",%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n' "$i" "$type" cmd "$(basename "$wd")" "${cmdstr//\"/}" \
    "$rt" "$kt" "$pt" "$(wc -c < "$fr")" "$(wc -c < "$fk")" "$(wc -c < "$fp")" "$(wc -l < "$fr")" "$(wc -l < "$fk")" "$(wc -l < "$fp")" \
    "$rms" "$kms" "$pms" "$(pct "$rt" "$kt")" "$(pct "$rt" "$pt")" "$kex" "$pex" >> "$CSV"
  echo "[$i] $type: $cmdstr  raw=$rt rtk=$kt prism=$pt" >&2; }
# git extras
run git "$P" git stash list
run git "$P" git remote -v
run git "$P" git blame -L 1,40 src/main.rs
run git "$P" git show --stat HEAD
run git "$P" git diff HEAD~1 -- src/proxy.rs
run git "$P" git shortlog -sn
run git "$P" git reflog -10
run git "$P" git worktree list
run git "$P" git log --format=%h_%s -10
run git "$P" git status -sb
# gh
run gh "$P" gh repo view deziss/prism
run gh "$P" gh pr list -R rtk-ai/rtk --limit 20
run gh "$P" gh issue list -R rtk-ai/rtk --limit 30
run gh "$P" gh api repos/rtk-ai/rtk
run gh "$P" gh run list -R rtk-ai/rtk --limit 10
run gh "$P" gh release list -R rtk-ai/rtk --limit 10
# cargo extras
run cargo "$P" cargo metadata --format-version 1
run cargo "$P" cargo tree --depth 1
run cargo "$P" cargo --list
run cargo "$P" cargo clippy --offline
run cargo "$P" cargo doc --no-deps --offline
# go (tasch, small)
run go "$TA" go list ./...
run go "$TA" go vet ./...
run go "$TA" go build ./...
run go "$TA" go env
run go "$TA" go mod graph
run go "$TA" go version
run go "$TA" go test ./... -run NONE
# make
run make "$TA" make -n build
run make "$TA" make fmt-check
run make "$TA" make vet
# node/npx (prism-hub backend)
run npx "$HB" npx tsc --noEmit -p .
run npx "$HB" npx eslint src --max-warnings 0
run npx "$HB" npx jest --listTests
run npx "$HB" npx prettier --check src
run npx "$HB" npm pkg get scripts
run npx "$HB" npx prisma --version
# php/composer (nexasaas)
run php "$NX" composer show --direct
run php "$NX" composer validate --no-check-publish
run php "$NX" php -v
run php "$NX" php -m
run php "$NX" vendor/bin/phpstan --version
run php "$NX" vendor/bin/phpunit --list-tests
# terraform (jitsi/terraform, no init)
run tf "$TF" terraform fmt -check -diff
run tf "$TF" terraform version
run tf "$TF" terraform validate
run tf "$TF" terraform providers
# kubectl/helm (no cluster → error paths)
run k8s "$P" kubectl version --client
run k8s "$P" kubectl get pods -A
run k8s "$P" helm version
run k8s "$P" helm list -A
run k8s "$P" helm env
# psql
run db "$P" psql --version
run db "$P" psql -h 127.0.0.1 -p 5432 -U postgres -c "\l"
# jq/tree
run tree "$P" tree src
run tree "$P" tree -L 1 .
run tree "$P" tree -d extensions
run tree "$HOME" tree -L 1 .claude
run jq "$P" jq -r .[].benchmark benchmark_results.json
run jq "$HB" jq .scripts package.json
# curl/wget/ping
run net "$P" curl -s http://127.0.0.1:27182/health
run net "$P" curl -sI https://example.com
run net "$P" curl -s https://api.github.com/repos/rtk-ai/rtk
run net "$P" curl -sv http://127.0.0.1:27182/health
run net "$P" curl -s https://example.com
run net "$P" curl -s -X POST http://127.0.0.1:27182/ -H content-type:application/json -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
run net "$P" wget -q -O - http://127.0.0.1:27182/health
run net /tmp wget -O /tmp/prism_bench_example.html https://example.com
run net "$P" wget --spider https://example.com
run net "$P" ping -c 3 127.0.0.1
run net "$P" ping -c 2 -W 1 8.8.8.8
# system
run sys "$P" ps aux
run sys "$P" ps -ef
run sys "$P" ps -eo pid,ppid,rss,cmd --sort=-rss
run sys "$P" ss -tulpn
run sys "$P" ss -tan
run sys "$P" df -h
run sys "$P" df -i
run sys "$LEARN" du -h --max-depth=1 prism
run sys "$P" free -h
run sys "$P" free -m
run sys "$P" systemctl --user list-units --type=service --no-pager
run sys "$P" systemctl status --no-pager docker
run sys "$P" systemctl list-units --type=service --state=running --no-pager
run sys "$P" journalctl --user -n 100 --no-pager
run sys "$P" journalctl -n 50 --no-pager
run sys "$P" man -P cat ls
run sys "$P" man -P cat grep
run sys "$P" env
run sys "$P" printenv
# docker extras
run docker "$P" docker compose ls
run docker "$P" docker system df
run docker "$P" docker network inspect bridge
run docker "$P" docker top conman-server
run docker "$P" docker port conman-server
run docker "$LS" docker compose -f docker-compose.yml ps -a
run docker "$LS" docker compose -f docker-compose.yml config
run docker "$P" docker image history conman-server:latest
run docker "$P" podman ps -a
# python pkg tools (venv binaries → basename dispatch)
run pip "$AT" .venv/bin/pip list
run pip "$AT" .venv/bin/pip show requests
run pip "$AT" uv pip list --python .venv/bin/python
run pip "$AT" uv --version
echo "DONE: $((ID-100)) tests" >&2
