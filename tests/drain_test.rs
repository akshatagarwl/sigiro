//! Tests for the DRAIN log template extraction algorithm.
//!
//! Inspired by test patterns from drain-rs (benwtrent) and drain-flow
//! (nharring-adjacent) — two production Rust DRAIN implementations.

use sigiro::drain::DrainTree;

// ---------------------------------------------------------------------------
// Basic template extraction
// ---------------------------------------------------------------------------

#[test]
fn first_log_returns_exact_body() {
    let mut tree = DrainTree::new();
    let (template, _) = tree.classify("Connection refused to host 10.0.0.1");
    assert_eq!(template, "Connection refused to host 10.0.0.1");
}

#[test]
fn second_log_with_variable_token_creates_wildcard() {
    let mut tree = DrainTree::new();
    tree.classify("User 12345 failed login");
    let (template, _) = tree.classify("User 67890 failed login");
    assert_eq!(template, "User <*> failed login");
}

#[test]
fn stable_template_after_convergence() {
    let mut tree = DrainTree::new();
    tree.classify("User alice failed login");
    let (_, id2) = tree.classify("User bob failed login");
    let (t3, id3) = tree.classify("User charlie failed login");
    assert_eq!(t3, "User <*> failed login");
    assert_eq!(id3, id2); // template_id stable once converged
}

#[test]
fn constant_tokens_preserved() {
    let mut tree = DrainTree::new();
    for i in 0..10 {
        tree.classify(&format!("Order {i} shipped to warehouse 42"));
    }
    let (template, _) = tree.classify("Order 999 shipped to warehouse 42");
    // "42" is constant across all logs — should NOT become <*>
    assert_eq!(template, "Order <*> shipped to warehouse 42");
}

// ---------------------------------------------------------------------------
// Different structures → different groups
// ---------------------------------------------------------------------------

#[test]
fn different_token_counts_different_groups() {
    let mut tree = DrainTree::new();
    let (t1, id1) = tree.classify("short message");
    let (t2, id2) = tree.classify("this is a much longer message with more tokens");
    assert_ne!(id1, id2);
    assert_ne!(t1, t2);
}

