use sigiro_segments::{Manifest, ManifestError, Segment, Table};

fn seg(key: &str, table: Table, min_us: i64, max_us: i64) -> Segment {
    Segment {
        key: key.into(),
        table,
        min_time_us: min_us,
        max_time_us: max_us,
        rows: 10,
        bytes: 100,
    }
}

fn keys(segments: Vec<&Segment>) -> Vec<String> {
    let mut keys: Vec<String> = segments.into_iter().map(|s| s.key.clone()).collect();
    keys.sort();
    keys
}

#[test]
fn a_registered_segment_is_not_queryable_until_committed() {
    let mut m = Manifest::default();
    m.register("a", Table::Spans).unwrap();
    assert_eq!(
        keys(m.live(Table::Spans, 0, i64::MAX)),
        Vec::<String>::new()
    );
    m.commit(seg("a", Table::Spans, 10, 20)).unwrap();
    assert_eq!(keys(m.live(Table::Spans, 0, i64::MAX)), vec!["a"]);
}

#[test]
fn committing_an_unregistered_segment_is_rejected() {
    let mut m = Manifest::default();
    assert_eq!(
        m.commit(seg("a", Table::Spans, 10, 20)),
        Err(ManifestError::NotPending("a".into()))
    );
}

#[test]
fn a_key_cannot_be_registered_twice() {
    let mut m = Manifest::default();
    m.register("a", Table::Spans).unwrap();
    assert_eq!(
        m.register("a", Table::Spans),
        Err(ManifestError::KeyInUse("a".into()))
    );
    m.commit(seg("a", Table::Spans, 10, 20)).unwrap();
    assert_eq!(
        m.register("a", Table::Spans),
        Err(ManifestError::KeyInUse("a".into()))
    );
}

#[test]
fn live_segments_are_filtered_by_table_and_time_overlap() {
    let mut m = Manifest::default();
    for (k, sig, lo, hi) in [
        ("a", Table::Spans, 0, 9),
        ("b", Table::Spans, 10, 19),
        ("c", Table::Logs, 10, 19),
        ("d", Table::Spans, 20, 29),
    ] {
        m.register(k, sig).unwrap();
        m.commit(seg(k, sig, lo, hi)).unwrap();
    }
    assert_eq!(keys(m.live(Table::Spans, 15, 20)), vec!["b", "d"]);
}

#[test]
fn every_change_advances_the_generation() {
    let mut m = Manifest::default();
    assert_eq!(m.generation(), 0);
    m.register("a", Table::Spans).unwrap();
    m.commit(seg("a", Table::Spans, 0, 1)).unwrap();
    assert_eq!(m.generation(), 2);
}

#[test]
fn the_sweep_spares_pending_and_live_objects() {
    let mut m = Manifest::default();
    m.register("pending", Table::Spans).unwrap();
    m.register("live", Table::Spans).unwrap();
    m.commit(seg("live", Table::Spans, 0, 1)).unwrap();
    let stored = ["pending", "live", "orphan"];
    assert_eq!(m.orphans(stored.iter().copied()), vec!["orphan"]);
}

#[test]
fn compaction_replaces_its_inputs_in_one_change() {
    let mut m = Manifest::default();
    for k in ["a", "b"] {
        m.register(k, Table::Spans).unwrap();
        m.commit(seg(k, Table::Spans, 0, 9)).unwrap();
    }
    m.register("ab", Table::Spans).unwrap();
    m.replace(&["a", "b"], seg("ab", Table::Spans, 0, 9))
        .unwrap();
    assert_eq!(keys(m.live(Table::Spans, 0, 9)), vec!["ab"]);
    assert_eq!(m.orphans(["a", "b", "ab"].into_iter()), vec!["a", "b"]);
}

#[test]
fn compaction_aborts_when_an_input_was_already_removed() {
    let mut m = Manifest::default();
    for k in ["a", "b"] {
        m.register(k, Table::Spans).unwrap();
        m.commit(seg(k, Table::Spans, 0, 9)).unwrap();
    }
    m.register("ab", Table::Spans).unwrap();
    assert_eq!(
        m.expire(Table::Spans, 10),
        vec!["a".to_string(), "b".to_string()]
    );
    assert_eq!(
        m.replace(&["a", "b"], seg("ab", Table::Spans, 0, 9)),
        Err(ManifestError::NotLive("a".into()))
    );
    assert_eq!(
        keys(m.live(Table::Spans, 0, i64::MAX)),
        Vec::<String>::new()
    );
    assert_eq!(m.orphans(["ab"].into_iter()), Vec::<&str>::new());
    m.abandon("ab");
    assert_eq!(m.orphans(["ab"].into_iter()), vec!["ab"]);
}

#[test]
fn retention_removes_segments_entirely_older_than_the_cutoff() {
    let mut m = Manifest::default();
    for (k, lo, hi) in [("old", 0, 9), ("straddle", 5, 15), ("new", 20, 29)] {
        m.register(k, Table::Spans).unwrap();
        m.commit(seg(k, Table::Spans, lo, hi)).unwrap();
    }
    assert_eq!(m.expire(Table::Spans, 10), vec!["old".to_string()]);
    assert_eq!(
        keys(m.live(Table::Spans, 0, i64::MAX)),
        vec!["new", "straddle"]
    );
}

#[test]
fn manifest_json_round_trips_the_stable_versioned_shape() {
    let mut m = Manifest::default();
    m.register("pending-metric", Table::MetricsGauge).unwrap();
    m.register("span-a", Table::Spans).unwrap();
    m.commit(seg("span-a", Table::Spans, 20, 30)).unwrap();

    let json = serde_json::to_string_pretty(&m).unwrap();
    assert_eq!(
        json,
        r#"{
  "version": 1,
  "generation": 3,
  "pending": {
    "pending-metric": "metrics_gauge"
  },
  "live": {
    "span-a": {
      "key": "span-a",
      "table": "spans",
      "min_time_us": 20,
      "max_time_us": 30,
      "rows": 10,
      "bytes": 100
    }
  }
}"#
    );
    let decoded: Manifest = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.generation(), 3);
    assert_eq!(
        keys(decoded.live(Table::Spans, 0, i64::MAX)),
        vec!["span-a"]
    );
    assert_eq!(
        decoded.orphans(["pending-metric", "span-a"].into_iter()),
        Vec::<&str>::new()
    );
}
