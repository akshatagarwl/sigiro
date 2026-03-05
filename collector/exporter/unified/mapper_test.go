package unified

import (
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/akshatagarwl/observe/pkg/schema"
	"go.opentelemetry.io/collector/pdata/pcommon"
	"go.opentelemetry.io/collector/pdata/plog"
	"go.opentelemetry.io/collector/pdata/pmetric"
	"go.opentelemetry.io/collector/pdata/pprofile"
	"go.opentelemetry.io/collector/pdata/ptrace"
)

func setResource(res pcommon.Resource) {
	res.Attributes().PutStr("service.name", "checkout")
	res.Attributes().PutStr("host.name", "node-01")
	res.Attributes().PutStr("k8s.pod.name", "checkout-abc-1")
	res.Attributes().PutStr("service.version", "v2.3.1")
}

// setResourceWithExtras sets the 4 promoted attrs plus extra resource attrs.
func setResourceWithExtras(res pcommon.Resource) {
	setResource(res)
	res.Attributes().PutStr("cloud.region", "us-east-1")
	res.Attributes().PutStr("k8s.namespace.name", "production")
}

// parseMetadata is a test helper that parses the metadata JSON string.
func parseMetadata(t *testing.T, metadata *string) map[string]string {
	t.Helper()

	if metadata == nil {
		t.Fatal("metadata is nil, expected JSON")
	}

	var m map[string]string
	if err := json.Unmarshal([]byte(*metadata), &m); err != nil {
		t.Fatalf("unmarshal metadata: %v", err)
	}

	return m
}

func TestMapMetrics_Gauge(t *testing.T) {
	md := pmetric.NewMetrics()
	rm := md.ResourceMetrics().AppendEmpty()
	setResource(rm.Resource())
	sm := rm.ScopeMetrics().AppendEmpty()
	m := sm.Metrics().AppendEmpty()
	m.SetName("memory.usage")
	m.SetUnit("MB")
	m.SetEmptyGauge()
	dp := m.Gauge().DataPoints().AppendEmpty()
	dp.SetDoubleValue(512.0)
	dp.SetTimestamp(pcommon.Timestamp(time.Date(2026, 3, 1, 14, 10, 0, 0, time.UTC).UnixNano()))

	rows := mapMetrics(md)
	if len(rows) != 1 {
		t.Fatalf("expected 1 row, got %d", len(rows))
	}

	r := rows[0]
	if r.Signal != schema.SignalMetric {
		t.Errorf("signal = %q, want metric", r.Signal)
	}

	if r.Service != "checkout" {
		t.Errorf("service = %q, want checkout", r.Service)
	}

	if r.Name != "memory.usage" {
		t.Errorf("name = %q, want memory.usage", r.Name)
	}

	if *r.Value != 512.0 {
		t.Errorf("value = %f, want 512", *r.Value)
	}

	if *r.Unit != "MB" {
		t.Errorf("unit = %q, want MB", *r.Unit)
	}
}

func TestMapMetrics_Sum(t *testing.T) {
	md := pmetric.NewMetrics()
	rm := md.ResourceMetrics().AppendEmpty()
	setResource(rm.Resource())
	sm := rm.ScopeMetrics().AppendEmpty()
	m := sm.Metrics().AppendEmpty()
	m.SetName("http.request.count")
	m.SetUnit("count")
	m.SetEmptySum()
	dp := m.Sum().DataPoints().AppendEmpty()
	dp.SetIntValue(342)
	dp.SetTimestamp(pcommon.Timestamp(time.Date(2026, 3, 1, 14, 10, 0, 0, time.UTC).UnixNano()))

	rows := mapMetrics(md)
	if len(rows) != 1 {
		t.Fatalf("expected 1 row, got %d", len(rows))
	}

	if *rows[0].Value != 342.0 {
		t.Errorf("value = %f, want 342", *rows[0].Value)
	}
}

