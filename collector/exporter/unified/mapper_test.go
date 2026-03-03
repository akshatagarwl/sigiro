package unified

import (
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

	rows := mapProfiles(pd)
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

func TestMapProfiles_MissingServiceName(t *testing.T) {
	pd := pprofile.NewProfiles()
	dict := pd.Dictionary()
	dict.StringTable().Append("cpu")
	dict.StringTable().Append("count")
	dict.StringTable().Append("runtime.main")

	fn0 := dict.FunctionTable().AppendEmpty()
	fn0.SetNameStrindex(2)

	loc0 := dict.LocationTable().AppendEmpty()
	loc0.Lines().AppendEmpty().SetFunctionIndex(0)

	stack0 := dict.StackTable().AppendEmpty()
	stack0.LocationIndices().Append(0)

	rp := pd.ResourceProfiles().AppendEmpty()
	rp.Resource().Attributes().PutStr("host.name", "node-01")
	sp := rp.ScopeProfiles().AppendEmpty()
	p := sp.Profiles().AppendEmpty()
	p.SampleType().SetTypeStrindex(0)
	p.SampleType().SetUnitStrindex(1)
	p.SetTime(pcommon.Timestamp(time.Date(2026, 3, 1, 14, 9, 30, 0, time.UTC).UnixNano()))
	sample := p.Samples().AppendEmpty()
	sample.SetStackIndex(0)
	sample.Values().Append(1)

	rows := mapProfiles(pd)
	if len(rows) != 1 {
		t.Fatalf("expected 1 row, got %d", len(rows))
	}

	if rows[0].Service != "unknown_service" {
		t.Fatalf("service = %q, want unknown_service", rows[0].Service)
	}
}
