package schema

// Column describes a single queryable column in the unified table.
type Column struct {
	Name        string   // column name in SQL
	Type        string   // DuckDB type (int64, string, float64, list<string>)
	Signals     []string // which signals use this column ("all" = every signal)
	Description string   // human/agent-readable description
	Nullable    bool     // whether the column can be null
}

// Columns is the canonical column registry. Every query surface (CLI, browser,
// agents.txt) is generated from this. Add columns here, not in agents.txt.
var Columns = []Column{
	{"timestamp", "int64", []string{"all"}, "Unix nanoseconds", false},
	{"signal", "string", []string{"all"}, `"metric", "span", "log", "profile"`, false},
	{"service", "string", []string{"all"}, "service name, primary filter", false},
	{"host", "string", []string{"all"}, "machine hostname", false},
	{"pod", "string", []string{"all"}, "k8s pod name", false},
	{"deploy_version", "string", []string{"all"}, "git SHA or version tag", true},
	{"trace_id", "string", []string{"span", "log"}, "distributed trace ID (hex)", true},
	{"span_id", "string", []string{"span"}, "span ID within trace (hex)", true},
	{"parent_span_id", "string", []string{"span"}, "parent span ID for call trees", true},
	{"name", "string", []string{"all"}, "what's measured: metric name, operation name, logger name, profile type", false},
	{"value", "float64", []string{"metric", "span", "profile"}, "numeric measurement: metric value, span duration (ms), sample count. null for logs", true},
	{"status", "string", []string{"span", "log"}, "span: OK/ERROR/UNSET, log: INFO/WARN/ERROR", true},
	{"message", "string", []string{"log"}, "log body text", true},
	{"unit", "string", []string{"metric", "span", "profile"}, "unit of value", true},
	{"kind", "string", []string{"span", "profile"}, "span: client/server/internal, profile: cpu/memory", true},
	{"stacktrace", "list<string>", []string{"profile"}, "call stack, root-to-leaf order", true},
	{"metadata", "string", []string{"all"}, "JSON blob, extra attributes — filter LAST", true},
}
