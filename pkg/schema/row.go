package schema

// Signal type constants.
const (
	SignalMetric  = "metric"
	SignalSpan    = "span"
	SignalLog     = "log"
	SignalProfile = "profile"
)

// Canonical units for the Value column by signal type.
// These define the contract between the write path (mapper) and the read path (queries).
// FRAGILE: this is a stringly-typed contract enforced only by comments.
// Changing a unit here requires updating both mapper.go and queries.go,
// but nothing prevents them from drifting. The query layer hardcodes
// "value > 1000" assuming milliseconds — if the mapper ever changes to
// seconds or microseconds, queries silently return wrong results.
const (
	UnitSpanDuration = "ms"    // span Value is duration in milliseconds (converted from OTel nanoseconds at write time)
	UnitProfileCount = "count" // profile Value default unit; actual unit preserved from pprofile SampleType (e.g. "bytes" for alloc_space)
)

// UnifiedRow is the canonical schema for the unified Parquet table.
// All observability signals (metrics, spans, logs, profiles) are stored
// as rows in this single schema. Sort order: (service, signal, timestamp).
//
// Planned additions (see docs/plans/04-wide-event-gaps.md):
//
//	P1: duration_ms float64?          — span duration (decouple from Value)
//	P1: http_method string? (dict)    — promoted from metadata
//	P1: http_status_code int32?       — promoted from metadata
//	P1: http_route string? (dict)     — promoted from metadata
//	P2: ingest_timestamp int64        — when the exporter received the data
type UnifiedRow struct {
	Timestamp    int64    `parquet:"timestamp,delta"`
	Signal       string   `parquet:"signal,dict"`
	Service      string   `parquet:"service,dict"`
	Host         string   `parquet:"host,dict"`
	Pod          string   `parquet:"pod,dict"`
	DeployVer    string   `parquet:"deploy_version,dict,optional"`
	TraceID      *string  `parquet:"trace_id,optional"`
	SpanID       *string  `parquet:"span_id,optional"`
	ParentSpanID *string  `parquet:"parent_span_id,optional"`
	Name         string   `parquet:"name,dict"`
	Value        *float64 `parquet:"value,optional"`
	Status       *string  `parquet:"status,optional,dict"`
	Message      *string  `parquet:"message,optional"`
	Unit         *string  `parquet:"unit,optional,dict"`
	Kind         *string  `parquet:"kind,optional,dict"`
	Stacktrace   []string `parquet:"stacktrace,optional,list" parquet-element:",dict"`
	// FRAGILE: metadata is a JSON-encoded string, not a structured type.
	// Every consumer must parse it, and there is no schema validation.
	// Attribute values are coerced to strings via AsString() in the mapper,
	// losing type information (ints, bools, arrays, maps all become strings).
	//
	// GAP: High-value attributes (http.method, http.status_code, http.route)
	// should be promoted to their own columns for queryability. Filtering by
	// http.status_code=500 currently requires JSON_EXTRACT on every row.
	// See docs/plans/11-wide-event-gaps.md §P1.
	Metadata *string `parquet:"metadata,optional"`
}
