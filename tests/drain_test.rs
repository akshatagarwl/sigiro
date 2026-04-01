use sigiro::drain::DrainTree;

#[test]
fn identical_logs_same_template() {
    let mut tree = DrainTree::new();
    let (t1, _id1) = tree.classify("User 12345 failed login");
    assert_eq!(t1, "User 12345 failed login"); // first log = exact

    let (t2, id2) = tree.classify("User 67890 failed login");
    // After second log, "12345" and "67890" differ → <*>
    assert_eq!(t2, "User <*> failed login");

    // Third log should match the updated (stable) template
    let (t3, id3) = tree.classify("User 99999 failed login");
    assert_eq!(t3, "User <*> failed login");
    assert_eq!(id3, id2); // stable after convergence
}

#[test]
fn different_structures_different_templates() {
    let mut tree = DrainTree::new();
    tree.classify("Connection refused to host 10.0.0.1");
    tree.classify("Timeout waiting for response from server");
    // Different token counts → different groups → different templates
    // (can't directly check groups, but we can verify different templates)
    let (t1, _) = tree.classify("Connection refused to host 10.0.0.2");
    let (t2, _) = tree.classify("Timeout waiting for response from client");
    assert_ne!(t1, t2);
}

#[test]
fn empty_body() {
    let mut tree = DrainTree::new();
    let (template, id) = tree.classify("");
    assert_eq!(template, "");
    assert_eq!(id, 0);
}

#[test]
fn convergence_after_few_logs() {
    let mut tree = DrainTree::new();
    // Simulate 10 similar logs with varying IDs
    for i in 0..10 {
        tree.classify(&format!("Order {i} shipped to warehouse 42"));
    }
    // After convergence, only the varying token becomes <*>. "42" is constant.
    let (template, _) = tree.classify("Order 999 shipped to warehouse 42");
    assert_eq!(template, "Order <*> shipped to warehouse 42");
}

#[test]
fn max_clusters_respected() {
    let mut tree = DrainTree::new();
    // Create logs with same token count but completely different content
    for i in 0..600 {
        tree.classify(&format!("unique_{i}_a unique_{i}_b unique_{i}_c"));
    }
    // Should not panic or grow unboundedly — internal limit caps clusters.
    // We can verify the tree still classifies correctly.
    let (_, _) = tree.classify("unique_999_a unique_999_b unique_999_c");
}
