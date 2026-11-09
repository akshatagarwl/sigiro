use arrow::array::{Array, Int64Array, ListArray, StringArray, TimestampMicrosecondArray};
use arrow::record_batch::RecordBatch;
use otlp033::tonic::collector::profiles::v1development::ExportProfilesServiceRequest;
use otlp033::tonic::common::v1::{AnyValue, KeyValue, any_value};
use otlp033::tonic::profiles::v1development::{
    Function, Line, Location, Profile, ProfilesDictionary, ResourceProfiles, Sample, ScopeProfiles,
    Stack, ValueType,
};
use otlp033::tonic::resource::v1::Resource;
use prost::Message;
use sigiro_segments::profile_export::{
    ExportFrame, ExportProfile, ExportRequest, ExportSample, encode_profiles_request,
};
use sigiro_segments::{Table, transform_otlp_profiles, write_segment};

fn strings(batch: &RecordBatch, column: &str) -> Vec<String> {
    let array = batch
        .column_by_name(column)
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    (0..array.len())
        .map(|row| array.value(row).to_string())
        .collect()
}

fn stacks(batch: &RecordBatch) -> Vec<Vec<String>> {
    let array = batch
        .column_by_name("stack_trace")
        .unwrap()
        .as_any()
        .downcast_ref::<ListArray>()
        .unwrap();
    (0..array.len())
        .map(|row| {
            let frames = array.value(row);
            let frames = frames.as_any().downcast_ref::<StringArray>().unwrap();
            (0..frames.len())
                .map(|i| frames.value(i).to_string())
                .collect()
        })
        .collect()
}

fn values(batch: &RecordBatch) -> Vec<i64> {
    batch
        .column_by_name("value")
        .unwrap()
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap()
        .values()
        .to_vec()
}

fn otlp_request() -> Vec<u8> {
    let string_table = ["", "cpu", "nanoseconds", "main", "work"]
        .map(String::from)
        .to_vec();
    let function = |name: i32| Function {
        name_strindex: name,
        ..Default::default()
    };
    let location = |function: i32| Location {
        lines: vec![Line {
            function_index: function,
            ..Default::default()
        }],
        ..Default::default()
    };
    let dictionary = ProfilesDictionary {
        string_table,
        function_table: vec![function(0), function(3), function(4)],
        location_table: vec![Location::default(), location(1), location(2)],
        stack_table: vec![
            Stack::default(),
            Stack {
                location_indices: vec![2, 1],
            },
            Stack {
                location_indices: vec![1],
            },
        ],
        ..Default::default()
    };
    let profile = Profile {
        sample_type: Some(ValueType {
            type_strindex: 1,
            unit_strindex: 2,
        }),
        samples: vec![
            Sample {
                stack_index: 1,
                values: vec![10],
                ..Default::default()
            },
            Sample {
                stack_index: 2,
                values: vec![5],
                ..Default::default()
            },
        ],
        time_unix_nano: 1_700_000_000_000_000_000,
        ..Default::default()
    };
    let resource = Resource {
        attributes: vec![kv("service.name", "checkout")],
        ..Default::default()
    };
    ExportProfilesServiceRequest {
        resource_profiles: vec![ResourceProfiles {
            resource: Some(resource),
            scope_profiles: vec![ScopeProfiles {
                profiles: vec![profile],
                ..Default::default()
            }],
            ..Default::default()
        }],
        dictionary: Some(dictionary),
    }
    .encode_to_vec()
}

fn assert_cpu_rows(batch: &RecordBatch) {
    assert_eq!(strings(batch, "service_name"), ["checkout", "checkout"]);
    assert_eq!(stacks(batch), [vec!["main", "work"], vec!["main"]]);
    assert_eq!(values(batch), [10, 5]);
    let timestamps = batch
        .column_by_name("timestamp")
        .unwrap()
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .unwrap();
    assert_eq!(timestamps.value(0), 1_700_000_000_000_000);
}

