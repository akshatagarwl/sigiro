#!/usr/bin/env python3
"""Generate OTLP protobuf corpus seeds for fuzz/otlp_traces_fuzz.

Run: python3 fuzz/gen_otlp_corpus.py
Outputs to fuzz/corpus/otlp_traces_fuzz/

Proto encoding: field_number << 3 | wire_type
  wire_type 0 = varint,  2 = length-delimited,  5 = 32-bit
  All integers are unsigned varint. Negative fixed32/64 stored as zigzag.

To fetch real OTel proto JSON examples and convert:
  1. Clone: git clone --depth1 --branch v1.3.2 https://github.com/open-telemetry/opentelemetry-proto
  2. protoc --decode_raw < opentelemetry-proto/examples/trace.json  # see structure
  3. Write a script to JSON->protobuf using the proto definitions
This script produces hand-verified seeds without requiring protoc.
"""
import os

BASE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(BASE, "corpus", "otlp_traces_fuzz")
os.makedirs(OUT, exist_ok=True)


def varint(v: int) -> bytes:
    if v < 0:
        v = (1 << 64) + v
    b = []
    while v > 0x7F:
        b.append((v & 0x7F) | 0x80)
        v >>= 7
    b.append(v & 0x7F)
    return bytes(b) if b else b"\x00"


def encode_tag(fn: int, wt: int) -> bytes:
    return bytes([(fn << 3) | wt])


def encode_string(s: str) -> bytes:
    d = s.encode("utf-8")
    return varint(len(d)) + d


def encode_bytes(b: bytes) -> bytes:
    return varint(len(b)) + b


def make_kv(key: str, val: str) -> bytes:
    return encode_tag(1, 2) + encode_string(key) + encode_tag(2, 2) + encode_string(val)


TRACE_ID = b"0123456789abcdef"
SPAN_ID = b"fedcba98"
PARENT_SPAN = b"\x00" * 8
START_NANO = 1_700_000_000_000_000_000
END_NANO = 1_700_000_001_000_000_000

# Span kinds: 0=unspecified, 1=server, 2=client, 3=producer, 4=consumer
KIND_UNSPECIFIED, KIND_SERVER, KIND_CLIENT, KIND_PRODUCER, KIND_CONSUMER = 0, 1, 2, 3, 4


def make_span(name: str, trace: bytes, span: bytes, parent: bytes,
              start: int, end: int, kind: int, attrs: list) -> bytes:
    parts = [
        encode_tag(1, 2) + encode_bytes(trace),
        encode_tag(2, 2) + encode_bytes(span),
        encode_tag(3, 2) + encode_bytes(parent),
        encode_tag(5, 2) + encode_string(name),
        encode_tag(9, 0) + varint(kind),
        encode_tag(10, 0) + varint(start),
        encode_tag(11, 0) + varint(end),
    ]
    if attrs:
        attrs_enc = b"".join(make_kv(k, v) for k, v in attrs)
        parts.append(encode_tag(12, 2) + encode_bytes(attrs_enc))
    return b"".join(parts)


def make_resource_spans(trace: bytes, span: bytes, parent: bytes,
                        name: str, start: int, end: int,
                        kind: int, attrs: list,
                        svc_name: str = "corpus-gen") -> bytes:
    svc_attrs = [("service.name", svc_name)] + attrs
    span_enc = make_span(name, trace, span, parent, start, end, kind, svc_attrs)
    scope_spans = encode_tag(2, 2) + encode_bytes(span_enc)
    svc_name_attr = make_kv("service.name", svc_name)
    resource = encode_tag(1, 2) + encode_bytes(svc_name_attr)
    rs = resource + encode_tag(2, 2) + encode_bytes(scope_spans)
    return encode_tag(1, 2) + encode_bytes(rs)


