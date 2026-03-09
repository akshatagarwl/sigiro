#!/usr/bin/env python3
"""
Fast parallel domain availability check.
Budget: ≤$15/yr → .com (~$10/yr) or .dev (~$10/yr). .ai at $75/yr is excluded.

Usage:
    python3 check_domains.py candidates.txt

Output:
    com_free.txt  — names available on .com
    dev_free.txt  — names available on .dev (checked only if not .com-free)
"""

import asyncio
import aiohttp
import sys

CONCURRENCY = 120
RDAP_COM = "https://rdap.verisign.com/com/v1/domain/{}.com"
RDAP_DEV = "https://www.registry.google/rdap/domain/{}.dev"
TIMEOUT = aiohttp.ClientTimeout(total=8)


async def check(session, url, word, sem):
    async with sem:
        try:
            async with session.get(url, timeout=TIMEOUT) as resp:
                return resp.status == 404
        except Exception:
            return False  # network error = assume taken


async def main(candidates):
    sem = asyncio.Semaphore(CONCURRENCY)
    connector = aiohttp.TCPConnector(limit=CONCURRENCY, ttl_dns_cache=300)

    com_free = []
    dev_free = []
    batch = 1000
    total = len(candidates)

    async with aiohttp.ClientSession(connector=connector) as session:
        # Phase 1: check .com
        print("Phase 1: checking .com...", file=sys.stderr)
        for i in range(0, total, batch):
            chunk = candidates[i:i+batch]
            tasks = [check(session, RDAP_COM.format(w), w, sem) for w in chunk]
            results = await asyncio.gather(*tasks)
            for w, avail in zip(chunk, results):
                if avail:
                    com_free.append(w)
            done = min(i + batch, total)
            print(f"  {done}/{total} — {len(com_free)} .com free", file=sys.stderr)

        # Phase 2: check .dev for names NOT available on .com
        # (most dev builders will use .dev as primary anyway)
        print("\nPhase 2: checking .dev for remaining...", file=sys.stderr)
        not_com = [w for w in candidates if w not in set(com_free)]
        # Sample every 3rd to keep this fast — still get good coverage
        sample = not_com[::3]
        for i in range(0, len(sample), batch):
            chunk = sample[i:i+batch]
            tasks = [check(session, RDAP_DEV.format(w), w, sem) for w in chunk]
            results = await asyncio.gather(*tasks)
            for w, avail in zip(chunk, results):
                if avail:
                    dev_free.append(w)
            done = min(i + batch, len(sample))
            print(f"  {done}/{len(sample)} — {len(dev_free)} .dev free", file=sys.stderr)

    return com_free, dev_free


def score(w):
    """Phonetic quality score — higher = more likely to feel like a good tech name."""
    s = 0
    SONORANT = set('lmnr')
    STOP = set('bdgkpt')
    FRICATIVE = set('fsvz')
    # Ends in vowel (soft, memorable — like figma, versa, navi)
    if w[-1] in 'aeiou':
        s += 4
    # Ends in sonorant (clean — like render, linear)
    elif w[-1] in SONORANT:
        s += 2
    # Starts with stop or fricative (punchy)
    if w[0] in STOP | FRICATIVE:
        s += 1
    # 4-5 chars strongly preferred
    if len(w) == 4:
        s += 3
    elif len(w) == 5:
        s += 2
    elif len(w) == 6:
        s += 1
    # Penalize back-to-back stops (hard to say)
    for i in range(len(w)-1):
        if w[i] in STOP and w[i+1] in STOP:
            s -= 3
    # Reward vowel-consonant balance (~40-60% vowels)
    vc = sum(1 for c in w if c in 'aeiou')
    ratio = vc / len(w)
    if 0.35 <= ratio <= 0.60:
        s += 1
    return s


if __name__ == '__main__':
    infile = sys.argv[1] if len(sys.argv) > 1 else 'candidates.txt'

    with open(infile) as f:
        candidates = [line.strip() for line in f if line.strip()]

    print(f"Checking {len(candidates)} candidates...", file=sys.stderr)
    com_free, dev_free = asyncio.run(main(candidates))

    # Score and sort
    com_ranked = sorted(com_free, key=lambda w: (-score(w), len(w), w))
    dev_ranked = sorted(dev_free, key=lambda w: (-score(w), len(w), w))

    base = infile.rsplit('/', 1)[0] if '/' in infile else '.'

    with open(f'{base}/com_free.txt', 'w') as f:
        for w in com_ranked:
            f.write(f"{w}\n")

    with open(f'{base}/dev_free.txt', 'w') as f:
        for w in dev_ranked:
            f.write(f"{w}\n")

    print(f"\n=== RESULTS ===", file=sys.stderr)
    print(f".com available: {len(com_free)} names → scripts/com_free.txt", file=sys.stderr)
    print(f".dev available: {len(dev_free)} names (sampled) → scripts/dev_free.txt", file=sys.stderr)
    print(f"\nTop .com candidates:", file=sys.stderr)
    for w in com_ranked[:30]:
        print(f"  {w}", file=sys.stderr)
    print(f"\nTop .dev candidates:", file=sys.stderr)
    for w in dev_ranked[:30]:
        print(f"  {w}", file=sys.stderr)