#[test]
fn similar_structure_same_group() {
    let mut tree = DrainTree::new();
    tree.classify("GET /api/users/100 200 15ms");
    let (t, _) = tree.classify("GET /api/users/200 200 23ms");
    assert!(
        t.contains("<*>"),
        "variable path/timing should be wildcarded"
    );
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

#[test]
fn empty_body_returns_empty() {
    let mut tree = DrainTree::new();
    let (template, id) = tree.classify("");
    assert_eq!(template, "");
    assert_eq!(id, 0);
}

#[test]
fn single_token_log() {
    let mut tree = DrainTree::new();
    let (t, _) = tree.classify("ERROR");
    assert_eq!(t, "ERROR");
}

#[test]
fn whitespace_only_treated_as_empty() {
    let mut tree = DrainTree::new();
    let (template, id) = tree.classify("   ");
    assert_eq!(template, "");
    assert_eq!(id, 0);
}

#[test]
fn single_token_completely_different_stays_separate() {
    let mut tree = DrainTree::new();
    let (_, id1) = tree.classify("abc123");
    let (_, id2) = tree.classify("def456");
    // Single-token logs with 0% similarity → separate clusters (correct behavior).
    // DRAIN requires ≥50% token match to merge. 0/1 tokens match = 0%.
    assert_ne!(id1, id2);
}

// ---------------------------------------------------------------------------
// Real-world log patterns
// ---------------------------------------------------------------------------

#[test]
fn http_access_logs() {
    let mut tree = DrainTree::new();
    // Warm up
    tree.classify("GET /api/v1/users/1001 HTTP/1.1 200 42ms");
    tree.classify("GET /api/v1/users/2002 HTTP/1.1 200 38ms");
    // Should converge: user ID and timing are variable
    let (t, _) = tree.classify("GET /api/v1/users/3003 HTTP/1.1 200 51ms");
    assert_eq!(t, "GET <*> HTTP/1.1 200 <*>");
}

#[test]
fn kubernetes_pod_events() {
    let mut tree = DrainTree::new();
    tree.classify("Pod api-server-abc12 started on node worker-1");
    tree.classify("Pod api-server-def34 started on node worker-2");
    let (t, _) = tree.classify("Pod api-server-ghi56 started on node worker-3");
    assert_eq!(t, "Pod <*> started on node <*>");
}

#[test]
fn database_query_logs() {
    let mut tree = DrainTree::new();
    tree.classify("Query SELECT * FROM users WHERE id = 1001 took 5ms");
    tree.classify("Query SELECT * FROM users WHERE id = 2002 took 12ms");
    let (t, _) = tree.classify("Query SELECT * FROM users WHERE id = 3003 took 8ms");
    // id and timing are variable, the rest is constant
    assert!(t.contains("<*>"));
    assert!(t.contains("Query"));
    assert!(t.contains("users"));
}

#[test]
fn error_with_uuid() {
    let mut tree = DrainTree::new();
    tree.classify("Failed to process request 550e8400-e29b-41d4-a716-446655440000");
    tree.classify("Failed to process request a1b2c3d4-e5f6-7890-abcd-ef1234567890");
    let (t, _) = tree.classify("Failed to process request 99999999-aaaa-bbbb-cccc-dddddddddddd");
    assert_eq!(t, "Failed to process request <*>");
}

#[test]
fn error_with_ip_addresses() {
    let mut tree = DrainTree::new();
    tree.classify("Connection timeout for 192.168.1.1 port 5432");
    tree.classify("Connection timeout for 10.0.0.5 port 5432");
    let (t, _) = tree.classify("Connection timeout for 172.16.0.99 port 5432");
    // IP varies, port is constant
    assert_eq!(t, "Connection timeout for <*> port 5432");
}

#[test]
fn mixed_severity_same_structure() {
    let mut tree = DrainTree::new();
    // Same structure, different severity prefix — should still match
    // because token-by-token "INFO" vs "WARN" differs at position 0
    tree.classify("INFO Request 100 completed in 5ms");
    tree.classify("WARN Request 200 completed in 15ms");
    let (t, _) = tree.classify("ERROR Request 300 completed in 50ms");
    // All three differ at position 0 (INFO/WARN/ERROR) and positions 2,5
    assert!(t.contains("<*>"));
}

// ---------------------------------------------------------------------------
// Cluster capacity / bounded growth
// ---------------------------------------------------------------------------

#[test]
fn max_clusters_bounded() {
    let mut tree = DrainTree::new();
    // Create many completely unique 3-token logs
    for i in 0..1000 {
        tree.classify(&format!("unique_{i}_a unique_{i}_b unique_{i}_c"));
    }
    // Should not panic or OOM — internal limit caps clusters
    let (_, _) = tree.classify("one_more_a one_more_b one_more_c");
}

#[test]
fn many_groups_by_token_count() {
    let mut tree = DrainTree::new();
    // Create logs with different token counts (1 through 20)
    for n in 1..=20 {
        let tokens: Vec<String> = (0..n).map(|i| format!("word{i}")).collect();
        tree.classify(&tokens.join(" "));
    }
    // Should handle 20 different groups without issue
    let (t, _) = tree.classify("word0 word1 word2");
    assert!(!t.is_empty());
}

// ---------------------------------------------------------------------------
// Template ID consistency
// ---------------------------------------------------------------------------

#[test]
fn same_template_same_id() {
    let mut tree = DrainTree::new();
    // Warm up
    tree.classify("Request 1 failed");
    tree.classify("Request 2 failed");
    // Now converged — all subsequent should get same id
    let (_, id1) = tree.classify("Request 3 failed");
    let (_, id2) = tree.classify("Request 4 failed");
    let (_, id3) = tree.classify("Request 999 failed");
    assert_eq!(id1, id2);
    assert_eq!(id2, id3);
}

#[test]
fn different_templates_different_ids() {
    let mut tree = DrainTree::new();
    tree.classify("Connection timeout for 1.1.1.1");
    tree.classify("Connection timeout for 2.2.2.2");
    tree.classify("Disk usage at 80 percent on host-1");
    tree.classify("Disk usage at 90 percent on host-2");
    let (_, id_conn) = tree.classify("Connection timeout for 3.3.3.3");
    let (_, id_disk) = tree.classify("Disk usage at 95 percent on host-3");
    assert_ne!(id_conn, id_disk);
}

// ---------------------------------------------------------------------------
// Similarity threshold behavior
// ---------------------------------------------------------------------------

#[test]
fn mostly_different_tokens_stays_separate() {
    let mut tree = DrainTree::new();
    // Two 6-token logs that share only "the" — below 0.5 similarity threshold
    tree.classify("the quick brown fox jumps over");
    let (t, _) = tree.classify("the lazy old dog sleeps under");
    // Should NOT merge — only 1/6 tokens match
    assert!(!t.contains("<*>"), "dissimilar logs should not merge: {t}");
}

#[test]
fn majority_matching_tokens_merges() {
    let mut tree = DrainTree::new();
    // 6 tokens, 4 match → 4/6 = 0.67 > 0.5 threshold
    tree.classify("server alpha is running on port 8080");
    let (t, _) = tree.classify("server beta is running on port 9090");
    // "alpha"/"beta" and "8080"/"9090" should become <*>
    assert_eq!(t, "server <*> is running on port <*>");
}

// ---------------------------------------------------------------------------
// Batch processing (simulates inserter behavior)
// ---------------------------------------------------------------------------

#[test]
fn batch_of_similar_logs_converges() {
    let mut tree = DrainTree::new();
    let mut last_id = 0;
    for i in 0..100 {
        let (_, id) = tree.classify(&format!(
            "Worker thread-{i} processed batch {} in {}ms",
            i * 10,
            i + 5
        ));
        last_id = id;
    }
    // After 100 similar logs, template should be stable
    let (t, id) = tree.classify("Worker thread-999 processed batch 9990 in 1004ms");
    assert_eq!(id, last_id);
    assert_eq!(t, "Worker <*> processed batch <*> in <*>");
}
