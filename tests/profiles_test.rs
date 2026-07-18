//! Tests for OTLP Profiles: dictionary resolution, inserter pipeline, investigate queries.

use arrow::array::{Int64Array, ListBuilder, StringArray, StringBuilder};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use prost::Message;
use sigiro::IngestMsg;
use sigiro::inserter::MsgRouter;
use sigiro::investigate::QueryCtx;
use sigiro::pool::ProjectPool;
use std::sync::{Arc, Once};

static EXTENSION_ENV: Once = Once::new();

fn test_extension_dir() -> String {
    std::env::var("SIGIRO_EXTENSION_DIR").unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|home| format!("{home}/.duckdb/extensions"))
            .unwrap_or_else(|_| "/tmp/sigiro-test-extensions".to_string())
    })
}

fn test_pool(dir: &std::path::Path) -> Arc<ProjectPool> {
    EXTENSION_ENV.call_once(|| {
        // SAFETY: all tests in this binary use the same read-only extension cache.
        unsafe {
            std::env::set_var("SIGIRO_EXTENSION_DIR", test_extension_dir());
        }
    });
    Arc::new(ProjectPool::new(
        camino::Utf8PathBuf::from_path_buf(dir.to_path_buf()).unwrap(),
        10,
        sigiro::storage::StorageMode::LocalFile,
    ))
}

/// Build a profiles RecordBatch manually (simulating what transform_profiles produces).
fn make_profiles_batch(
    service_name: &str,
    ts_base: i64,
    stacks: &[(&[&str], i64, &str, &str)], // (stack_frames, value, profile_type, unit)
) -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![
        Field::new("profile_id", DataType::Utf8, true),
        Field::new("trace_id", DataType::Utf8, true),
        Field::new("span_id", DataType::Utf8, true),
        Field::new("service_name", DataType::Utf8, false),
        Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Microsecond, None),
            false,
        ),
        Field::new("profile_type", DataType::Utf8, true),
        Field::new(
            "stack_trace",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            true,
        ),
        Field::new("value", DataType::Int64, true),
        Field::new("unit", DataType::Utf8, true),
        Field::new("resource_attributes", DataType::Utf8, true),
    ]));

    let mut profile_ids = Vec::new();
    let mut trace_ids: Vec<Option<&str>> = Vec::new();
    let mut span_ids: Vec<Option<&str>> = Vec::new();
    let mut service_names = Vec::new();
    let mut timestamps = Vec::new();
    let mut profile_types = Vec::new();
    let mut values = Vec::new();
    let mut units = Vec::new();
    let mut resource_attrs: Vec<Option<&str>> = Vec::new();

    let mut stack_traces = ListBuilder::new(StringBuilder::new()).with_field(Arc::new(Field::new(
        "item",
        DataType::Utf8,
        true,
    )));

    for (i, (frames, value, ptype, unit)) in stacks.iter().enumerate() {
        profile_ids.push("test-profile-001");
        trace_ids.push(None);
        span_ids.push(None);
        service_names.push(service_name);
        timestamps.push(ts_base + i as i64);
        profile_types.push(*ptype);
        values.push(*value);
        units.push(*unit);
        resource_attrs.push(None);

        for frame in *frames {
            stack_traces.values().append_value(*frame);
        }
        stack_traces.append(true);
    }

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(profile_ids)),
            Arc::new(StringArray::from(trace_ids)),
            Arc::new(StringArray::from(span_ids)),
            Arc::new(StringArray::from(service_names)),
            Arc::new(arrow::array::TimestampMicrosecondArray::from(timestamps)),
            Arc::new(StringArray::from(profile_types)),
            Arc::new(stack_traces.finish()),
            Arc::new(Int64Array::from(values)),
            Arc::new(StringArray::from(units)),
            Arc::new(StringArray::from(resource_attrs)),
        ],
    )
    .unwrap()
}