func TestMapTraces(t *testing.T) {
	td := ptrace.NewTraces()
	rs := td.ResourceSpans().AppendEmpty()
	setResource(rs.Resource())
	ss := rs.ScopeSpans().AppendEmpty()
	s := ss.Spans().AppendEmpty()
	s.SetName("POST /checkout")
	s.SetTraceID([16]byte{1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16})
	s.SetSpanID([8]byte{1, 2, 3, 4, 5, 6, 7, 8})
	s.SetStartTimestamp(pcommon.Timestamp(1000000000))
	s.SetEndTimestamp(pcommon.Timestamp(1050000000))
	s.Status().SetCode(ptrace.StatusCodeError)
	s.SetKind(ptrace.SpanKindServer)

	rows := mapTraces(td)
	if len(rows) != 1 {
		t.Fatalf("expected 1 row, got %d", len(rows))
	}

	r := rows[0]
	if r.Signal != schema.SignalSpan {
		t.Errorf("signal = %q, want span", r.Signal)
	}

	if r.Name != "POST /checkout" {
		t.Errorf("name = %q", r.Name)
	}

	if r.TraceID == nil || *r.TraceID != "0102030405060708090a0b0c0d0e0f10" {
		t.Errorf("trace_id = %v", r.TraceID)
	}

	if r.SpanID == nil || *r.SpanID != "0102030405060708" {
		t.Errorf("span_id = %v", r.SpanID)
	}

	if *r.Value != 50 { // 50ms (converted from ns at write time, see schema.UnitSpanDuration)
		t.Errorf("duration = %f, want 50 (ms)", *r.Value)
	}

	if *r.Status != "ERROR" {
		t.Errorf("status = %q, want ERROR", *r.Status)
	}
}

func TestMapLogs(t *testing.T) {
	ld := plog.NewLogs()
	rl := ld.ResourceLogs().AppendEmpty()
	setResource(rl.Resource())
	sl := rl.ScopeLogs().AppendEmpty()
	sl.Scope().SetName("app.log")
	lr := sl.LogRecords().AppendEmpty()
	lr.SetSeverityText("ERROR")
	lr.Body().SetStr("OOMKilled: container checkout exceeded 512Mi")
	lr.SetTimestamp(pcommon.Timestamp(time.Date(2026, 3, 1, 14, 10, 2, 0, time.UTC).UnixNano()))

	rows := mapLogs(ld)
	if len(rows) != 1 {
		t.Fatalf("expected 1 row, got %d", len(rows))
	}

	r := rows[0]
	if r.Signal != schema.SignalLog {
		t.Errorf("signal = %q, want log", r.Signal)
	}

	if *r.Status != "ERROR" {
		t.Errorf("status = %q, want ERROR", *r.Status)
	}

	if *r.Message != "OOMKilled: container checkout exceeded 512Mi" {
		t.Errorf("message = %q", *r.Message)
	}

	if r.Name != "app.log" {
		t.Errorf("name = %q, want app.log", r.Name)
	}
}

func TestMapProfiles(t *testing.T) {
	pd := pprofile.NewProfiles()

	// Build the shared dictionary:
	// StringTable: [0]="cpu" [1]="count" [2]="runtime.gcDrain" [3]="runtime.gcBgMarkWorker" [4]="main.handler"
	dict := pd.Dictionary()
	dict.StringTable().Append("cpu")                    // 0
	dict.StringTable().Append("count")                  // 1
	dict.StringTable().Append("runtime.gcDrain")        // 2
	dict.StringTable().Append("runtime.gcBgMarkWorker") // 3
	dict.StringTable().Append("main.handler")           // 4

	// FunctionTable: [0]=gcDrain [1]=gcBgMarkWorker [2]=main.handler
	fn0 := dict.FunctionTable().AppendEmpty()
	fn0.SetNameStrindex(2) // "runtime.gcDrain"

	fn1 := dict.FunctionTable().AppendEmpty()
	fn1.SetNameStrindex(3) // "runtime.gcBgMarkWorker"

	fn2 := dict.FunctionTable().AppendEmpty()
	fn2.SetNameStrindex(4) // "main.handler"

	// LocationTable: [0]→fn0(gcDrain) [1]→fn1(gcBgMarkWorker) [2]→fn2(main.handler)
	loc0 := dict.LocationTable().AppendEmpty()
	loc0.Lines().AppendEmpty().SetFunctionIndex(0)
	loc1 := dict.LocationTable().AppendEmpty()
	loc1.Lines().AppendEmpty().SetFunctionIndex(1)
	loc2 := dict.LocationTable().AppendEmpty()
	loc2.Lines().AppendEmpty().SetFunctionIndex(2)

	// StackTable: [0]→locations [2, 1, 0] = main.handler → gcBgMarkWorker → gcDrain
	stack0 := dict.StackTable().AppendEmpty()
	stack0.LocationIndices().Append(2) // main.handler
	stack0.LocationIndices().Append(1) // gcBgMarkWorker
	stack0.LocationIndices().Append(0) // gcDrain

	// Resource + Profile
	rp := pd.ResourceProfiles().AppendEmpty()
	setResource(rp.Resource())
	sp := rp.ScopeProfiles().AppendEmpty()
	p := sp.Profiles().AppendEmpty()
	p.SampleType().SetTypeStrindex(0) // "cpu"
	p.SampleType().SetUnitStrindex(1) // "count"
	p.SetTime(pcommon.Timestamp(time.Date(2026, 3, 1, 14, 9, 30, 0, time.UTC).UnixNano()))

	// One sample: stack[0], value=150
	sample := p.Samples().AppendEmpty()
	sample.SetStackIndex(0)
	sample.Values().Append(150)

	rows, err := mapProfiles(pd)
	if err != nil {
		t.Fatalf("mapProfiles: %v", err)
	}

	if len(rows) != 1 {
		t.Fatalf("expected 1 row, got %d", len(rows))
	}

	r := rows[0]
	if r.Signal != schema.SignalProfile {
		t.Errorf("signal = %q, want profile", r.Signal)
	}

	if r.Service != "checkout" {
		t.Errorf("service = %q, want checkout", r.Service)
	}

	if r.Name != "profile.cpu" {
		t.Errorf("name = %q, want profile.cpu", r.Name)
	}

	if *r.Value != 150 {
		t.Errorf("value = %f, want 150", *r.Value)
	}

	if *r.Unit != "count" {
		t.Errorf("unit = %q, want count", *r.Unit)
	}

	// Stack should resolve to [main.handler, runtime.gcBgMarkWorker, runtime.gcDrain]
	wantStack := []string{"main.handler", "runtime.gcBgMarkWorker", "runtime.gcDrain"}
	if len(r.Stacktrace) != len(wantStack) {
		t.Fatalf("stacktrace len = %d, want %d: %v", len(r.Stacktrace), len(wantStack), r.Stacktrace)
	}

	for i, s := range r.Stacktrace {
		if s != wantStack[i] {
			t.Errorf("stacktrace[%d] = %q, want %q", i, s, wantStack[i])
		}
	}
}

