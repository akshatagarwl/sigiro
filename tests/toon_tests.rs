use serde_json::json;

#[test]
fn test_toon_status_output() {
    let status = json!({
        "status": "ok",
        "version": "1.0.0"
    });
    let toon = sigiro::toon::to_toon(&status);

    // TOON should be more compact than JSON
    let json_str = serde_json::to_string_pretty(&status).unwrap();
    assert!(
        toon.len() < json_str.len(),
        "TOON should be more compact than JSON"
    );

    // TOON should contain the data
    assert!(
        toon.contains("status: ok"),
        "TOON should contain status field"
    );
    assert!(
        toon.contains("version: 1.0.0"),
        "TOON should contain version field"
    );
}

#[test]
fn test_toon_services_array() {
    let services = json!([
        {"name": "checkout", "spans": 1500, "logs": 300},
        {"name": "payments", "spans": 800, "logs": 200}
    ]);
    let toon = sigiro::toon::to_toon(&services);

    // TOON should use table format for arrays of objects
    assert!(toon.contains("["), "TOON should have array indicator");
    assert!(toon.contains("name"), "TOON should contain name field");
    assert!(toon.contains("spans"), "TOON should contain spans field");
    assert!(toon.contains("logs"), "TOON should contain logs field");

    // Should contain the actual data
    assert!(
        toon.contains("checkout"),
        "TOON should contain checkout service"
    );
    assert!(toon.contains("1500"), "TOON should contain span count");
}

#[test]
fn test_toon_investigate_structure() {
    let investigate = json!({
        "service": "checkout",
        "window": {
            "from_ts": 1711000000,
            "to_ts": 1711001000
        },
        "operation_summaries": [
            {
                "span_name": "POST /checkout",
                "count": 100,
                "error_pct": 5.0
            }
        ]
    });
    let toon = sigiro::toon::to_toon(&investigate);

    // Should handle nested objects
    assert!(
        toon.contains("service: checkout"),
        "TOON should contain service"
    );
    assert!(
        toon.contains("window"),
        "TOON should contain window section"
    );
    assert!(
        toon.contains("operation_summaries"),
        "TOON should contain operation summaries"
    );
}

#[test]
fn test_toon_empty_values() {
    let empty_array = json!([]);
    let toon = sigiro::toon::to_toon(&empty_array);
    // Empty arrays might return empty string or some indicator
    // The important thing is it doesn't panic
    let _ = toon; // Just verify it doesn't panic

    let empty_object = json!({});
    let toon = sigiro::toon::to_toon(&empty_object);
    // Empty objects might return empty string
    // The important thing is it doesn't panic
    let _ = toon; // Just verify it doesn't panic
}

#[test]
fn test_toon_special_characters() {
    let with_commas = json!({
        "message": "Hello, world",
        "description": "This has, commas"
    });
    let toon = sigiro::toon::to_toon(&with_commas);

    // Should handle strings with commas
    assert!(
        toon.contains("Hello, world"),
        "TOON should handle commas in strings"
    );
    assert!(
        toon.contains("This has, commas"),
        "TOON should handle commas in strings"
    );
}

#[test]
fn test_toon_numeric_types() {
    let numbers = json!({
        "integer": 42,
        "float": 1.61803,
        "negative": -100
    });
    let toon = sigiro::toon::to_toon(&numbers);

    assert!(toon.contains("42"), "TOON should contain integer");
    assert!(toon.contains("1.618"), "TOON should contain float");
    assert!(toon.contains("-100"), "TOON should contain negative number");
}

#[test]
fn test_toon_boolean_values() {
    let booleans = json!({
        "active": true,
        "deleted": false
    });
    let toon = sigiro::toon::to_toon(&booleans);

    assert!(toon.contains("true"), "TOON should contain true");
    assert!(toon.contains("false"), "TOON should contain false");
}

#[test]
fn test_toon_null_values() {
    let with_null = json!({
        "present": "value",
        "missing": null
    });
    let toon = sigiro::toon::to_toon(&with_null);

    assert!(
        toon.contains("present: value"),
        "TOON should contain present field"
    );
    // Null handling depends on implementation
    assert!(!toon.is_empty(), "TOON should handle null values");
}

#[test]
fn test_toon_performance_comparison() {
    // Create a larger structure to test token efficiency
    let mut operations = Vec::new();
    for i in 0..10 {
        operations.push(json!({
            "span_name": format!("operation_{}", i),
            "count": i * 100,
            "error_pct": i as f64 * 0.5,
            "duration_p95_ns": i as f64 * 1000000.0
        }));
    }

    let large_structure = json!({
        "service": "test-service",
        "operations": operations
    });

    let toon = sigiro::toon::to_toon(&large_structure);
    let json_str = serde_json::to_string_pretty(&large_structure).unwrap();

    // TOON should be significantly more compact
    let compression_ratio = toon.len() as f64 / json_str.len() as f64;
    assert!(
        compression_ratio < 0.8,
        "TOON should be at least 20% more compact than JSON. Ratio: {:.2}",
        compression_ratio
    );
}

#[test]
fn snapshot_toon_simple_object() {
    let value = json!({
        "status": "ok",
        "version": "1.0.0"
    });
    let toon = sigiro::toon::to_toon(&value);
    insta::assert_snapshot!(toon);
}

#[test]
fn snapshot_toon_array_of_objects() {
    let value = json!([
        {"name": "checkout", "spans": 1500},
        {"name": "payments", "spans": 800}
    ]);
    let toon = sigiro::toon::to_toon(&value);
    insta::assert_snapshot!(toon);
}

#[test]
fn snapshot_toon_nested_object() {
    let value = json!({
        "service": "checkout",
        "window": {
            "from_ts": 1711000000,
            "to_ts": 1711001000
        }
    });
    let toon = sigiro::toon::to_toon(&value);
    insta::assert_snapshot!(toon);
}

#[test]
fn snapshot_toon_performance_comparison() {
    let mut operations = Vec::new();
    for i in 0..10 {
        operations.push(json!({
            "span_name": format!("operation_{}", i),
            "count": i * 100,
            "error_pct": i as f64 * 0.5,
            "duration_p95_ns": i as f64 * 1000000.0
        }));
    }

    let large_structure = json!({
        "service": "test-service",
        "operations": operations
    });

    let toon = sigiro::toon::to_toon(&large_structure);
    insta::assert_snapshot!(toon);
}
