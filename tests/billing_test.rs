use sigiro::billing::*;

#[test]
fn usage_counter_increment() {
    let counters = UsageCounters::new();
    counters.increment("proj_1", Signal::Spans, 10);
    counters.increment("proj_1", Signal::Spans, 5);
    let drained = counters.drain();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].2, 15);
}

#[test]
fn usage_counter_drain_resets() {
    let counters = UsageCounters::new();
    counters.increment("proj_1", Signal::Logs, 3);
    let first = counters.drain();
    assert_eq!(first.len(), 1);
    let second = counters.drain();
    assert!(second.is_empty());
}

#[test]
fn lago_event_format() {
    let events = build_lago_events(&[("proj_1".to_string(), Signal::Spans, 42)], "test_prefix");
    assert_eq!(events.len(), 1);
    let e = &events[0];
    assert_eq!(e["code"], "spans_ingested");
    assert_eq!(e["properties"]["count"], 42);
    assert!(
        e["transaction_id"]
            .as_str()
            .unwrap()
            .starts_with("test_prefix_proj_1_")
    );
}

#[test]
fn lago_batch_chunking() {
    let events: Vec<serde_json::Value> = (0..250).map(|i| serde_json::json!({ "id": i })).collect();
    let chunks = chunk_events(&events, 100);
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0].len(), 100);
    assert_eq!(chunks[1].len(), 100);
    assert_eq!(chunks[2].len(), 50);
}

#[test]
fn flush_empty_is_noop() {
    let counters = UsageCounters::new();
    let drained = counters.drain();
    assert!(drained.is_empty());
}

#[test]
fn signal_lago_codes() {
    assert_eq!(Signal::Spans.lago_code(), "spans_ingested");
    assert_eq!(Signal::Logs.lago_code(), "logs_ingested");
    assert_eq!(Signal::Metrics.lago_code(), "metrics_ingested");
    assert_eq!(Signal::Queries.lago_code(), "queries_executed");
}

#[test]
fn multiple_projects_and_signals() {
    let counters = UsageCounters::new();
    counters.increment("proj_1", Signal::Spans, 10);
    counters.increment("proj_2", Signal::Logs, 20);
    counters.increment("proj_1", Signal::Metrics, 5);
    let mut drained = counters.drain();
    drained.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.lago_code().cmp(b.1.lago_code())));
    assert_eq!(drained.len(), 3);
}
