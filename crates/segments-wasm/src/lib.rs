use arrow::record_batch::RecordBatch;
use js_sys::{Array, Object, Reflect, Uint8Array};
use sigiro_segments::{
    InputFormat, Manifest, Segment, Table, transform_logs, transform_metrics, transform_traces,
    write_segment,
};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn decode_otlp(signal: &str, format: &str, body: &[u8]) -> Result<Array, JsValue> {
    let format = parse_format(format)?;
    let segments = match signal {
        "traces" => write_batch(
            Table::Spans,
            &transform_traces(body, format).map_err(|error| to_js_error(&error))?,
        )?,
        "logs" => {
            let (batch, _) = transform_logs(body, format).map_err(|error| to_js_error(&error))?;
            write_batch(Table::Logs, &batch)?
        }
        "metrics" => {
            let batches = transform_metrics(body, format).map_err(|error| to_js_error(&error))?;
            let out = Array::new();
            if let Some(batch) = batches.gauge {
                push_segments(&out, &write_batch(Table::MetricsGauge, &batch)?);
            }
            if let Some(batch) = batches.sum {
                push_segments(&out, &write_batch(Table::MetricsSum, &batch)?);
            }
            if let Some(batch) = batches.histogram {
                push_segments(&out, &write_batch(Table::MetricsHistogram, &batch)?);
            }
            if let Some(batch) = batches.exp_histogram {
                push_segments(&out, &write_batch(Table::MetricsExpHistogram, &batch)?);
            }
            out
        }
        _ => return Err(error("unsupported signal")),
    };
    Ok(segments)
}

#[wasm_bindgen]
pub fn manifest_empty() -> Result<String, JsValue> {
    serde_json::to_string(&Manifest::default()).map_err(|error| to_js_error(&error))
}

#[wasm_bindgen]
pub fn manifest_register(manifest: &str, key: &str, table: &str) -> Result<String, JsValue> {
    let mut manifest = read_manifest(manifest)?;
    manifest
        .register(key, parse_table(table)?)
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    write_manifest(&manifest)
}

#[wasm_bindgen]
pub fn manifest_commit(manifest: &str, segment: &str) -> Result<String, JsValue> {
    let mut manifest = read_manifest(manifest)?;
    let segment = serde_json::from_str::<Segment>(segment).map_err(|error| to_js_error(&error))?;
    manifest
        .commit(segment)
        .map_err(|error| JsValue::from_str(&error.to_string()))?;
    write_manifest(&manifest)
}

#[wasm_bindgen]
pub fn manifest_live(
    manifest: &str,
    table: &str,
    from_us: i64,
    to_us: i64,
) -> Result<String, JsValue> {
    let manifest = read_manifest(manifest)?;
    serde_json::to_string(&manifest.live(parse_table(table)?, from_us, to_us))
        .map_err(|error| to_js_error(&error))
}

#[wasm_bindgen]
pub fn manifest_orphans(manifest: &str, stored: &Array) -> Result<Array, JsValue> {
    let manifest = read_manifest(manifest)?;
    let keys = stored
        .iter()
        .map(|value| {
            value
                .as_string()
                .ok_or_else(|| JsValue::from_str("stored keys must be strings"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let out = Array::new();
    for key in manifest.orphans(keys.iter().map(String::as_str)) {
        out.push(&JsValue::from_str(key));
    }
    Ok(out)
}

fn write_batch(table: Table, batch: &RecordBatch) -> Result<Array, JsValue> {
    if batch.num_rows() == 0 {
        return Ok(Array::new());
    }
    let segment =
        write_segment(table, batch).map_err(|error| JsValue::from_str(&error.to_string()))?;
    let out = Array::new();
    let entry = Object::new();
    let metadata = serde_json::to_string(&segment.metadata).map_err(|error| to_js_error(&error))?;
    let parquet = Uint8Array::from(segment.parquet.as_slice());
    Reflect::set(
        &entry,
        &JsValue::from_str("metadata"),
        &JsValue::from_str(&metadata),
    )?;
    Reflect::set(&entry, &JsValue::from_str("parquet"), &parquet)?;
    out.push(&entry);
    Ok(out)
}

fn push_segments(out: &Array, segments: &Array) {
    for segment in segments.iter() {
        out.push(&segment);
    }
}

fn parse_format(format: &str) -> Result<InputFormat, JsValue> {
    match format {
        "protobuf" => Ok(InputFormat::Protobuf),
        "json" => Ok(InputFormat::Json),
        _ => Err(error("unsupported OTLP format")),
    }
}

fn parse_table(table: &str) -> Result<Table, JsValue> {
    match table {
        "spans" => Ok(Table::Spans),
        "logs" => Ok(Table::Logs),
        "metrics_gauge" => Ok(Table::MetricsGauge),
        "metrics_sum" => Ok(Table::MetricsSum),
        "metrics_histogram" => Ok(Table::MetricsHistogram),
        "metrics_exp_histogram" => Ok(Table::MetricsExpHistogram),
        "profiles" => Ok(Table::Profiles),
        _ => Err(error("unsupported table")),
    }
}

fn read_manifest(manifest: &str) -> Result<Manifest, JsValue> {
    serde_json::from_str(manifest).map_err(|error| to_js_error(&error))
}

fn write_manifest(manifest: &Manifest) -> Result<String, JsValue> {
    serde_json::to_string(manifest).map_err(|error| to_js_error(&error))
}

fn to_js_error(error: &impl ToString) -> JsValue {
    JsValue::from_str(&error.to_string())
}

fn error(message: &str) -> JsValue {
    JsValue::from_str(message)
}