// makeMinimalProfile builds the smallest valid pprofile.Profiles:
// one function ("runtime.main"), one location, one stack, one profile.
// The sample is added but with no values — caller must append values.
// Returns the Profiles and a reference to the sample for customization.
func makeMinimalProfile(typeStr, unitStr string) (pprofile.Profiles, pprofile.Sample) {
	pd := pprofile.NewProfiles()
	dict := pd.Dictionary()
	dict.StringTable().Append(typeStr)        // 0
	dict.StringTable().Append(unitStr)        // 1
	dict.StringTable().Append("runtime.main") // 2

	fn := dict.FunctionTable().AppendEmpty()
	fn.SetNameStrindex(2)

	loc := dict.LocationTable().AppendEmpty()
	loc.Lines().AppendEmpty().SetFunctionIndex(0)

	stack := dict.StackTable().AppendEmpty()
	stack.LocationIndices().Append(0)

	rp := pd.ResourceProfiles().AppendEmpty()
	setResource(rp.Resource())
	sp := rp.ScopeProfiles().AppendEmpty()
	p := sp.Profiles().AppendEmpty()
	p.SampleType().SetTypeStrindex(0)
	p.SampleType().SetUnitStrindex(1)
	p.SetTime(pcommon.Timestamp(time.Date(2026, 3, 1, 14, 9, 30, 0, time.UTC).UnixNano()))

	sample := p.Samples().AppendEmpty()
	sample.SetStackIndex(0)

	return pd, sample
}

func TestMapProfiles_MultiValueError(t *testing.T) {
	pd, sample := makeMinimalProfile("alloc_objects", "count")
	sample.Values().Append(150)
	sample.Values().Append(48000) // second value, no type metadata

	_, err := mapProfiles(pd)
	if err == nil {
		t.Fatal("expected error for multi-value profile sample, got nil")
	}

	if !strings.Contains(err.Error(), "multi-value") {
		t.Errorf("error should mention multi-value, got: %v", err)
	}
}

func TestMapProfiles_EmptyValueSkipped(t *testing.T) {
	pd, _ := makeMinimalProfile("cpu", "count")
	// eBPF profiler emits CPU samples with no values — should be skipped.

	rows, err := mapProfiles(pd)
	if err != nil {
		t.Fatalf("unexpected error for empty-value profile sample: %v", err)
	}

	if len(rows) != 0 {
		t.Errorf("expected 0 rows for empty-value sample, got %d", len(rows))
	}
}

