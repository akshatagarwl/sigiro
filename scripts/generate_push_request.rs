use flate2::Compression;
use flate2::write::GzEncoder;
use prost::Message;
use pyroscope::encode::r#gen::google::{Function, Line, Location, Profile, Sample, ValueType};
use pyroscope::encode::r#gen::push::{PushRequest, RawProfileSeries, RawSample};
use pyroscope::encode::r#gen::types::LabelPair;
use std::io::Write;

fn main() {
    let service_name = "python-sdk-test";
    let frames = ["main", "handle_request", "process_data"];
    let sample_value: i64 = 15000000;

    // Use a fixed timestamp (1680000000000000000 ns)
    let timestamp_ns: i64 = 1680000000000000000;

    // pprof string_table: [0]="", [1]="cpu", [2]="nanoseconds", [3..]=frame names
    let mut string_table = vec![String::new()];
    string_table.push("cpu".to_string());
    string_table.push("nanoseconds".to_string());

    let frame_base = string_table.len() as i64;
    for frame in &frames {
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
    let mut encoder = GzEncoder::new(&mut gzipped, Compression::default());
    encoder.write_all(&encoded).expect("gzip write");
    encoder.finish().expect("gzip finish");

    // Write hex to stdout
    println!("{}", hex::encode(&gzipped));

    // Also save to /tmp/push_request.gz
    std::fs::write("/tmp/push_request.gz", &gzipped).expect("write gzip file");
}
