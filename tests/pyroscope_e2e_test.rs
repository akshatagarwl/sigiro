use prost::Message;
use sigiro::IngestMsg;
use sigiro::inserter::MsgRouter;
use sigiro::pool::ProjectPool;
use std::io::Write;
use std::sync::Arc;

fn build_pyroscope_push_request(
    service_name: &str,
    timestamp_ns: i64,
    frames: &[&str],
    sample_types: &[(&str, &str)],
    sample_values: &[i64],
) -> Vec<u8> {
    use pyroscope::encode::r#gen::google::{Function, Line, Location, Profile, Sample, ValueType};
    use pyroscope::encode::r#gen::push::{PushRequest, RawProfileSeries, RawSample};
    use pyroscope::encode::r#gen::types::LabelPair;

    let mut string_table = vec![String::new()];

    fn intern(string_table: &mut Vec<String>, value: &str) -> i64 {
        if let Some(index) = string_table.iter().position(|entry| entry == value) {
            index as i64
        } else {
            string_table.push(value.to_string());
            (string_table.len() - 1) as i64
        }
    }

    let sample_types = sample_types
        .iter()
        .map(|(profile_type, unit)| ValueType {
            r#type: intern(&mut string_table, profile_type),
            unit: intern(&mut string_table, unit),
        })
        .collect();

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
        value: sample_values.to_vec(),
        ..Default::default()
    };

    let profile = Profile {
        sample_type: sample_types,
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
    encoder.write_all(&encoded).expect("gzip write");
    encoder.finish().expect("gzip finish");

    gzipped
}

// ---------------------------------------------------------------------------
// E2E: Pyroscope native pipeline → transform → MsgRouter → DuckDB
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_pyroscope_native_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(
        camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap(),
        10,
    ));
    let router = MsgRouter::new(pool.clone());
    router.start();

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
        &[("cpu", "nanoseconds")],
        &[15_000_000],
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

    router.shutdown_and_wait().await.unwrap();

    // Verify in DuckDB
    let conn = pool.create_project("pyro_test").unwrap();
    let conn = conn.lock();

    let total: i64 = conn
        .query_row("SELECT count(*) FROM lake.sigiro_profiles", [], |r| {
            r.get(0)
        })
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
        .query_row("SELECT unit FROM lake.sigiro_profiles LIMIT 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(unit, "nanoseconds");

    // Verify value
    let val: i64 = conn
        .query_row("SELECT value FROM lake.sigiro_profiles LIMIT 1", [], |r| {
            r.get(0)
        })
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
    assert_eq!(
        batch.num_rows(),
        0,
        "empty PushRequest should produce 0 rows"
    );
}

// ---------------------------------------------------------------------------
// E2E: Multiple series and multiple samples in one PushRequest
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_pyroscope_multi_series() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(
        camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap(),
        10,
    ));
    let router = MsgRouter::new(pool.clone());
    router.start();

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
        &[("cpu", "nanoseconds")],
        &[10_000_000],
    );

    // Series 2: service_b with one sample
    let bytes_b = build_pyroscope_push_request(
        "service_b",
        now_ns + 1_000_000_000, // +1s
        &["worker", "run_task"],
        &[("cpu", "nanoseconds")],
        &[20_000_000],
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

    router.shutdown_and_wait().await.unwrap();

    let conn = pool.create_project("pyro_multi").unwrap();
    let conn = conn.lock();

    let total: i64 = conn
        .query_row("SELECT count(*) FROM lake.sigiro_profiles", [], |r| {
            r.get(0)
        })
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

#[tokio::test]
async fn e2e_pyroscope_heap_profile_http_preserves_every_sample_type() {
    use axum::http::StatusCode;
    use axum_test::TestServer;

    let dir = tempfile::tempdir().unwrap();
    let pool = Arc::new(ProjectPool::new(
        camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap(),
        10,
    ));
    let router = Arc::new(MsgRouter::new(pool.clone()));
    router.start();
    let app = sigiro::ingest::otlp_http_router(
        router,
        sigiro::auth::AuthMode::Key {
            key: "sigiro_dev_test".to_string(),
        },
    );
    let server = TestServer::new(app).unwrap();

    let now_ns = time::OffsetDateTime::now_utc().unix_timestamp_nanos() as i64;
    let profile = build_pyroscope_push_request(
        "heap-svc",
        now_ns,
        &["main", "handle_request", "allocate_buffer"],
        &[
            ("alloc_objects", "count"),
            ("alloc_space", "bytes"),
            ("inuse_objects", "count"),
            ("inuse_space", "bytes"),
        ],
        &[40, 4096, 20, 2048],
    );

    let response = server
        .post("/push.v1.PusherService/Push")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .content_type("application/proto")
        .bytes(profile.into())
        .await;
    response.assert_status_ok();

    let shared_conn = pool.get_project("default").unwrap();
    let mut stored_rows = 0;
    for _ in 0..50 {
        stored_rows = shared_conn
            .lock()
            .query_row("SELECT count(*) FROM lake.sigiro_profiles", [], |row| {
                row.get(0)
            })
            .unwrap();
        if stored_rows == 4 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(stored_rows, 4, "every heap sample type must be stored");

    {
        let conn = shared_conn.lock();
        for (profile_type, unit, value) in [
            ("alloc_objects", "count", 40),
            ("alloc_space", "bytes", 4096),
            ("inuse_objects", "count", 20),
            ("inuse_space", "bytes", 2048),
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM lake.sigiro_profiles
                     WHERE service_name = ?1 AND profile_type = ?2 AND unit = ?3 AND value = ?4
                       AND stack_trace[1] = 'main' AND stack_trace[-1] = 'allocate_buffer'",
                    duckdb::params!["heap-svc", profile_type, unit, value],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "missing {profile_type}/{unit} heap sample");
        }
    }

    let invalid_profile = build_pyroscope_push_request(
        "heap-svc",
        now_ns,
        &["main", "allocate_buffer"],
        &[("alloc_objects", "count"), ("alloc_space", "bytes")],
        &[1],
    );
    let response = server
        .post("/push.v1.PusherService/Push")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .content_type("application/proto")
        .bytes(invalid_profile.into())
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);

    let rows_after_rejection: i64 = shared_conn
        .lock()
        .query_row("SELECT count(*) FROM lake.sigiro_profiles", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        rows_after_rejection, 4,
        "invalid profiles must not be ingested"
    );
}