#[test]
fn an_otlp_profile_becomes_one_row_per_sample_with_root_first_stacks() {
    let batch = transform_otlp_profiles(&otlp_request()).unwrap();
    assert_cpu_rows(&batch);
    let segment = write_segment(Table::Profiles, &batch).unwrap();
    assert_eq!(segment.metadata.rows, 2);
}

#[test]
fn a_profile_with_an_out_of_range_stack_is_rejected() {
    let mut request = ExportProfilesServiceRequest::decode(otlp_request().as_slice()).unwrap();
    request.resource_profiles[0].scope_profiles[0].profiles[0].samples[0].stack_index = 99;
    assert!(transform_otlp_profiles(&request.encode_to_vec()).is_err());
}

fn kv(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.into())),
        }),
        ..Default::default()
    }
}

/// A mutex profile with a labelled, trace-linked sample whose leaf location
/// has an inlined frame.
fn otlp_mutex_request() -> Vec<u8> {
    use otlp033::tonic::profiles::v1development::{KeyValueAndUnit, Link};
    let string_table = [
        "",
        "delay",
        "nanoseconds",
        "main",
        "lock",
        "inlined",
        "go.pprof.name",
        "endpoint",
    ]
    .map(String::from)
    .to_vec();
    let function = |name: i32| Function {
        name_strindex: name,
        ..Default::default()
    };
    let line = |function_index: i32| Line {
        function_index,
        ..Default::default()
    };
    let attr = |key: i32, value: &str| KeyValueAndUnit {
        key_strindex: key,
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.into())),
        }),
        ..Default::default()
    };
    let dictionary = ProfilesDictionary {
        string_table,
        function_table: vec![function(0), function(3), function(4), function(5)],
        location_table: vec![
            Location::default(),
            Location {
                lines: vec![line(1)],
                ..Default::default()
            },
            // `inlined` was inlined into `lock`.
            Location {
                lines: vec![line(3), line(2)],
                ..Default::default()
            },
        ],
        stack_table: vec![
            Stack::default(),
            Stack {
                location_indices: vec![2, 1],
            },
        ],
        attribute_table: vec![
            KeyValueAndUnit::default(),
            attr(6, "mutex"),
            attr(7, "/slow"),
        ],
        link_table: vec![
            Link::default(),
            Link {
                trace_id: vec![0x0a; 16],
                span_id: vec![0x0b; 8],
            },
        ],
        ..Default::default()
    };
    let profile = Profile {
        sample_type: Some(ValueType {
            type_strindex: 1,
            unit_strindex: 2,
        }),
        samples: vec![Sample {
            stack_index: 1,
            attribute_indices: vec![2],
            link_index: 1,
            values: vec![42],
            ..Default::default()
        }],
        attribute_indices: vec![1],
        time_unix_nano: 1_700_000_000_000_000_000,
        ..Default::default()
    };
    ExportProfilesServiceRequest {
        resource_profiles: vec![ResourceProfiles {
            resource: Some(Resource {
                attributes: vec![kv("service.name", "checkout"), kv("env", "prod")],
                ..Default::default()
            }),
            scope_profiles: vec![ScopeProfiles {
                profiles: vec![profile],
                ..Default::default()
            }],
            ..Default::default()
        }],
        dictionary: Some(dictionary),
    }
    .encode_to_vec()
}

