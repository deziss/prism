#!/usr/bin/env python3
"""Compare old vs new prism bench results: python3 compare.py results_old.csv results.csv"""
import csv, sys, collections
old = {r['id']: r for r in csv.DictReader(open(sys.argv[1]))}
new = {r['id']: r for r in csv.DictReader(open(sys.argv[2]))}
I = lambda r, k: int(r[k]) if r and r.get(k, '').strip() not in ('', '-') else 0
byt = collections.OrderedDict()
for i, r in new.items(): byt.setdefault(r['type'], []).append(i)
print(f"{'TYPE':<8}{'n':>3}{'RAW':>9}{'RTK':>9}{'OLD prism':>11}{'NEW prism':>11}{'RTK%':>7}{'OLD%':>7}{'NEW%':>7}{'beat rtk':>9}{'worse':>7}")
print('-' * 88)
T = collections.Counter()
for t, ids in byt.items():
    raw = sum(I(new[i], 'raw_tok') for i in ids); k = sum(I(new[i], 'rtk_tok') for i in ids)
    o = sum(I(old.get(i), 'prism_tok') for i in ids); n = sum(I(new[i], 'prism_tok') for i in ids)
    beat = sum(1 for i in ids if I(new[i], 'prism_tok') < I(new[i], 'rtk_tok')); worse = sum(1 for i in ids if I(new[i], 'prism_tok') > I(new[i], 'rtk_tok'))
    pct = lambda x: f"{(raw-x)*100/raw:6.1f}%" if raw else "   n/a"
    print(f"{t:<8}{len(ids):>3}{raw:>9}{k:>9}{o:>11}{n:>11}{pct(k):>7}{pct(o):>7}{pct(n):>7}{beat:>9}{worse:>7}")
    T.update(raw=raw, k=k, o=o, n=n, beat=beat, worse=worse, cnt=len(ids))
raw = T['raw']; pct = lambda x: f"{(raw-x)*100/raw:6.1f}%"
print('-' * 88)
print(f"{'TOTAL':<8}{T['cnt']:>3}{raw:>9}{T['k']:>9}{T['o']:>11}{T['n']:>11}{pct(T['k']):>7}{pct(T['o']):>7}{pct(T['n']):>7}{T['beat']:>9}{T['worse']:>7}")
print("\nNEW PRISM STILL LOSES TO RTK (tokens):")
for i, r in new.items():
    p, k = I(r, 'prism_tok'), I(r, 'rtk_tok')
    if p > k: print(f"  {i} {r['type']:<7} {r['command'][:46]:<47} raw={I(r,'raw_tok'):<7} rtk={k:<6} prism={p:<6} old={I(old.get(i),'prism_tok')}")
print("\nREGRESSIONS (new > old by >20 tokens):")
for i, r in new.items():
    p, o = I(r, 'prism_tok'), I(old.get(i), 'prism_tok')
    if old.get(i) and p > o + 20: print(f"  {i} {r['type']:<7} {r['command'][:46]:<47} old={o:<6} new={p:<6} rtk={I(r,'rtk_tok')}")
print("\nNONZERO PRISM EXIT:")
for i, r in new.items():
    if r['prism_exit'] != '0': print(f"  {i} {r['type']:<7} {r['command'][:46]:<47} prism_exit={r['prism_exit']} rtk_exit={r['rtk_exit']}")
