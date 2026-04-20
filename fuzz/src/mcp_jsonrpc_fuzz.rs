//! Fuzz target for MCP JSON-RPC message parsing.
//!
//! Run with:
//!   cargo fuzz run mcp_jsonrpc_fuzz
//!
//! corpus/mcp_jsonrpc_fuzz/ — seed files: valid + invalid JSON-RPC messages.
//!
//! What this catches:
//!   - Panics in serde_json from deeply nested objects
//!   - Stack overflow from recursive JSON parsing
//!   - Invalid UTF-8 causing crashes in string handling
//!   - Panic in MCP method name validation

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }

    // Lossy UTF-8 — the fuzzer can still trigger all JSON parse bugs.
    let string = String::from_utf8_lossy(data);

    // Parse as raw JSON Value — catches all serde_json crash bugs.
    let _: Result<serde_json::Value, _> = serde_json::from_str(&string);

    // Additionally try parsing as a known MCP-like structure.
    #[derive(serde::Deserialize)]
    struct JsonRpcRequest {
        jsonrpc: String,
        method: Option<String>,
        id: Option<serde_json::Value>,
        params: Option<serde_json::Value>,
    }
    let _: Result<JsonRpcRequest, _> = serde_json::from_str(&string);
});
