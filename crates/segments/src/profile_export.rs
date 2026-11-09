//! Encode in-process profiles as an OTLP `ExportProfilesServiceRequest`.
//!
//! sigiro profiles itself (CPU and heap) and sends the result to an OTLP
//! profiles endpoint, normally its own. This builds the request from plain
//! stacks so the caller needs no OTLP types: frames are deduplicated into the
//! shared dictionary's function, location and stack tables.

use std::collections::HashMap;

use prost::Message;

use crate::otel_proto::tonic::collector::profiles::v1development::ExportProfilesServiceRequest;
use crate::otel_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue, any_value};
use crate::otel_proto::tonic::profiles::v1development::{
    Function, KeyValueAndUnit, Line, Link, Location, Mapping, Profile, ProfilesDictionary,
    ResourceProfiles, Sample, ScopeProfiles, Stack, ValueType,
};
use crate::otel_proto::tonic::resource::v1::Resource;

/// One frame of a stack. For an inlined call site, list the inlined function
/// first and the function it was inlined into after it, as separate frames.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExportFrame {
    pub function: String,
    pub file: String,
    pub line: i64,
}

/// A stack (leaf first) with its value and string attributes.
#[derive(Debug, Clone)]
pub struct ExportSample {
    pub frames: Vec<ExportFrame>,
    pub value: i64,
    pub attributes: Vec<(String, String)>,
}

/// One profile: a sample type and its samples over a time window.
#[derive(Debug, Clone)]
pub struct ExportProfile {
    /// Sample type and unit, for example `("cpu", "nanoseconds")`.
    pub sample_type: (String, String),
    /// Sampling period type and unit, for example `("cpu", "nanoseconds")`.
    pub period_type: (String, String),
    pub period: i64,
    pub time_unix_nano: u64,
    pub duration_nano: u64,
    pub samples: Vec<ExportSample>,
}

/// Profiles from one process, identified by its resource attributes, which
/// must include `service.name`.
#[derive(Debug, Clone)]
pub struct ExportRequest {
    pub resource: Vec<(String, String)>,
    pub scope_name: String,
    pub scope_version: String,
    pub profiles: Vec<ExportProfile>,
}

#[derive(Default)]
struct Dictionary {
    dict: ProfilesDictionary,
    strings: HashMap<String, i32>,
    functions: HashMap<(i32, i32), i32>,
    locations: HashMap<(i32, i64), i32>,
    stacks: HashMap<Vec<i32>, i32>,
    attributes: HashMap<(i32, String), i32>,
}

fn index(len: usize) -> i32 {
    i32::try_from(len).unwrap_or(i32::MAX)
}

impl Dictionary {
    fn new() -> Self {
        let mut this = Self::default();
        // Index 0 of every table is the zero value, per the OTLP profiles spec.
        this.dict.string_table.push(String::new());
        this.strings.insert(String::new(), 0);
        this.dict.mapping_table.push(Mapping::default());
        this.dict.location_table.push(Location::default());
        this.dict.function_table.push(Function::default());
        this.dict.link_table.push(Link::default());
        this.dict.attribute_table.push(KeyValueAndUnit::default());
        this.dict.stack_table.push(Stack::default());
        this
    }

    fn string(&mut self, value: &str) -> i32 {
        if let Some(&i) = self.strings.get(value) {
            return i;
        }
        let i = index(self.dict.string_table.len());
        self.dict.string_table.push(value.to_string());
        self.strings.insert(value.to_string(), i);
        i
    }

    fn value_type(&mut self, (kind, unit): &(String, String)) -> ValueType {
        ValueType {
            type_strindex: self.string(kind),
            unit_strindex: self.string(unit),
        }
    }

    fn location(&mut self, frame: &ExportFrame) -> i32 {
        let name = self.string(&frame.function);
        let file = self.string(&frame.file);
        let function = match self.functions.get(&(name, file)) {
            Some(&i) => i,
            None => {
                let i = index(self.dict.function_table.len());
                self.dict.function_table.push(Function {
                    name_strindex: name,
                    system_name_strindex: name,
                    filename_strindex: file,
                    start_line: 0,
                });
                self.functions.insert((name, file), i);
                i
            }
        };
        if let Some(&i) = self.locations.get(&(function, frame.line)) {
            return i;
        }
        let i = index(self.dict.location_table.len());
        self.dict.location_table.push(Location {
            mapping_index: 0,
            address: 0,
            lines: vec![Line {
                function_index: function,
                line: frame.line,
                column: 0,
            }],
            attribute_indices: Vec::new(),
        });
        self.locations.insert((function, frame.line), i);
        i
    }

    fn stack(&mut self, frames: &[ExportFrame]) -> i32 {
        let locations: Vec<i32> = frames.iter().map(|frame| self.location(frame)).collect();
        if let Some(&i) = self.stacks.get(&locations) {
            return i;
        }
        let i = index(self.dict.stack_table.len());
        self.dict.stack_table.push(Stack {
            location_indices: locations.clone(),
        });
        self.stacks.insert(locations, i);
        i
    }

    fn attribute(&mut self, key: &str, value: &str) -> i32 {
        let key = self.string(key);
        if let Some(&i) = self.attributes.get(&(key, value.to_string())) {
            return i;
        }
        let i = index(self.dict.attribute_table.len());
        self.dict.attribute_table.push(KeyValueAndUnit {
            key_strindex: key,
            value: Some(string_value(value)),
            unit_strindex: 0,
        });
        self.attributes.insert((key, value.to_string()), i);
        i
    }
}

fn string_value(value: &str) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::StringValue(value.to_string())),
    }
}

/// Encode `request` as the protobuf bytes of an `ExportProfilesServiceRequest`.
pub fn encode_profiles_request(request: &ExportRequest) -> Vec<u8> {
    let mut dict = Dictionary::new();
    let profiles = request
        .profiles
        .iter()
        .map(|profile| Profile {
            sample_type: Some(dict.value_type(&profile.sample_type)),
            period_type: Some(dict.value_type(&profile.period_type)),
            period: profile.period,
            time_unix_nano: profile.time_unix_nano,
            duration_nano: profile.duration_nano,
            samples: profile
                .samples
                .iter()
                .filter(|sample| sample.value != 0)
                .map(|sample| Sample {
                    stack_index: dict.stack(&sample.frames),
                    attribute_indices: sample
                        .attributes
                        .iter()
                        .map(|(key, value)| dict.attribute(key, value))
                        .collect(),
                    link_index: 0,
                    values: vec![sample.value],
                    timestamps_unix_nano: Vec::new(),
                })
                .collect(),
            ..Profile::default()
        })
        .collect();
    let resource = Resource {
        attributes: request
            .resource
            .iter()
            .map(|(key, value)| KeyValue {
                key: key.clone(),
                value: Some(string_value(value)),
                ..KeyValue::default()
            })
            .collect(),
        ..Resource::default()
    };
    ExportProfilesServiceRequest {
        resource_profiles: vec![ResourceProfiles {
            resource: Some(resource),
            scope_profiles: vec![ScopeProfiles {
                scope: Some(InstrumentationScope {
                    name: request.scope_name.clone(),
                    version: request.scope_version.clone(),
                    ..InstrumentationScope::default()
                }),
                profiles,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
        dictionary: Some(dict.dict),
    }
    .encode_to_vec()
}
