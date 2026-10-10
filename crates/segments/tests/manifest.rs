use sigiro_segments::{Manifest, ManifestError, Segment, Signal};

fn seg(key: &str, signal: Signal, min_us: i64, max_us: i64) -> Segment {
    Segment {
        key: key.into(),
        signal,
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
    m.register("a", Signal::Spans).unwrap();
    assert_eq!(
        keys(m.live(Signal::Spans, 0, i64::MAX)),
        Vec::<String>::new()
    );
    m.commit(seg("a", Signal::Spans, 10, 20)).unwrap();
    assert_eq!(keys(m.live(Signal::Spans, 0, i64::MAX)), vec!["a"]);
}

#[test]
fn committing_an_unregistered_segment_is_rejected() {
    let mut m = Manifest::default();
    assert_eq!(
        m.commit(seg("a", Signal::Spans, 10, 20)),
        Err(ManifestError::NotPending("a".into()))
    );
}

#[test]
fn a_key_cannot_be_registered_twice() {
    let mut m = Manifest::default();
    m.register("a", Signal::Spans).unwrap();
    assert_eq!(
        m.register("a", Signal::Spans),
        Err(ManifestError::KeyInUse("a".into()))
    );
    m.commit(seg("a", Signal::Spans, 10, 20)).unwrap();
    assert_eq!(
        m.register("a", Signal::Spans),
        Err(ManifestError::KeyInUse("a".into()))
    );
}

#[test]
fn live_segments_are_filtered_by_signal_and_time_overlap() {
    let mut m = Manifest::default();
    for (k, sig, lo, hi) in [
        ("a", Signal::Spans, 0, 9),
        ("b", Signal::Spans, 10, 19),
        ("c", Signal::Logs, 10, 19),
        ("d", Signal::Spans, 20, 29),
    ] {
        m.register(k, sig).unwrap();
        m.commit(seg(k, sig, lo, hi)).unwrap();
    }
    assert_eq!(keys(m.live(Signal::Spans, 15, 20)), vec!["b", "d"]);
}

#[test]
fn every_change_advances_the_generation() {
    let mut m = Manifest::default();
    assert_eq!(m.generation(), 0);
    m.register("a", Signal::Spans).unwrap();
    m.commit(seg("a", Signal::Spans, 0, 1)).unwrap();
    assert_eq!(m.generation(), 2);
}

#[test]
fn the_sweep_spares_pending_and_live_objects() {
    let mut m = Manifest::default();
    m.register("pending", Signal::Spans).unwrap();
    m.register("live", Signal::Spans).unwrap();
    m.commit(seg("live", Signal::Spans, 0, 1)).unwrap();
    let stored = ["pending", "live", "orphan"];
    assert_eq!(m.orphans(stored.iter().copied()), vec!["orphan"]);
}

#[test]
fn compaction_replaces_its_inputs_in_one_change() {
    let mut m = Manifest::default();
    for k in ["a", "b"] {
        m.register(k, Signal::Spans).unwrap();
        m.commit(seg(k, Signal::Spans, 0, 9)).unwrap();
    }
    m.register("ab", Signal::Spans).unwrap();
    m.replace(&["a", "b"], seg("ab", Signal::Spans, 0, 9))
        .unwrap();
    assert_eq!(keys(m.live(Signal::Spans, 0, 9)), vec!["ab"]);
    assert_eq!(m.orphans(["a", "b", "ab"].into_iter()), vec!["a", "b"]);
}

#[test]
fn compaction_aborts_when_an_input_was_already_removed() {
    let mut m = Manifest::default();
    for k in ["a", "b"] {
        m.register(k, Signal::Spans).unwrap();
        m.commit(seg(k, Signal::Spans, 0, 9)).unwrap();
    }
    m.register("ab", Signal::Spans).unwrap();
    assert_eq!(
        m.expire(Signal::Spans, 10),
        vec!["a".to_string(), "b".to_string()]
    );
    assert_eq!(
        m.replace(&["a", "b"], seg("ab", Signal::Spans, 0, 9)),
        Err(ManifestError::NotLive("a".into()))
    );
    assert_eq!(
        keys(m.live(Signal::Spans, 0, i64::MAX)),
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
        m.register(k, Signal::Spans).unwrap();
        m.commit(seg(k, Signal::Spans, lo, hi)).unwrap();
    }
    assert_eq!(m.expire(Signal::Spans, 10), vec!["old".to_string()]);
    assert_eq!(
        keys(m.live(Signal::Spans, 0, i64::MAX)),
        vec!["new", "straddle"]
    );
}
