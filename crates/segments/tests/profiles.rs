use std::io::Write;

use arrow::array::{Array, Int64Array, ListArray, StringArray, TimestampMicrosecondArray};
use arrow::record_batch::RecordBatch;
use otlp031::tonic::collector::profiles::v1development::ExportProfilesServiceRequest;
use otlp031::tonic::common::v1::{AnyValue, KeyValue, any_value};
use otlp031::tonic::profiles::v1development::{
    Function, Line, Location, Profile, ProfilesDictionary, ResourceProfiles, Sample, ScopeProfiles,
    Stack, ValueType,
};
use otlp031::tonic::resource::v1::Resource;
use prost::Message;
use pyroscope::encode::r#gen::google;
use pyroscope::encode::r#gen::push::{PushRequest, RawProfileSeries, RawSample};
use pyroscope::encode::r#gen::types::LabelPair;
use sigiro_segments::{
    DecodeError, Table, transform_otlp_profiles, transform_pyroscope_profiles, write_segment,
};

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
        line: vec![Line {
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
            ..Default::default()
        }),
        sample: vec![
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
        attributes: vec![KeyValue {
            key: "service.name".into(),
            value: Some(AnyValue {
                value: Some(any_value::Value::StringValue("checkout".into())),
            }),
        }],
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

fn pprof() -> Vec<u8> {
    let string_table = ["", "cpu", "nanoseconds", "main", "work"]
        .map(String::from)
        .to_vec();
    google::Profile {
        sample_type: vec![google::ValueType { r#type: 1, unit: 2 }],
        sample: vec![
            google::Sample {
                location_id: vec![2, 1],
                value: vec![10],
                ..Default::default()
            },
            google::Sample {
                location_id: vec![1],
                value: vec![5],
                ..Default::default()
            },
        ],
        location: vec![
            google::Location {
                id: 1,
                line: vec![google::Line {
                    function_id: 1,
                    line: 0,
                }],
                ..Default::default()
            },
            google::Location {
                id: 2,
                line: vec![google::Line {
                    function_id: 2,
                    line: 0,
                }],
                ..Default::default()
            },
        ],
        function: vec![
            google::Function {
                id: 1,
                name: 3,
                ..Default::default()
            },
            google::Function {
                id: 2,
                name: 4,
                ..Default::default()
            },
        ],
        string_table,
        time_nanos: 1_700_000_000_000_000_000,
        ..Default::default()
    }
    .encode_to_vec()
}

fn push_request(service: &str) -> Vec<u8> {
    PushRequest {
        series: vec![RawProfileSeries {
            labels: vec![LabelPair {
                name: "service_name".into(),
                value: service.into(),
            }],
            samples: vec![RawSample {
                raw_profile: pprof(),
                id: String::new(),
            }],
        }],
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
fn a_pyroscope_push_request_takes_its_service_from_the_series_label() {
    let batch = transform_pyroscope_profiles(push_request("checkout"), None, 1 << 20).unwrap();
    assert_cpu_rows(&batch);
}

#[test]
fn a_raw_pprof_takes_its_service_from_the_sdk_name() {
    let batch = transform_pyroscope_profiles(pprof(), Some("checkout{env=prod}"), 1 << 20).unwrap();
    assert_cpu_rows(&batch);
}

#[test]
fn a_gzipped_pyroscope_request_is_decompressed() {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&push_request("checkout")).unwrap();
    let batch = transform_pyroscope_profiles(encoder.finish().unwrap(), None, 1 << 20).unwrap();
    assert_cpu_rows(&batch);
}

#[test]
fn an_oversized_pyroscope_request_is_too_large() {
    assert!(matches!(
        transform_pyroscope_profiles(push_request("checkout"), None, 8),
        Err(DecodeError::TooLarge(_))
    ));
}

#[test]
fn a_profile_with_an_out_of_range_stack_is_rejected() {
    let mut request = ExportProfilesServiceRequest::decode(otlp_request().as_slice()).unwrap();
    request.resource_profiles[0].scope_profiles[0].profiles[0].sample[0].stack_index = 99;
    assert!(transform_otlp_profiles(&request.encode_to_vec()).is_err());
}