fn build_linked_otlp_profile_request(
    service_name: &str,
    timestamp_ns: u64,
    trace_id: Vec<u8>,
    span_id: Vec<u8>,
) -> Vec<u8> {
    use opentelemetry_proto::tonic::collector::profiles::v1development::ExportProfilesServiceRequest;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
    use opentelemetry_proto::tonic::profiles::v1development::{
        Function, Line, Link, Location, Profile, ProfilesDictionary, ResourceProfiles, Sample,
        ScopeProfiles, Stack, ValueType,
    };
    use opentelemetry_proto::tonic::resource::v1::Resource;

    let request = ExportProfilesServiceRequest {
        resource_profiles: vec![ResourceProfiles {
            resource: Some(Resource {
                attributes: vec![KeyValue {
                    key: "service.name".to_string(),
                    value: Some(AnyValue {
                        value: Some(any_value::Value::StringValue(service_name.to_string())),
                    }),
                }],
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_profiles: vec![ScopeProfiles {
                scope: None,
                profiles: vec![Profile {
                    sample_type: Some(ValueType {
                        type_strindex: 1,
                        unit_strindex: 2,
                        aggregation_temporality: 0,
                    }),
                    sample: vec![Sample {
                        stack_index: 1,
                        values: vec![42],
                        attribute_indices: Vec::new(),
                        link_index: 1,
                        timestamps_unix_nano: Vec::new(),
                    }],
                    time_unix_nano: timestamp_ns,
                    duration_nano: 1_000_000_000,
                    period_type: None,
                    period: 0,
                    comment_strindices: Vec::new(),
                    profile_id: vec![0x33; 16],
                    dropped_attributes_count: 0,
                    original_payload_format: String::new(),
                    original_payload: Vec::new(),
                    attribute_indices: Vec::new(),
                }],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
        dictionary: Some(ProfilesDictionary {
            mapping_table: Vec::new(),
            location_table: vec![
                Location::default(),
                Location {
                    line: vec![Line {
                        function_index: 1,
                        line: 0,
                        column: 0,
                    }],
                    ..Default::default()
                },
                Location {
                    line: vec![Line {
                        function_index: 2,
                        line: 0,
                        column: 0,
                    }],
                    ..Default::default()
                },
            ],
            function_table: vec![
                Function::default(),
                Function {
                    name_strindex: 3,
                    ..Default::default()
                },
                Function {
                    name_strindex: 4,
                    ..Default::default()
                },
            ],
            link_table: vec![
                Link {
                    trace_id: Vec::new(),
                    span_id: Vec::new(),
                },
                Link { trace_id, span_id },
            ],
            string_table: vec![
                String::new(),
                "cpu".to_string(),
                "nanoseconds".to_string(),
                "main".to_string(),
                "handler".to_string(),
            ],
            attribute_table: Vec::new(),
            stack_table: vec![
                Stack::default(),
                Stack {
                    location_indices: vec![1, 2],
                },
            ],
        }),
    };

    request.encode_to_vec()
}

// ---------------------------------------------------------------------------
// E2E: profiles through inserter → DuckDB → investigate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_profiles_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());
    let router = MsgRouter::new(pool.clone());
    router.start();

    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };

    // Send CPU profile samples with known stack traces
    let stacks = vec![
        (
            &["main", "handle_request", "db_query"][..],
            15_000_000i64,
            "cpu",
            "nanoseconds",
        ),
        (
            &["main", "handle_request", "serialize"],
            8_000_000,
            "cpu",
            "nanoseconds",
        ),
        (
            &["main", "handle_request", "db_query"],
            12_000_000,
            "cpu",
            "nanoseconds",
        ),
        (&["main", "gc_collect"], 3_000_000, "cpu", "nanoseconds"),
    ];

    router
        .send(IngestMsg::Profiles {
            project_id: "profile_test".to_string(),
            batch: make_profiles_batch("api-svc", now_us, &stacks),
        })
        .await
        .unwrap();

    router.shutdown_and_wait().await.unwrap();

    // Verify rows landed in DuckDB
    let conn = pool.create_project("profile_test").unwrap();
    let conn = conn.get();

    let total: i64 = conn
        .query_row("SELECT count(*) FROM lake.sigiro_profiles", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(total, 4, "should have 4 profile samples");

    // Verify stack traces are stored correctly
    let db_query_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM lake.sigiro_profiles WHERE list_contains(stack_trace, 'db_query')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(db_query_count, 2, "should have 2 samples with db_query");

    // Verify profile_type stored
    let cpu_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM lake.sigiro_profiles WHERE profile_type = 'cpu'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cpu_count, 4, "all 4 samples should be cpu type");

    // Verify via investigate query: top functions
    let qctx = QueryCtx {
        conn: &conn,
        service: "api-svc",
        from_ts: now_us - 60_000_000,
        to_ts: now_us + 60_000_000,
        prefix: "lake.",
    };
    let profiles = sigiro::investigate::query_profiles(&qctx, 20).unwrap();
    assert!(!profiles.is_empty(), "should have profile summaries");

    // db_query should have highest total value (15M + 12M = 27M)
    let db_query_summary = profiles.iter().find(|p| p.function == "db_query");
    assert!(
        db_query_summary.is_some(),
        "db_query should appear in profile summaries"
    );
    assert_eq!(
        db_query_summary.unwrap().total_value,
        27_000_000,
        "db_query total should be 27M ns"
    );
}

#[tokio::test]
async fn e2e_otlp_profiles_store_sample_trace_span_link() {
    use axum::http::StatusCode;
    use axum_test::TestServer;

    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());
    let router = Arc::new(MsgRouter::new(pool.clone()));
    router.start();
    let app = sigiro::ingest::otlp_http_router(router, sigiro::auth::AuthMode::Open, 8_388_608);
    let server = TestServer::new(app).unwrap();

    let trace_id = vec![
        0x11, 0x11, 0x11, 0x11, 0x22, 0x22, 0x22, 0x22, 0x33, 0x33, 0x33, 0x33, 0x44, 0x44, 0x44,
        0x44,
    ];
    let span_id = vec![0xaa, 0xaa, 0xbb, 0xbb, 0xcc, 0xcc, 0xdd, 0xdd];
    let request = build_linked_otlp_profile_request(
        "linked-svc",
        time::OffsetDateTime::now_utc().unix_timestamp_nanos() as u64,
        trace_id.clone(),
        span_id.clone(),
    );

    let response = server
        .post("/v1/profiles")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .content_type("application/x-protobuf")
        .bytes(request.into())
        .await;
    response.assert_status_ok();

    let shared_conn = pool.get_project("default").unwrap();
    let expected_trace_id = hex::encode(&trace_id);
    let expected_span_id = hex::encode(&span_id);
    let mut linked_rows = 0;
    for _ in 0..50 {
        linked_rows = shared_conn
            .get()
            .query_row(
                "SELECT count(*) FROM lake.sigiro_profiles
                 WHERE service_name = 'linked-svc'
                   AND trace_id = ?1
                   AND span_id = ?2
                   AND stack_trace[1] = 'main'
                   AND stack_trace[-1] = 'handler'",
                duckdb::params![&expected_trace_id, &expected_span_id],
                |row| row.get(0),
            )
            .unwrap();
        if linked_rows == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(linked_rows, 1, "OTLP profile sample link should be stored");

    let invalid_request = build_linked_otlp_profile_request(
        "linked-svc",
        time::OffsetDateTime::now_utc().unix_timestamp_nanos() as u64,
        vec![0x11; 15],
        vec![0x22; 8],
    );
    let response = server
        .post("/v1/profiles")
        .add_header("Authorization", "Bearer sigiro_dev_test")
        .content_type("application/x-protobuf")
        .bytes(invalid_request.into())
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);

    let rows_after_rejection: i64 = shared_conn
        .get()
        .query_row("SELECT count(*) FROM lake.sigiro_profiles", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        rows_after_rejection, 1,
        "malformed profile links must not be ingested"
    );
}

// ---------------------------------------------------------------------------
// Multi-type profiles (CPU + heap in same project)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_profiles_multi_type() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());
    let router = MsgRouter::new(pool.clone());
    router.start();

    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };

    // CPU samples
    let cpu_stacks = vec![(
        &["main", "compute"][..],
        50_000_000i64,
        "cpu",
        "nanoseconds",
    )];
    // Heap samples
    let heap_stacks = vec![(
        &["main", "allocate_buffer"][..],
        1_048_576i64,
        "alloc_space",
        "bytes",
    )];

    router
        .send(IngestMsg::Profiles {
            project_id: "multi_type".to_string(),
            batch: make_profiles_batch("api-svc", now_us, &cpu_stacks),
        })
        .await
        .unwrap();
    router
        .send(IngestMsg::Profiles {
            project_id: "multi_type".to_string(),
            batch: make_profiles_batch("api-svc", now_us + 100, &heap_stacks),
        })
        .await
        .unwrap();

    router.shutdown_and_wait().await.unwrap();

    let conn = pool.create_project("multi_type").unwrap();
    let conn = conn.get();

    // Should have both profile types
    let type_count: i64 = conn
        .query_row(
            "SELECT count(DISTINCT profile_type) FROM lake.sigiro_profiles",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(type_count, 2, "should have 2 distinct profile types");

    // Query profiles — should see both types
    let qctx = QueryCtx {
        conn: &conn,
        service: "api-svc",
        from_ts: now_us - 60_000_000,
        to_ts: now_us + 60_000_000,
        prefix: "lake.",
    };
    let profiles = sigiro::investigate::query_profiles(&qctx, 20).unwrap();
    assert_eq!(
        profiles.len(),
        2,
        "should have 2 profile summaries (cpu + heap)"
    );

    let cpu = profiles.iter().find(|p| p.profile_type == "cpu");
    let heap = profiles.iter().find(|p| p.profile_type == "alloc_space");
    assert!(cpu.is_some(), "should have cpu profile");
    assert!(heap.is_some(), "should have alloc_space profile");
}

// ---------------------------------------------------------------------------
// Empty profiles batch doesn't crash
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_profiles_empty_batch() {
    let dir = tempfile::tempdir().unwrap();
    let pool = test_pool(dir.path());
    let router = MsgRouter::new(pool.clone());
    router.start();

    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64
    };

    // Empty profiles batch
    router
        .send(IngestMsg::Profiles {
            project_id: "empty_profiles".to_string(),
            batch: make_profiles_batch("api-svc", now_us, &[]),
        })
        .await
        .unwrap();

    router.shutdown_and_wait().await.unwrap();

    let conn = pool.create_project("empty_profiles").unwrap();
    let conn = conn.get();

    let total: i64 = conn
        .query_row("SELECT count(*) FROM lake.sigiro_profiles", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(total, 0, "should have 0 profile samples");
}
