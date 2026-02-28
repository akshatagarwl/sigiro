package schema

// InvestigationBlock is the structured response returned by the query server.
// It contains all signals for a service within a time window, plus
// cross-service linked traces.
type InvestigationBlock struct {
	Service  string       `json:"service"`
	Window   string       `json:"window"`
	Metrics  []MetricRow  `json:"metrics"`
	Spans    []SpanNode   `json:"spans"`
	Logs     []LogRow     `json:"logs"`
	Profiles []ProfileRow `json:"profiles"`
	Linked   LinkedBlock  `json:"linked"`
}

// MetricRow is a time-bucketed metric aggregation.
type MetricRow struct {
	Bucket string   `json:"bucket"`
	Name   string   `json:"name"`
	P95    *float64 `json:"p95"`
	Avg    *float64 `json:"avg"`
	N      int      `json:"n"`
}

// SpanNode is a span with optional children forming a trace tree.
type SpanNode struct {
	TraceID    string     `json:"trace_id"`
	SpanID     string     `json:"span_id"`
	ParentID   *string    `json:"parent_span_id,omitempty"`
	Service    string     `json:"service,omitempty"`
	Name       string     `json:"name"`
	DurationMs float64   `json:"duration_ms"`
	Status     string     `json:"status"`
	Kind       string     `json:"kind,omitempty"`
	Children   []SpanNode `json:"children,omitempty"`
}

// LogRow is a single log entry.
type LogRow struct {
	T       string  `json:"t"`
	Status  string  `json:"status"`
	Message string  `json:"message"`
	TraceID *string `json:"trace_id,omitempty"`
}

// ProfileRow is an aggregated profile function with sample count.
type ProfileRow struct {
	Function string  `json:"function"`
	Samples  float64 `json:"samples"`
}

// LinkedBlock contains cross-service traces and logs associated
// with error trace IDs found in the primary query.
type LinkedBlock struct {
	Traces []LinkedTrace `json:"traces"`
	Logs   []LogRow      `json:"logs"`
}

// LinkedTrace is a full trace tree for a single trace ID,
// containing spans from all services.
type LinkedTrace struct {
	TraceID string     `json:"trace_id"`
	Spans   []SpanNode `json:"spans"`
}
