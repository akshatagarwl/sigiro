#!/usr/bin/env python3
"""Generate OTLP protobuf corpus seeds — no external dependencies.

Run: python3 fuzz/gen_otlp_corpus.py

Outputs to fuzz/corpus/otlp_traces_fuzz/
Run this whenever opentelemetry-proto message schemas change.
"""
import os

BASE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(BASE, "corpus", "otlp_traces_fuzz")
os.makedirs(OUT, exist_ok=True)

def encode_tag(fn, wt): return bytes([(fn << 3) | wt])
def encode_varint(v):
    if v < 0: v = (1 << 64) + v
    b = []
    while v > 0x7F: b.append((v & 0x7F) | 0x80); v >>= 7
    b.append(v & 0x7F)
    return bytes(b) if b else b"\x00"
def encode_string(s): d = s.encode("utf-8"); return encode_varint(len(d)) + d
def encode_bytes(b): return encode_varint(len(b)) + b
def make_kv(k, v): return encode_tag(1,2) + encode_string(k) + encode_tag(2,2) + encode_string(v)

TRACE = b"0123456789abcdef"
SPAN = b"fedcba98"
PARENT = b"\x00" * 8
START = 1_700_000_000_000_000_000
END = 1_700_000_001_000_000_000

SEEDS = [
  ("seed_000_minimal.bin",    "corpus-service", "minimal_span",    [],                        0),
  ("seed_001_http.bin",       "user-service",   "POST /api/users", [("http.method","POST"),("http.url","https://example.com/api/users")], 2),
  ("seed_002_db.bin",         "db-service",     "SELECT * FROM users", [("db.system","postgresql"),("db.statement","SELECT * FROM users")], 1),
  ("seed_003_error.bin",      "error-service",  "Connection timeout", [("error","true")],  3),
  ("seed_004_unicode.bin",    "i18n-service",   "Query with Jose",     [("db.name","users_db")], 2),
  ("seed_005_no_attrs.bin",    "plain-service",  "plain_span",          [],                        0),
  ("seed_006_many_attrs.bin",  "verbose-service","verbose_span",        [(f"k{i}",f"v{i}") for i in range(10)], 1),
]

for fname, svc, span_name, attrs, kind in SEEDS:
    svc_attrs = [("service.name", svc)] + attrs
    attrs_enc = b"".join([make_kv(k, v) for k, v in svc_attrs])
    span_enc = (
        encode_tag(1,2)+encode_bytes(TRACE)
       +encode_tag(2,2)+encode_bytes(SPAN)
       +encode_tag(3,2)+encode_bytes(PARENT)
       +encode_tag(5,2)+encode_string(span_name)
       +encode_tag(9,0)+encode_varint(kind)
       +encode_tag(10,0)+encode_varint(START)
       +encode_tag(11,0)+encode_varint(END)
       +encode_tag(12,2)+encode_bytes(attrs_enc)
    )
    scope_spans = encode_tag(2,2)+encode_bytes(span_enc)
    resource = encode_tag(1,2)+encode_bytes(b"".join([make_kv("service.name", svc)]))
    rs = resource + encode_tag(2,2)+encode_bytes(scope_spans)
    req = encode_tag(1,2)+encode_bytes(rs)
    with open(os.path.join(OUT, fname), "wb") as f: f.write(req)
    print(f"  wrote {fname} ({len(req)} bytes)")

print(f"\nGenerated {len(SEEDS)} OTLP corpus seeds in {OUT}")
