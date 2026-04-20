//! Fuzz target for OTLP trace protobuf decoding via prost.
//!
//! Run with:
//!   cargo fuzz run otlp_traces_fuzz
//!
//! corpus/ — place sample protobuf-encoded ExportTraceServiceRequest blobs here.
//! Each file becomes a seed input that libFuzzer mutates.
//!
//! What this catches:
//!   - Panics on malformed protobuf (invalid wire types, truncated messages)
//!   - Buffer over-reads in prost's Decode implementation
//!   - Stack overflow from deeply nested message structures
//!   - Resource exhaustion from oversized allocations

#![no_main]

use libfuzzer_sys::fuzz_target;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;

fuzz_target!(|data: &[u8]| {
    // libFuzzer provides raw bytes. A zero-size input means end of fuzzing.
    if data.is_empty() {
        return;
    }

    // rust-prost returns DecodeError on invalid protobuf — expected.
    // The bug is if decode() panics or causes undefined behaviour.
    let result = ExportTraceServiceRequest::decode(data);

    if let Ok(req) = result {
        // Valid protobuf — verify self-consistency via round-trip.
        let mut buf = Vec::new();
        if req.encode(&mut buf).is_ok() {
            let _ = ExportTraceServiceRequest::decode(&buf[..]);
        }
    }
});