SEEDS = [
    # Minimal valid span
    ("seed_000_minimal.bin", make_resource_spans(
        TRACE_ID, SPAN_ID, PARENT_SPAN,
        "minimal_span", START_NANO, END_NANO, KIND_UNSPECIFIED, [])),

    # HTTP server span
    ("seed_001_http_server.bin", make_resource_spans(
        TRACE_ID, SPAN_ID, PARENT_SPAN,
        "POST /api/users", START_NANO, END_NANO, KIND_SERVER,
        [("http.method", "POST"), ("http.url", "https://example.com/api/users"),
         ("http.status_code", "200")])),

    # HTTP client span
    ("seed_002_http_client.bin", make_resource_spans(
        TRACE_ID, SPAN_ID, PARENT_SPAN,
        "HTTP GET", START_NANO, END_NANO, KIND_CLIENT,
        [("http.method", "GET"), ("net.peer.name", "api.example.com"),
         ("net.peer.port", "443")])),

    # Database client span
    ("seed_003_db_query.bin", make_resource_spans(
        TRACE_ID, SPAN_ID, PARENT_SPAN,
        "SELECT users", START_NANO, END_NANO, KIND_CLIENT,
        [("db.system", "postgresql"), ("db.statement", "SELECT * FROM users WHERE id=1"),
         ("db.name", "production")])),

    # Error span
    ("seed_004_error.bin", make_resource_spans(
        TRACE_ID, SPAN_ID, PARENT_SPAN,
        "handle_request", START_NANO, END_NANO, KIND_SERVER,
        [("error", "true"), ("error.message", "Connection timeout"),
         ("http.status_code", "500")])),

    # Unicode span name and attributes
    ("seed_005_unicode.bin", make_resource_spans(
        TRACE_ID, SPAN_ID, PARENT_SPAN,
        "Query with José and 阿瑟·琼斯", START_NANO, END_NANO, KIND_UNSPECIFIED,
        [("user.name", "José"), ("db.name", "У何尝")])),

    # Span with no attributes
    ("seed_006_no_attrs.bin", make_resource_spans(
        TRACE_ID, SPAN_ID, PARENT_SPAN,
        "plain_span", START_NANO, END_NANO, KIND_UNSPECIFIED, [])),

    # Span with many attributes (resource exhaustion edge case)
    ("seed_007_many_attrs.bin", make_resource_spans(
        TRACE_ID, SPAN_ID, PARENT_SPAN,
        "verbose_span", START_NANO, END_NANO, KIND_UNSPECIFIED,
        [(f"attr.key{i}", f"attr.value{i}") for i in range(20)])),

    # Producer span
    ("seed_008_producer.bin", make_resource_spans(
        TRACE_ID, SPAN_ID, PARENT_SPAN,
        "kafka produce", START_NANO, END_NANO, KIND_PRODUCER,
        [("messaging.system", "kafka"), ("messaging.destination", "events"),
         ("messaging.operation", "publish")])),

    # Consumer span
    ("seed_009_consumer.bin", make_resource_spans(
        TRACE_ID, SPAN_ID, PARENT_SPAN,
        "kafka consume", START_NANO, END_NANO, KIND_CONSUMER,
        [("messaging.system", "kafka"), ("messaging.destination", "events"),
         ("messaging.partition", "3")])),

    # Empty string span name (edge case)
    ("seed_010_empty_name.bin", make_resource_spans(
        TRACE_ID, SPAN_ID, PARENT_SPAN,
        "", START_NANO, END_NANO, KIND_UNSPECIFIED, [])),

    # Very long span name (edge case)
    ("seed_011_long_name.bin", make_resource_spans(
        TRACE_ID, SPAN_ID, PARENT_SPAN,
        "x" * 1024, START_NANO, END_NANO, KIND_UNSPECIFIED, [])),
]


for fname, data in SEEDS:
    path = os.path.join(OUT, fname)
    with open(path, "wb") as f:
        f.write(data)
    print(f"  wrote {fname} ({len(data)} bytes)")

print(f"\nGenerated {len(SEEDS)} OTLP corpus seeds in {OUT}")
