#!/usr/bin/env python3
"""
Fast domain availability check using parallel DNS queries.
DNS is ~10x faster than RDAP for bulk checks.
No NS records = domain likely unregistered.

Usage:
    python3 check_domains.py candidates.txt com_free.txt
"""

import asyncio
import sys

CONCURRENCY = 500  # massively parallel DNS

VOW = set('aeiou')
SONORANT = set('lmnr')
STOP = set('bdgkpt')
FRIC = set('fsvz')

VALID_CC = {
    'bl','br','cl','cr','dr','fl','fr','gl','gr','kl','kr',
    'pl','pr','sc','sk','sl','sm','sn','sp','st','sw','tr','tw',
    'nd','nt','nk','rd','rn','rk','rt','ld','lk','lt','mp','ng',
    'ns','rs','ls','ts','ks',
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
    elif w[-1] in SONORANT: s += 5
    elif w[-1] in 'sk':   s += 1
    if len(w) == 5: s += 4
    elif len(w) == 6: s += 3
    elif len(w) == 7: s += 1
    cc = sum(1 for i in range(len(w)-1) if w[i] not in VOW and w[i+1] not in VOW)
    s -= cc * 2
    vc = sum(1 for c in w if c in VOW)
    if 0.35 <= vc/len(w) <= 0.65: s += 1
    return s


async def check_dns(resolver, word, sem, tld='com'):
    """Returns word only on NXDOMAIN (domain truly does not exist)."""
    import aiodns
    async with sem:
        try:
            await resolver.query(f"{word}.{tld}", 'A')
            return None  # resolved = registered
        except aiodns.error.DNSError as e:
            # ARES_ENOTFOUND (4) = NXDOMAIN = domain doesn't exist
            if e.args[0] == 4:
                return word
            return None  # timeout, refused, servfail = assume registered
        except Exception:
            return None  # anything else = assume registered (safe default)


async def main(candidates, tld):
    try:
        import aiodns
    except ImportError:
        print("Installing aiodns...", file=sys.stderr)
        import subprocess
        subprocess.run([sys.executable, '-m', 'pip', 'install', 'aiodns', '-q'])
        import aiodns

    resolver = aiodns.DNSResolver(timeout=2, tries=1)
    sem = asyncio.Semaphore(CONCURRENCY)

    free = []
    batch = 5000
    total = len(candidates)
    for i in range(0, total, batch):
        chunk = candidates[i:i+batch]
        tasks = [check_dns(resolver, w, sem, tld) for w in chunk]
        results = await asyncio.gather(*tasks)
        found = [r for r in results if r]
        free.extend(found)
        done = min(i+batch, total)
        print(f"  {done}/{total} — {len(free)} .{tld} free", file=sys.stderr)

    return free


if __name__ == '__main__':
    infile  = sys.argv[1] if len(sys.argv) > 1 else 'scripts/candidates.txt'
    outfile = sys.argv[2] if len(sys.argv) > 2 else 'scripts/com_free.txt'
    tld     = sys.argv[3] if len(sys.argv) > 3 else 'com'

    with open(infile) as f:
        all_candidates = [l.strip() for l in f if l.strip()]

    candidates = [w for w in all_candidates if ok(w)]
    print(f"After filter: {len(candidates)}/{len(all_candidates)} candidates", file=sys.stderr)

    free = asyncio.run(main(candidates, tld))
    free.sort(key=lambda w: (-score(w), len(w), w))

    with open(outfile, 'w') as f:
        for w in free:
            f.write(w+'\n')

    print(f"\n.{tld} available: {len(free)} names → {outfile}", file=sys.stderr)
    print(f"\nTop 60:", file=sys.stderr)
    for w in free[:60]:
        print(f"  {w}", file=sys.stderr)
