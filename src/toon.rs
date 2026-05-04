use serde_json::Value;

/// Convert JSON to TOON format (token-efficient for LLM output).
pub fn to_toon(value: &Value) -> String {
    toon::encode(value, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_simple_object() {
        let value = json!({
            "status": "ok",
            "version": "1.0.0"
        });
        let toon = to_toon(&value);
        assert!(toon.contains("status: ok"));
        assert!(toon.contains("version: 1.0.0"));
    }

    #[test]
    fn test_array_of_objects() {
        let value = json!([
            {"name": "checkout", "spans": 1500},
            {"name": "payments", "spans": 800}
        ]);
        let toon = to_toon(&value);
        assert!(!toon.is_empty());
    }

    #[test]
    fn test_nested_object() {
        let value = json!({
            "service": "checkout",
            "window": {
                "from_ts": 1711000000,
                "to_ts": 1711001000
            }
        });
        let toon = to_toon(&value);
        assert!(toon.contains("service: checkout"));
    }
}