#[test]
fn otlp_sample_attributes_links_inlined_frames_and_profile_kind_are_kept() {
    let batch = transform_otlp_profiles(&otlp_mutex_request()).unwrap();
    assert_eq!(strings(&batch, "profile_type"), ["mutex_duration"]);
    assert_eq!(stacks(&batch), [vec!["main", "lock", "inlined"]]);
    assert_eq!(strings(&batch, "labels"), [r#"{"endpoint":"/slow"}"#]);
    assert_eq!(
        strings(&batch, "trace_id"),
        ["0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a"]
    );
    assert_eq!(strings(&batch, "span_id"), ["0b0b0b0b0b0b0b0b"]);
    let resource: serde_json::Value =
        serde_json::from_str(&strings(&batch, "resource_attributes")[0]).unwrap();
    assert_eq!(resource["env"], "prod");
    assert_eq!(resource["service.name"], "checkout");
}

fn frame(function: &str, line: i64) -> ExportFrame {
    ExportFrame {
        function: function.into(),
        file: "src/main.rs".into(),
        line,
    }
}

#[test]
fn an_exported_profile_round_trips_through_the_otlp_transform() {
    let thread = |name: &str| vec![("thread.name".to_string(), name.to_string())];
    let request = ExportRequest {
        resource: vec![
            ("service.name".into(), "sigiro".into()),
            ("process.pid".into(), "42".into()),
        ],
        scope_name: "sigiro".into(),
        scope_version: "0.1.0".into(),
        profiles: vec![
            ExportProfile {
                sample_type: ("cpu".into(), "nanoseconds".into()),
                period_type: ("cpu".into(), "nanoseconds".into()),
                period: 10_000_000,
                time_unix_nano: 1_700_000_000_000_000_000,
                duration_nano: 10_000_000_000,
                samples: vec![
                    // Leaf first, with `inlined` inlined into `work`.
                    ExportSample {
                        frames: vec![frame("inlined", 7), frame("work", 3), frame("main", 1)],
                        value: 30_000_000,
                        attributes: thread("worker-1"),
                    },
                    ExportSample {
                        frames: vec![frame("work", 4), frame("main", 1)],
                        value: 20_000_000,
                        attributes: thread("worker-2"),
                    },
                    // Zero samples carry nothing and are not sent.
                    ExportSample {
                        frames: vec![frame("main", 1)],
                        value: 0,
                        attributes: Vec::new(),
                    },
                ],
            },
            ExportProfile {
                sample_type: ("inuse_space".into(), "bytes".into()),
                period_type: ("space".into(), "bytes".into()),
                period: 524_288,
                time_unix_nano: 1_700_000_000_000_000_000,
                duration_nano: 0,
                samples: vec![ExportSample {
                    frames: vec![frame("alloc", 9), frame("main", 1)],
                    value: 4096,
                    attributes: Vec::new(),
                }],
            },
        ],
    };
    let bytes = encode_profiles_request(&request);
    let decoded = ExportProfilesServiceRequest::decode(bytes.as_slice()).unwrap();
    let dictionary = decoded.dictionary.as_ref().unwrap();
    // The two `main` frames at line 1 share one location.
    assert_eq!(dictionary.location_table.len(), 1 + 5);

    let batch = transform_otlp_profiles(&bytes).unwrap();
    assert_eq!(
        strings(&batch, "profile_type"),
        ["cpu", "cpu", "inuse_space"]
    );
    assert_eq!(
        strings(&batch, "unit"),
        ["nanoseconds", "nanoseconds", "bytes"]
    );
    assert_eq!(
        stacks(&batch),
        [
            vec!["main", "work", "inlined"],
            vec!["main", "work"],
            vec!["main", "alloc"]
        ]
    );
    assert_eq!(values(&batch), [30_000_000, 20_000_000, 4096]);
    assert_eq!(strings(&batch, "service_name"), ["sigiro"; 3]);
    assert_eq!(
        strings(&batch, "labels")[..2],
        [
            r#"{"thread.name":"worker-1"}"#,
            r#"{"thread.name":"worker-2"}"#
        ]
    );
    let resource: serde_json::Value =
        serde_json::from_str(&strings(&batch, "resource_attributes")[0]).unwrap();
    assert_eq!(resource["process.pid"], "42");
    let timestamps = batch
        .column_by_name("timestamp")
        .unwrap()
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .unwrap();
    assert_eq!(timestamps.value(0), 1_700_000_000_000_000);
}
