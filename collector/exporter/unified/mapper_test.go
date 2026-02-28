package unified

import (
	"testing"
	"time"

	"github.com/AkshatM/observe/pkg/schema"
	"go.opentelemetry.io/collector/pdata/pcommon"
	"go.opentelemetry.io/collector/pdata/plog"
	"go.opentelemetry.io/collector/pdata/pmetric"
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
	if *r.Value != 50000000 { // 50ms in ns
		t.Errorf("duration = %f, want 50000000", *r.Value)
	}
	if *r.Status != "Error" {
		t.Errorf("status = %q, want Error", *r.Status)
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
