package schema

// Signal type constants.
const (
	SignalMetric  = "metric"
	SignalSpan    = "span"
	SignalLog     = "log"
	SignalProfile = "profile"
)

// Event represents a span event (e.g. exception, annotation).
type Event struct {
	Timestamp  int64   `parquet:"timestamp" json:"timestamp"`
	Name       string  `parquet:"name" json:"name"`
	Attributes *string `parquet:"attributes,optional" json:"attributes,omitempty"`
}

// UnifiedRow is the canonical schema for the unified Parquet table.
// All observability signals (metrics, spans, logs, profiles) are stored
// as rows in this single schema. Sort order: (service, signal, timestamp).
type UnifiedRow struct {
	Timestamp    int64    `parquet:"timestamp"`
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
	Stacktrace   []string `parquet:"stacktrace,optional"`
	Events       []Event  `parquet:"events,optional"`
	Metadata     *string  `parquet:"metadata,optional"`
}
