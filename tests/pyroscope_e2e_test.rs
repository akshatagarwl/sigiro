use prost::Message;
use sigiro::inserter::MsgRouter;
use sigiro::pool::ProjectPool;
use sigiro::IngestMsg;
use std::io::Write;
use std::sync::Arc;

fn build_pyroscope_push_request(
    service_name: &str,
    timestamp_ns: i64,
    frames: &[&str],
    sample_value: i64,
) -> Vec<u8> {
    use pyroscope::encode::r#gen::google::{
        Function, Line, Location, Profile, Sample, ValueType,
    };
    use pyroscope::encode::r#gen::push::{PushRequest, RawProfileSeries, RawSample};
    use pyroscope::encode::r#gen::types::LabelPair;

    // pprof string_table: [0]="", [1]="cpu", [2]="nanoseconds", [3..]=frame names
    let mut string_table = vec![String::new()];
    string_table.push("cpu".to_string());
    string_table.push("nanoseconds".to_string());

    let frame_base = string_table.len() as i64;
    for frame in frames {
        string_table.push(frame.to_string());
    }

    let functions: Vec<Function> = frames
        .iter()
        .enumerate()
        .map(|(i, _)| Function {
            id: (i as u64) + 1,
            name: frame_base + i as i64,
            ..Default::default()
        })
        .collect();

    let locations: Vec<Location> = frames
        .iter()
        .enumerate()
        .map(|(i, _)| Location {
            id: (i as u64) + 1,
            line: vec![Line {
                function_id: (i as u64) + 1,
                ..Default::default()
            }],
            ..Default::default()
        })
        .collect();

    // pprof: location_ids are leaf-first (callee → caller)
    let location_ids: Vec<u64> = (1..=frames.len() as u64).rev().collect();
    let sample = Sample {
        location_id: location_ids,
        value: vec![sample_value],
        ..Default::default()
    };

    let profile = Profile {
        sample_type: vec![ValueType {
            r#type: 1, // string_table[1] = "cpu"
            unit: 2,   // string_table[2] = "nanoseconds"
        }],
        sample: vec![sample],
        location: locations,
        function: functions,
        string_table,
        time_nanos: timestamp_ns,
        ..Default::default()
    };

    let mut raw_profile = Vec::new();
    profile
        .encode(&mut raw_profile)
        .expect("encode pprof profile");

    let push_request = PushRequest {
        series: vec![RawProfileSeries {
            labels: vec![LabelPair {
                name: "service_name".to_string(),
                value: service_name.to_string(),
            }],
            samples: vec![RawSample {
                raw_profile,
                id: "test-pyro-001".to_string(),
            }],
        }],
    };

    let mut encoded = Vec::new();
    push_request
        .encode(&mut encoded)
        .expect("encode PushRequest");

    let mut gzipped = Vec::new();
    let mut encoder = flate2::write::GzEncoder::new(&mut gzipped, flate2::Compression::default());
    encoder
        .write_all(&encoded)
        .expect("gzip write");
    encoder.finish().expect("gzip finish");

    gzipped
}

