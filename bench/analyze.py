#!/usr/bin/env python3
"""Per-type summary of a bench results.csv (raw vs rtk vs prism tokens)."""
import csv, sys, collections
rows = list(csv.DictReader(open(sys.argv[1])))
I = lambda r, k: int(r[k]) if r[k].strip() not in ('', '-') else 0
byt = collections.OrderedDict()
for r in rows: byt.setdefault(r['type'], []).append(r)

def blk(name, rs):
    raw = sum(I(r, 'raw_tok') for r in rs); k = sum(I(r, 'rtk_tok') for r in rs); p = sum(I(r, 'prism_tok') for r in rs)
    kw = sum(1 for r in rs if I(r, 'rtk_tok') < I(r, 'prism_tok')); pw = sum(1 for r in rs if I(r, 'prism_tok') < I(r, 'rtk_tok'))
    pct = lambda x: (raw - x) * 100 / raw if raw else 0
    km = sum(I(r, 'rtk_ms') for r in rs) / len(rs); pm = sum(I(r, 'prism_ms') for r in rs) / len(rs)
    return f"{name:<8}{len(rs):>4}{raw:>10}{k:>10}{p:>10}{pct(k):>7.1f}%{pct(p):>7.1f}%{kw:>7}{pw:>7}{len(rs)-kw-pw:>5}{km:>8.0f}{pm:>8.0f}"

print(f"{'TYPE':<8}{'n':>4}{'RAW tok':>10}{'RTK tok':>10}{'PRISM tok':>10}{'RTK%':>8}{'PRISM%':>8}{'RTKwin':>7}{'PRSwin':>7}{'tie':>5}{'RTKms':>8}{'PRSms':>8}")
print('-' * 93)
for t, rs in byt.items(): print(blk(t, rs))
print('-' * 93); print(blk('TOTAL', rows))
big = [r for r in rows if I(r, 'raw_tok') >= 2000]
if big: print(f"\nOutputs >= 2000 raw tok (n={len(big)}):\n" + blk('BIG', big))
print("\nPRISM LOSES TO RTK:")
for r in rows:
    p, k = I(r, 'prism_tok'), I(r, 'rtk_tok')
    if p > k: print(f"  {r['id']} {r['type']:<7} {r['command'][:48]:<49} raw={I(r,'raw_tok'):<7} rtk={k:<6} prism={p}")
print("\nNO-OP OR INFLATED (prism >= raw, raw>0):")
for r in rows:
    if I(r, 'raw_tok') and I(r, 'prism_tok') >= I(r, 'raw_tok'): print(f"  {r['id']} {r['type']:<7} {r['command'][:48]:<49} prism {I(r,'prism_tok')}/{I(r,'raw_tok')}")
print("\nNONZERO WRAPPER EXIT:")
for r in rows:
    if r['rtk_exit'] != '0' or r['prism_exit'] != '0': print(f"  {r['id']} {r['type']:<7} {r['command'][:44]:<45} rtk_exit={r['rtk_exit']} prism_exit={r['prism_exit']}")
