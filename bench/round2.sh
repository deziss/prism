#!/usr/bin/env bash
# prism read --mode skeleton vs rtk read -l aggressive: tokens + fn retention
P=/home/anshukushwaha/95095/Backup/Desktop/learn/prism
PR=${PRISM:-$P/target/debug/prism}; TOK=/home/anshukushwaha/.local/bin/prism; RK=/home/anshukushwaha/.local/bin/rtk
tok(){ [ -s "$1" ] && $TOK count -f "$1" 2>/dev/null|tr -d '[:space:]' || echo 0; }
echo "file,raw_tok,rtk_agg_tok,prism_skel_tok,rtk_pct,prism_pct,raw_fns,rtk_fns,prism_fns"
for f in src/filter/common.rs src/filter/cloud.rs src/proxy.rs src/cli.rs src/mcp.rs src/reader.rs src/analytics.rs src/compress.rs src/memory.rs src/cache.rs src/encode.rs README.md; do
  cd $P; T=$(mktemp -d)
  cat "$f" > $T/raw; $RK read -l aggressive "$f" >$T/rtk 2>&1; $PR read --mode skeleton "$f" >$T/prs 2>&1
  rt=$(tok $T/raw); kt=$(tok $T/rtk); pt=$(tok $T/prs)
  rf=$(grep -cE "^\s*(pub(\([a-z]+\))? )?(async )?(unsafe )?fn " $T/raw); kf=$(grep -cE "(pub(\([a-z]+\))? )?(async )?(unsafe )?fn [A-Za-z_]" $T/rtk); pf=$(grep -cE "(pub(\([a-z]+\))? )?(async )?(unsafe )?fn [A-Za-z_]" $T/prs)
  awk -v f="$f" -v r=$rt -v k=$kt -v p=$pt -v rf=$rf -v kf=$kf -v pf=$pf 'BEGIN{printf "%s,%d,%d,%d,%.1f,%.1f,%d,%d,%d\n",f,r,k,p,(r>0?(r-k)*100/r:0),(r>0?(r-p)*100/r:0),rf,kf,pf}'
  rm -rf $T
done