// ---------------------------------------------------------------------------
// E2E: Pyroscope native pipeline → transform → MsgRouter → DuckDB
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_pyroscope_native_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(dir.path().to_path_buf(), 10));
    let router = MsgRouter::new(pool.clone());

    let now_ns = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000_000 + (odt.microsecond() as i64) * 1_000
    };

    // Explicit DI: initialize project and register flusher
    let _ = pool.create_project("pyro_test").unwrap();
    router.register_flusher("pyro_test").unwrap();

    // Build gzip-compressed PushRequest with known stack trace
    let bytes = build_pyroscope_push_request(
        "pyro-svc",
        now_ns,
        &["main", "handle_req", "db_query"],
        15_000_000,
    );

    // Call transform directly (bytes → Arrow RecordBatch)
    let batch = sigiro::ingest::transform_pyroscope_profiles(bytes)
        .await
        .unwrap();

    assert_eq!(batch.num_rows(), 1, "transform should produce 1 row");

    // Send through the router
    router
        .send(IngestMsg::Profiles {
            project_id: "pyro_test".to_string(),
            batch,
        })
        .await
        .unwrap();

    router.shutdown_and_wait().await;

    // Verify in DuckDB
    let conn = pool.create_project("pyro_test").unwrap();
    let conn = conn.lock().unwrap();

    let total: i64 = conn
        .query_row("SELECT count(*) FROM lake.sigiro_profiles", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 1, "should have 1 profile sample");

    // Verify stack trace contains "db_query"
    let db_query_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM lake.sigiro_profiles WHERE list_contains(stack_trace, 'db_query')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(db_query_count, 1, "stack should contain db_query");

    // Verify service_name
    let svc: String = conn
        .query_row(
            "SELECT service_name FROM lake.sigiro_profiles LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(svc, "pyro-svc");

    // Verify profile_type
    let ptype: String = conn
        .query_row(
            "SELECT profile_type FROM lake.sigiro_profiles LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ptype, "cpu");

    // Verify unit
    let unit: String = conn
        .query_row(
            "SELECT unit FROM lake.sigiro_profiles LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unit, "nanoseconds");

    // Verify value
    let val: i64 = conn
        .query_row(
            "SELECT value FROM lake.sigiro_profiles LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(val, 15_000_000);

    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM lake.sigiro_profiles WHERE list_contains(stack_trace, 'db_query') AND list_contains(stack_trace, 'handle_req') AND list_contains(stack_trace, 'main')",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap(),
        1,
        "stack should contain all 3 frames"
    );
}

// ---------------------------------------------------------------------------
// E2E: Pyroscope with empty PushRequest produces zero rows (no crash)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_pyroscope_empty_push_request() {
    use pyroscope::encode::r#gen::push::PushRequest;
    use std::io::Write as IoWrite;

    // Build an empty PushRequest (no series)
    let push_request = PushRequest { series: vec![] };
    let mut encoded = Vec::new();
    push_request.encode(&mut encoded).unwrap();

    let mut gzipped = Vec::new();
    let mut encoder = flate2::write::GzEncoder::new(&mut gzipped, flate2::Compression::default());
    encoder.write_all(&encoded).unwrap();
    encoder.finish().unwrap();

    let batch = sigiro::ingest::transform_pyroscope_profiles(gzipped)
        .await
        .unwrap();
    assert_eq!(batch.num_rows(), 0, "empty PushRequest should produce 0 rows");
}

// ---------------------------------------------------------------------------
// E2E: Multiple series and multiple samples in one PushRequest
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_pyroscope_multi_series() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(dir.path().to_path_buf(), 10));
    let router = MsgRouter::new(pool.clone());

    let now_ns = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000_000 + (odt.microsecond() as i64) * 1_000
    };

    let _ = pool.create_project("pyro_multi").unwrap();
    router.register_flusher("pyro_multi").unwrap();

    // Series 1: service_a with one sample
    let bytes_a = build_pyroscope_push_request(
        "service_a",
        now_ns,
        &["main", "process"],
        10_000_000,
    );

    // Series 2: service_b with one sample
    let bytes_b = build_pyroscope_push_request(
        "service_b",
        now_ns + 1_000_000_000, // +1s
        &["worker", "run_task"],
        20_000_000,
    );

    // Process each through transform
    let batch_a = sigiro::ingest::transform_pyroscope_profiles(bytes_a)
        .await
        .unwrap();
    let batch_b = sigiro::ingest::transform_pyroscope_profiles(bytes_b)
        .await
        .unwrap();

    assert_eq!(batch_a.num_rows(), 1);
    assert_eq!(batch_b.num_rows(), 1);

    router
        .send(IngestMsg::Profiles {
            project_id: "pyro_multi".to_string(),
            batch: batch_a,
        })
        .await
        .unwrap();
    router
        .send(IngestMsg::Profiles {
            project_id: "pyro_multi".to_string(),
            batch: batch_b,
        })
        .await
        .unwrap();

    router.shutdown_and_wait().await;

    let conn = pool.create_project("pyro_multi").unwrap();
    let conn = conn.lock().unwrap();

    let total: i64 = conn
        .query_row("SELECT count(*) FROM lake.sigiro_profiles", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 2, "should have 2 profile samples");

    // Verify each service
    let svc_a: i64 = conn
        .query_row(
            "SELECT count(*) FROM lake.sigiro_profiles WHERE service_name = 'service_a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(svc_a, 1);

    let svc_b: i64 = conn
        .query_row(
            "SELECT count(*) FROM lake.sigiro_profiles WHERE service_name = 'service_b'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(svc_b, 1);
}