func TestMapProfiles_MissingServiceName(t *testing.T) {
	pd, sample := makeMinimalProfile("cpu", "count")
	sample.Values().Append(1)

	// Override: remove service.name, keep only host.name.
	rp := pd.ResourceProfiles().At(0)
	rp.Resource().Attributes().Clear()
	rp.Resource().Attributes().PutStr("host.name", "node-01")

	rows, err := mapProfiles(pd)
	if err != nil {
		t.Fatalf("mapProfiles: %v", err)
	}

	if len(rows) != 1 {
		t.Fatalf("expected 1 row, got %d", len(rows))
	}

	if rows[0].Service != "unknown_service" {
		t.Fatalf("service = %q, want unknown_service", rows[0].Service)
	}
}

func TestMapProfiles_PreservesUnit(t *testing.T) {
	pd, sample := makeMinimalProfile("alloc_space", "bytes")
	sample.Values().Append(48000)

	rows, err := mapProfiles(pd)
	if err != nil {
		t.Fatalf("mapProfiles: %v", err)
	}

	if len(rows) != 1 {
		t.Fatalf("expected 1 row, got %d", len(rows))
	}

	if *rows[0].Unit != "bytes" {
		t.Errorf("unit = %q, want bytes", *rows[0].Unit)
	}

	if rows[0].Name != "profile.alloc_space" {
		t.Errorf("name = %q, want profile.alloc_space", rows[0].Name)
	}
}

// --- P0-1: Log attributes in metadata ---

func TestMapLogs_AttributesInMetadata(t *testing.T) {
	ld := plog.NewLogs()
	rl := ld.ResourceLogs().AppendEmpty()
	setResource(rl.Resource())
	sl := rl.ScopeLogs().AppendEmpty()
	sl.Scope().SetName("app.log")
	lr := sl.LogRecords().AppendEmpty()
	lr.SetSeverityText("ERROR")
	lr.Body().SetStr("request failed")
	lr.SetTimestamp(pcommon.Timestamp(time.Date(2026, 3, 1, 14, 10, 2, 0, time.UTC).UnixNano()))
	lr.Attributes().PutStr("user.id", "u-123")
	lr.Attributes().PutStr("request.id", "req-456")

	rows := mapLogs(ld)
	if len(rows) != 1 {
		t.Fatalf("expected 1 row, got %d", len(rows))
	}

	m := parseMetadata(t, rows[0].Metadata)

	if m["user.id"] != "u-123" {
		t.Errorf("user.id = %q, want u-123", m["user.id"])
	}

	if m["request.id"] != "req-456" {
		t.Errorf("request.id = %q, want req-456", m["request.id"])
	}
}

func TestMapLogs_NoAttributesNoMetadata(t *testing.T) {
	ld := plog.NewLogs()
	rl := ld.ResourceLogs().AppendEmpty()
	setResource(rl.Resource()) // only promoted attrs
	sl := rl.ScopeLogs().AppendEmpty()
	sl.Scope().SetName("app.log")
	lr := sl.LogRecords().AppendEmpty()
	lr.SetSeverityText("INFO")
	lr.Body().SetStr("ok")
	lr.SetTimestamp(pcommon.Timestamp(time.Date(2026, 3, 1, 14, 10, 2, 0, time.UTC).UnixNano()))

	rows := mapLogs(ld)
	if len(rows) != 1 {
		t.Fatalf("expected 1 row, got %d", len(rows))
	}

	if rows[0].Metadata != nil {
		t.Errorf("metadata should be nil when no extra attrs, got %q", *rows[0].Metadata)
	}
}

// --- P0-2: Resource attributes in metadata ---

func TestMapMetrics_ResourceAttrsInMetadata(t *testing.T) {
	md := pmetric.NewMetrics()
	rm := md.ResourceMetrics().AppendEmpty()
	setResourceWithExtras(rm.Resource())
	sm := rm.ScopeMetrics().AppendEmpty()
	m := sm.Metrics().AppendEmpty()
	m.SetName("cpu.usage")
	m.SetUnit("%")
	m.SetEmptyGauge()
	dp := m.Gauge().DataPoints().AppendEmpty()
	dp.SetDoubleValue(75.0)
	dp.SetTimestamp(pcommon.Timestamp(time.Date(2026, 3, 1, 14, 10, 0, 0, time.UTC).UnixNano()))

	rows := mapMetrics(md)
	if len(rows) != 1 {
		t.Fatalf("expected 1 row, got %d", len(rows))
	}

	meta := parseMetadata(t, rows[0].Metadata)

	if meta["cloud.region"] != "us-east-1" {
		t.Errorf("cloud.region = %q, want us-east-1", meta["cloud.region"])
	}

	if meta["k8s.namespace.name"] != "production" {
		t.Errorf("k8s.namespace.name = %q, want production", meta["k8s.namespace.name"])
	}

	// Promoted keys must NOT appear in metadata.
	for _, key := range []string{"service.name", "host.name", "k8s.pod.name", "service.version"} {
		if _, ok := meta[key]; ok {
			t.Errorf("promoted key %q should not be in metadata", key)
		}
	}
}

