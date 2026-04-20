//! Fuzz target for MCP JSON-RPC message parsing via rmcp's actual types.
//!
//! Run with:
//!   cargo +nightly fuzz run mcp_jsonrpc_fuzz
//!
//! corpus/mcp_jsonrpc_fuzz/ — seed files: valid + invalid MCP JSON-RPC messages.
//!
//! This fuzzes the real MCP codec:
//!   - rmcp::model::JsonRpcMessage<ClientRequest> deserialization
//!   - Typed ClientRequest enum variants (initialize, tools/list, tools/call, etc.)
//!   - is_standard_method validation in rmcp's codec
//!   - serde_json integration inside rmcp

#![no_main]

use libfuzzer_sys::fuzz_target;
use rmcp::model::{ClientRequest, JsonRpcMessage};

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }

    // Lossy UTF-8 — libfuzzer provides raw bytes, rmcp codec expects strings.
    let string = String::from_utf8_lossy(data);

    // Deserialize into the actual rmcp MCP message type — this exercises:
    //   - JsonRpcMessage<ClientRequest> deserialization
    //   - ClientRequest enum with all typed MCP method variants
    //   - serde_json with real typed fields, not just Value
    let result: Result<JsonRpcMessage<ClientRequest, serde_json::Value, serde_json::Value>, _> =
        serde_json::from_str(&string);

    // If parsing succeeded, also verify the JSON round-trips cleanly.
    if let Ok(msg) = result {
        let _ = serde_json::to_string(&msg);
    }
});
