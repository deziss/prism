#!/usr/bin/env python3
"""Fidelity probe: for grep/find/ls/jq/npm/cargo-list/docker tests, every raw entity
(path, path:line, dep name, test name, key, container/image) must appear in the filtered
output or be covered by a [+N more ...] marker. Usage: fidelity.py <bench dir> [out subdir] [csv]"""
import csv, re, sys, os
B = sys.argv[1]; OUT = sys.argv[2] if len(sys.argv) > 2 else 'out'; CSV = sys.argv[3] if len(sys.argv) > 3 else 'results.csv'
rows = list(csv.DictReader(open(os.path.join(B, CSV))))
# announced cuts: `[+N more x]` and find's `dir/ (N entries, collapsed)`
MARK = re.compile(r'\[\+(\d+) more [^\]]*\]|\((\d+) entries, collapsed\)')
def ents(t, cmd, raw):
    if t == 'grep': return set((m.group(1), m.group(2)) for m in re.finditer(r'^([^:\n]+):(\d+):', raw, re.M))
    if t == 'find': return set(os.path.basename(l.rstrip('/')) for l in raw.splitlines() if l.strip())
    if t == 'ls':
        out = set()
        for l in raw.splitlines():
            p = l.split(None, 8)
            if len(p) == 9 and p[0][0] in 'd-l' and len(p[0]) >= 10:
                name = p[8].split(' -> ')[0]
                if name not in ('.', '..'): out.add(name)  # ls always lists these two; they are not entries
        return out
    if t == 'npm' and (' ls' in cmd or ' list' in cmd) and '--json' not in cmd: return set(re.findall(r'([@\w./-]+@\d[\w.+-]*)', raw))
    if t == 'cargo' and '--list' in cmd: return set(re.findall(r'^([\w:]+): test$', raw, re.M))
    if t == 'jq' and '-c' not in cmd and '-r' not in cmd: return set(re.findall(r'^\s*"([^"]+)":', raw, re.M))
    if t == 'docker' and cmd.strip() in ('docker ps', 'docker ps -a', 'docker images'):
        return set(l.split()[0] if 'images' in cmd else l.split()[-1] for l in raw.splitlines()[1:] if l.strip())
    return None
tot = ok = loss = 0
for r in rows:
    i = r['id']; t = r['type']; cmd = r['command']
    try: raw = open(f'{B}/{OUT}/{i}.raw', errors='replace').read(); fil = open(f'{B}/{OUT}/{i}.prism', errors='replace').read()
    except FileNotFoundError: continue
    e = ents(t, cmd, raw)
    if not e: continue
    tot += 1
    def present(x):
        # grep hits are (file, line): the file heads a section, the line is an indented entry
        if isinstance(x, tuple):
            file, line = x
            # the header is a bare line: a path inside a match's *content* is not one
            hdr = re.search(r'^%s$' % re.escape(file), fil, re.M)
            if not hdr: return False
            sect = fil[hdr.end():]
            nxt = re.search(r'^\S', sect[1:], re.M)
            sect = sect[:nxt.start() + 1] if nxt else sect
            return re.search(r'^\s+-?%s:' % re.escape(line), sect, re.M) is not None
        if x in fil: return True
        if t == 'npm' and x.split('@')[0] in fil: return True
        # grouped output factors a shared prefix: `mod::sub: a, b` covers `mod::sub::a`
        if '::' in x:
            head, _, leaf = x.rpartition('::')
            if head in fil and leaf in fil: return True
        return False
    missing = [x for x in e if not present(x)]
    covered = sum(int(a or b) for a, b in MARK.findall(fil))
    status = 'OK' if not missing else ('COVERED' if covered >= len(missing) else 'LOSS')
    ok += status == 'OK'; loss += status == 'LOSS'
    print(f"{i} {t:<7} {cmd[:44]:<45} entities={len(e):<5} missing={len(missing):<5} marker_cover={covered:<6} {status}")
    if status == 'LOSS': print('     e.g.', missing[:5])
print(f"\n{ok}/{tot} fully present, {loss} LOSS rows (must be 0)")