func TestMapTraces_ResourceAndSpanAttrsMerged(t *testing.T) {
	td := ptrace.NewTraces()
	rs := td.ResourceSpans().AppendEmpty()
	setResourceWithExtras(rs.Resource())
	ss := rs.ScopeSpans().AppendEmpty()
	s := ss.Spans().AppendEmpty()
	s.SetName("GET /api")
	s.SetTraceID([16]byte{1})
	s.SetSpanID([8]byte{1})
	s.SetStartTimestamp(pcommon.Timestamp(1000000000))
	s.SetEndTimestamp(pcommon.Timestamp(1050000000))
	s.Status().SetCode(ptrace.StatusCodeOk)
	s.SetKind(ptrace.SpanKindClient)
	// Span attr that conflicts with resource attr.
	s.Attributes().PutStr("cloud.region", "eu-west-1")
	s.Attributes().PutStr("http.method", "GET")

	rows := mapTraces(td)
	if len(rows) != 1 {
		t.Fatalf("expected 1 row, got %d", len(rows))
	}

	meta := parseMetadata(t, rows[0].Metadata)

	// Span attr overrides resource attr on conflict.
	if meta["cloud.region"] != "eu-west-1" {
		t.Errorf("cloud.region = %q, want eu-west-1 (span override)", meta["cloud.region"])
	}

	// Resource attr preserved when no conflict.
	if meta["k8s.namespace.name"] != "production" {
		t.Errorf("k8s.namespace.name = %q, want production", meta["k8s.namespace.name"])
	}

	// Span-specific attr present.
	if meta["http.method"] != "GET" {
		t.Errorf("http.method = %q, want GET", meta["http.method"])
	}
}

func TestMapLogs_ResourceAndLogAttrsMerged(t *testing.T) {
	ld := plog.NewLogs()
	rl := ld.ResourceLogs().AppendEmpty()
	setResourceWithExtras(rl.Resource())
	sl := rl.ScopeLogs().AppendEmpty()
	sl.Scope().SetName("app.log")
	lr := sl.LogRecords().AppendEmpty()
	lr.SetSeverityText("WARN")
	lr.Body().SetStr("slow query")
	lr.SetTimestamp(pcommon.Timestamp(time.Date(2026, 3, 1, 14, 10, 2, 0, time.UTC).UnixNano()))
	lr.Attributes().PutStr("db.statement", "SELECT 1")
	// Conflict: log attr overrides resource attr.
	lr.Attributes().PutStr("cloud.region", "ap-south-1")

	rows := mapLogs(ld)
	if len(rows) != 1 {
		t.Fatalf("expected 1 row, got %d", len(rows))
	}

	meta := parseMetadata(t, rows[0].Metadata)

	if meta["cloud.region"] != "ap-south-1" {
		t.Errorf("cloud.region = %q, want ap-south-1 (log override)", meta["cloud.region"])
	}

	if meta["k8s.namespace.name"] != "production" {
		t.Errorf("k8s.namespace.name = %q, want production", meta["k8s.namespace.name"])
	}

	if meta["db.statement"] != "SELECT 1" {
		t.Errorf("db.statement = %q, want SELECT 1", meta["db.statement"])
	}
}

func TestMapProfiles_ResourceAttrsInMetadata(t *testing.T) {
	pd, sample := makeMinimalProfile("cpu", "count")
	sample.Values().Append(100)
	// Add extra resource attrs.
	rp := pd.ResourceProfiles().At(0)
	rp.Resource().Attributes().PutStr("cloud.region", "us-east-1")
	rp.Resource().Attributes().PutStr("telemetry.sdk.language", "go")

	rows, err := mapProfiles(pd)
	if err != nil {
		t.Fatalf("mapProfiles: %v", err)
	}

	if len(rows) != 1 {
		t.Fatalf("expected 1 row, got %d", len(rows))
	}

	meta := parseMetadata(t, rows[0].Metadata)

	if meta["cloud.region"] != "us-east-1" {
		t.Errorf("cloud.region = %q, want us-east-1", meta["cloud.region"])
	}

	if meta["telemetry.sdk.language"] != "go" {
		t.Errorf("telemetry.sdk.language = %q, want go", meta["telemetry.sdk.language"])
	}
}
