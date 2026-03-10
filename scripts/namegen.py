#!/usr/bin/env python3
"""
Generate pronounceable 5-7 char candidates NOT covered by the previous run.
Previous run covered: CVCV(4), CVCCV(5), CVCVC(6), diphthong endings.
This run adds: CCVCV(5), CCVCVC(6), CVCVCV(6), targeted 7-char suffix combos.
Target: under 250k candidates total.
"""

import sys
from itertools import product

VOW = set('aeiou')
C   = list('bdfgjklmnprstvwz')   # 16 single consonants, no H/X
V   = list('aeiou')              # 5 vowels

# Onset consonant clusters (natural English)
CC  = ['bl','br','cl','cr','dr','fl','fr','gl','gr','kl','kr',
       'pl','pr','sc','sk','sl','sm','sn','sp','st','sw','tr','tw']

VALID_CC = set(CC) | {
    'nd','nt','nk','rd','rn','rk','rt','ld','lk','lt','mp','ng',
    'ns','rs','ls','st','ts','ks',
}

def ok(w):
    if len(w) < 5 or len(w) > 7: return False
    if 'h' in w or 'x' in w: return False
    vr = 0
    for c in w:
        vr = vr+1 if c in VOW else 0
        if vr >= 3: return False
    for i in range(len(w)-1):
        if w[i] in VOW and w[i+1] in VOW:
            if w[i] == w[i+1]: return False
            if w[i]+w[i+1] in {'iu','ui','uo','oe','ao','oa'}: return False
    for i in range(len(w)-1):
        if w[i] not in VOW and w[i+1] not in VOW:
            if w[i]+w[i+1] not in VALID_CC: return False
    return True

def score(w):
    s = 0
    if w[-1] in VOW:      s += 7
    elif w[-1] in 'lnrm': s += 5
    elif w[-1] in 'sk':   s += 1
    if len(w) == 5: s += 4
    elif len(w) == 6: s += 3
    elif len(w) == 7: s += 1
    cc = sum(1 for i in range(len(w)-1) if w[i] not in VOW and w[i+1] not in VOW)
    s -= cc * 2
    vc = sum(1 for c in w if c in VOW)
    if 0.35 <= vc/len(w) <= 0.65: s += 1
    return s

words = set()

# ── NEW patterns not in previous run ─────────────────────────────────────────

# 5-char CCVCV: cluster onset + CVCV tail (travo, klena, grima, spila, stave)
# 23 CC × 5 V × 16 C × 5 V = 9,200
for cl,v1,c2,v2 in product(CC, V, C, V):
    words.add(cl+v1+c2+v2)

# 6-char CCVCVC: cluster onset + CVCVC (traklon, grevan, spilak)
# 23 CC × 5 V × 16 C × 5 V × 7 coda = 64,400
for cl,v1,c2,v2,k in product(CC, V, C, V, ['l','n','r','m','s','k','t']):
    words.add(cl+v1+c2+v2+k)

# 6-char CCVCCV: cluster onset + CVC + CV (stronga, skelta) — skip, very noisy

# 6-char CVCVCV: 3-syllable smooth (verona, navita, kalima, turova)
# 16 × 5 × 16 × 5 × 16 × 5 = 512,000 — too many alone
# LIMIT: only use C starting with b,d,f,g,k,l,m,n,p,r,s,t,v,w,z (skip j)
# That's 15^3 × 5^3 = 421,875 — still too many
# LIMIT: first C from shorter list (10 most common onset consonants)
C_COMMON = list('bdgklmnrstv')  # 11 chars
for c1,v1,c2,v2,c3,v3 in product(C_COMMON, V, C_COMMON, V, C_COMMON, V):
    words.add(c1+v1+c2+v2+c3+v3)
# 11^3 × 5^3 = 1331 × 125 = 166,375

# 7-char: CVCV root + natural suffix
# Only CVCV roots that scored well (end in vowel = more likely clean)
# That's 16^2 × 5^2 = 6,400 roots × 40 suffixes = 256,000
# LIMIT: use C_COMMON for roots = 11^2 × 5^2 = 3,025 roots × 40 suffixes = 121,000
SUFFIXES_7 = [
    # -C+VC endings (6 chars of suffix = total 7+root) — only pick short suffixes
    'ran','ren','ron','rin',
    'lan','len','lon','lin',
    'van','ven','von','vin',
    'tan','ten','ton','tin',
    'san','sen','son','sin',
    'nar','ner','nor','nir',
    'mar','mer','mor','mir',
    'dar','der','dor','dir',
    'pal','pel','pol','pil',
    'ral','rel','rol','ril',
    'nal','nel','nol','nil',
    'tal','tel','tol','til',
    'sal','sel','sol','sil',
    'val','vel','vol','vil',
    'kar','ker','kor','kir',
    'gar','ger','gor','gir',
]
for c1,v1,c2,v2 in product(C_COMMON, V, C_COMMON, V):
    root = c1+v1+c2+v2
    for suf in SUFFIXES_7:
        w = root + suf
        if len(w) == 7:
            words.add(w)

# ── Apply filter ─────────────────────────────────────────────────────────────
filtered = [w for w in words if ok(w)]
filtered.sort(key=lambda w: (-score(w), len(w), w))

print(f"Generated {len(filtered)} candidates", file=sys.stderr)

with open('~/workspace/observe/scripts/candidates.txt', 'w') as f:
    for w in filtered:
        f.write(w+'\n')

print("Done.", file=sys.stderr)
