#!/usr/bin/env python3
"""
Generate pronounceable 2-syllable word candidates.
Focuses on patterns that produce good-sounding short tech names.
Outputs to candidates.txt for domain checking.
"""

import sys
from itertools import product

# Onset consonants — no H, no X
C = list('bdfgjklmnprstvwz')
# Coda consonants (syllable endings that feel clean)
K = list('bdklmnrst')
# Simple vowels
V = list('aeiou')

# Scoring: some consonant-vowel combos feel better than others for tech names
# Penalize harsh-sounding clusters, reward balanced ones
GOOD_STARTS = set('bdfgjklmnprstvwz')  # all fine
SONORANT = set('lmnr')  # soft endings — preferred
STOP = set('bdgkpt')
FRICATIVE = set('fsvzs')

def score(w):
    """Higher = more likely to sound like a good tech name."""
    s = 0
    # Prefer ending in vowel or sonorant (soft endings like Figma, Versa, Kilo)
    if w[-1] in 'aeiou':
        s += 3
    elif w[-1] in SONORANT:
        s += 2
    # Prefer starting consonant that's a stop or fricative (punchy)
    if w[0] in STOP | FRICATIVE:
        s += 1
    # Prefer 4-5 char names (punchy)
    if len(w) == 4:
        s += 2
    elif len(w) == 5:
        s += 1
    # Penalize back-to-back stops (e.g. "kpali" sounds bad)
    for i in range(len(w)-1):
        if w[i] in STOP and w[i+1] in STOP:
            s -= 2
    # Prefer consonant variety (not same consonant repeated)
    if len(set(c for c in w if c not in 'aeiou')) < 2:
        s -= 1
    return s

candidates = set()

# Pattern 1: CV·CV  (e.g. navi, kelo, mura — 4 chars, cleanest)
for c1, v1, c2, v2 in product(C, V, C, V):
    candidates.add(c1 + v1 + c2 + v2)

# Pattern 2: CVC·CV  (e.g. kanta, versa, solba — 5 chars)
for c1, v1, k, c2, v2 in product(C, V, K, C, V):
    candidates.add(c1 + v1 + k + c2 + v2)

# Pattern 3: CV·CVC  (e.g. kalon, verin, noral — 6 chars)
for c1, v1, c2, v2, k in product(C, V, C, V, K):
    w = c1 + v1 + c2 + v2 + k
    if len(w) == 6:
        candidates.add(w)

# Pattern 4: CV·CAW / CV·CAI (diphthong endings — e.g. karaw, solai)
for c1, v1, c2, dv in product(C, V, C, ['ai', 'aw', 'ay']):
    candidates.add(c1 + v1 + c2 + dv)

def is_ok(w):
    if len(w) < 4 or len(w) > 6:
        return False
    if 'h' in w:
        return False
    # No same-consonant doubles in middle
    for i in range(len(w)-1):
        if w[i] == w[i+1] and w[i] not in 'aeiou':
            return False
    # No three vowels in a row
    vrun = 0
    for ch in w:
        vrun = vrun + 1 if ch in 'aeiou' else 0
        if vrun >= 3:
            return False
    return True

filtered = [w for w in candidates if is_ok(w)]
filtered.sort(key=lambda w: (-score(w), len(w), w))

print(f"Generated {len(filtered)} candidates", file=sys.stderr)

with open('~/workspace/observe/scripts/candidates.txt', 'w') as f:
    for w in filtered:
        f.write(w + '\n')

print("Written to scripts/candidates.txt", file=sys.stderr)
