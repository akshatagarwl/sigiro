package unified

import (
	"database/sql"
	"fmt"
	"os"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/AkshatM/observe/pkg/schema"
	_ "github.com/marcboeker/go-duckdb"
	"github.com/parquet-go/parquet-go"
	"github.com/parquet-go/parquet-go/compress/zstd"
	"go.opentelemetry.io/collector/pdata/pcommon"
	"go.opentelemetry.io/collector/pdata/plog"
	"go.opentelemetry.io/collector/pdata/pmetric"
	"go.opentelemetry.io/collector/pdata/pprofile"
	"go.opentelemetry.io/collector/pdata/ptrace"
)

// buildRealisticTraces creates ptrace.Traces mimicking two instrumented services:
// checkout (server) calling payments (client→server), with parent-child spans,
// span events (exceptions), and span attributes (metadata).
func buildRealisticTraces() ptrace.Traces {
	td := ptrace.NewTraces()

	// === checkout service ===
	rs1 := td.ResourceSpans().AppendEmpty()
	res1 := rs1.Resource()
	res1.Attributes().PutStr("service.name", "api-gateway")
	res1.Attributes().PutStr("host.name", "node-01")
	res1.Attributes().PutStr("k8s.pod.name", "api-gw-abc-1")
	res1.Attributes().PutStr("service.version", "v3.1.0")

	ss1 := rs1.ScopeSpans().AppendEmpty()
	ss1.Scope().SetName("go.opentelemetry.io/contrib/instrumentation/net/http")
	ss1.Scope().SetVersion("0.49.0")

	// Root span: HTTP request
	root := ss1.Spans().AppendEmpty()
	root.SetName("GET /users/123")
	root.SetTraceID([16]byte{0xab, 0xcd, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab})
	root.SetSpanID([8]byte{0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08})
	root.SetStartTimestamp(pcommon.Timestamp(time.Date(2026, 3, 15, 10, 30, 0, 0, time.UTC).UnixNano()))
	root.SetEndTimestamp(pcommon.Timestamp(time.Date(2026, 3, 15, 10, 30, 0, 150_000_000, time.UTC).UnixNano()))
	root.Status().SetCode(ptrace.StatusCodeOk)
	root.SetKind(ptrace.SpanKindServer)
	root.Attributes().PutStr("http.method", "GET")
	root.Attributes().PutStr("http.route", "/users/:id")
	root.Attributes().PutInt("http.status_code", 200)

	// Child span: DB query
	child1 := ss1.Spans().AppendEmpty()
	child1.SetName("SELECT users")
	child1.SetTraceID([16]byte{0xab, 0xcd, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab})
	child1.SetSpanID([8]byte{0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88})
	child1.SetParentSpanID([8]byte{0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08})
	child1.SetStartTimestamp(pcommon.Timestamp(time.Date(2026, 3, 15, 10, 30, 0, 10_000_000, time.UTC).UnixNano()))
	child1.SetEndTimestamp(pcommon.Timestamp(time.Date(2026, 3, 15, 10, 30, 0, 45_000_000, time.UTC).UnixNano()))
	child1.Status().SetCode(ptrace.StatusCodeOk)
	child1.SetKind(ptrace.SpanKindClient)
	child1.Attributes().PutStr("db.system", "postgresql")
	child1.Attributes().PutStr("db.statement", "SELECT * FROM users WHERE id = $1")

	// Error span with exception event
	errSpan := ss1.Spans().AppendEmpty()
	errSpan.SetName("POST /orders")
	errSpan.SetTraceID([16]byte{0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x00})
	errSpan.SetSpanID([8]byte{0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x11})
	errSpan.SetStartTimestamp(pcommon.Timestamp(time.Date(2026, 3, 15, 10, 31, 0, 0, time.UTC).UnixNano()))
	errSpan.SetEndTimestamp(pcommon.Timestamp(time.Date(2026, 3, 15, 10, 31, 2, 500_000_000, time.UTC).UnixNano()))
	errSpan.Status().SetCode(ptrace.StatusCodeError)
	errSpan.Status().SetMessage("deadline exceeded")
	errSpan.SetKind(ptrace.SpanKindServer)

	// Span event: exception
	ev := errSpan.Events().AppendEmpty()
	ev.SetName("exception")
	ev.SetTimestamp(pcommon.Timestamp(time.Date(2026, 3, 15, 10, 31, 2, 0, time.UTC).UnixNano()))
	ev.Attributes().PutStr("exception.type", "context.DeadlineExceeded")
	ev.Attributes().PutStr("exception.message", "context deadline exceeded")

	// === payments service (same trace, different resource) ===
	rs2 := td.ResourceSpans().AppendEmpty()
	res2 := rs2.Resource()
	res2.Attributes().PutStr("service.name", "billing")
	res2.Attributes().PutStr("host.name", "node-02")
	res2.Attributes().PutStr("k8s.pod.name", "billing-xyz-1")
	res2.Attributes().PutStr("service.version", "v2.0.4")

	ss2 := rs2.ScopeSpans().AppendEmpty()
	// Cross-service child of the error span
	paySpan := ss2.Spans().AppendEmpty()
	paySpan.SetName("POST /charge")
	paySpan.SetTraceID([16]byte{0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x00})
	paySpan.SetSpanID([8]byte{0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99})
	paySpan.SetParentSpanID([8]byte{0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x11})
	paySpan.SetStartTimestamp(pcommon.Timestamp(time.Date(2026, 3, 15, 10, 31, 0, 100_000_000, time.UTC).UnixNano()))
	paySpan.SetEndTimestamp(pcommon.Timestamp(time.Date(2026, 3, 15, 10, 31, 2, 400_000_000, time.UTC).UnixNano()))
	paySpan.Status().SetCode(ptrace.StatusCodeError)
	paySpan.SetKind(ptrace.SpanKindServer)

	return td
}

// buildRealisticMetrics creates pmetric.Metrics with gauge, sum, and histogram types.
func buildRealisticMetrics() pmetric.Metrics {
	md := pmetric.NewMetrics()

	rm := md.ResourceMetrics().AppendEmpty()
	res := rm.Resource()
	res.Attributes().PutStr("service.name", "api-gateway")
	res.Attributes().PutStr("host.name", "node-01")
	res.Attributes().PutStr("k8s.pod.name", "api-gw-abc-1")
	res.Attributes().PutStr("service.version", "v3.1.0")

	sm := rm.ScopeMetrics().AppendEmpty()
	sm.Scope().SetName("go.opentelemetry.io/contrib/instrumentation/runtime")

	// Gauge: memory usage
	mem := sm.Metrics().AppendEmpty()
	mem.SetName("process.runtime.go.mem.heap_alloc")
	mem.SetUnit("By")
	mem.SetEmptyGauge()
	for i := 0; i < 5; i++ {
		dp := mem.Gauge().DataPoints().AppendEmpty()
		dp.SetDoubleValue(float64(200_000_000 + i*50_000_000))
		dp.SetTimestamp(pcommon.Timestamp(
			time.Date(2026, 3, 15, 10, 30+i, 0, 0, time.UTC).UnixNano(),
		))
	}

	// Sum: request count
	reqCount := sm.Metrics().AppendEmpty()
	reqCount.SetName("http.server.request.count")
	reqCount.SetUnit("{requests}")
	reqCount.SetEmptySum()
	reqCount.Sum().SetIsMonotonic(true)
	dp := reqCount.Sum().DataPoints().AppendEmpty()
	dp.SetIntValue(1542)
	dp.SetTimestamp(pcommon.Timestamp(
		time.Date(2026, 3, 15, 10, 34, 0, 0, time.UTC).UnixNano(),
	))

	// Histogram: request duration
	dur := sm.Metrics().AppendEmpty()
	dur.SetName("http.server.duration")
	dur.SetUnit("ms")
	dur.SetEmptyHistogram()
	hdp := dur.Histogram().DataPoints().AppendEmpty()
	hdp.SetCount(100)
	hdp.SetSum(8500)
	hdp.SetTimestamp(pcommon.Timestamp(
		time.Date(2026, 3, 15, 10, 34, 0, 0, time.UTC).UnixNano(),
	))

	return md
}

// buildRealisticLogs creates plog.Logs with multiple severity levels and trace correlation.
func buildRealisticLogs() plog.Logs {
	ld := plog.NewLogs()

	rl := ld.ResourceLogs().AppendEmpty()
	res := rl.Resource()
	res.Attributes().PutStr("service.name", "api-gateway")
	res.Attributes().PutStr("host.name", "node-01")
	res.Attributes().PutStr("k8s.pod.name", "api-gw-abc-1")
	res.Attributes().PutStr("service.version", "v3.1.0")

	sl := rl.ScopeLogs().AppendEmpty()
	sl.Scope().SetName("app.logger")

	// INFO log
	info := sl.LogRecords().AppendEmpty()
	info.SetSeverityText("INFO")
	info.Body().SetStr("server started on :8080")
	info.SetTimestamp(pcommon.Timestamp(time.Date(2026, 3, 15, 10, 29, 0, 0, time.UTC).UnixNano()))

	// WARN log
	warn := sl.LogRecords().AppendEmpty()
	warn.SetSeverityText("WARN")
	warn.Body().SetStr("connection pool at 80% capacity")
	warn.SetTimestamp(pcommon.Timestamp(time.Date(2026, 3, 15, 10, 33, 0, 0, time.UTC).UnixNano()))

	// ERROR log correlated to a trace
	errLog := sl.LogRecords().AppendEmpty()
	errLog.SetSeverityText("ERROR")
	errLog.Body().SetStr("request failed: context deadline exceeded")
	errLog.SetTimestamp(pcommon.Timestamp(time.Date(2026, 3, 15, 10, 31, 2, 500_000_000, time.UTC).UnixNano()))
	errLog.SetTraceID([16]byte{0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x00})

	// Second service logs
	rl2 := ld.ResourceLogs().AppendEmpty()
	res2 := rl2.Resource()
	res2.Attributes().PutStr("service.name", "billing")
	res2.Attributes().PutStr("host.name", "node-02")
	res2.Attributes().PutStr("k8s.pod.name", "billing-xyz-1")
	res2.Attributes().PutStr("service.version", "v2.0.4")

	sl2 := rl2.ScopeLogs().AppendEmpty()
	sl2.Scope().SetName("billing.logger")

	errLog2 := sl2.LogRecords().AppendEmpty()
	errLog2.SetSeverityText("ERROR")
	errLog2.Body().SetStr("charge failed: upstream timeout from api-gateway")
	errLog2.SetTimestamp(pcommon.Timestamp(time.Date(2026, 3, 15, 10, 31, 2, 400_000_000, time.UTC).UnixNano()))
	errLog2.SetTraceID([16]byte{0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x00})

	return ld
}

// buildRealisticProfiles creates pprofile.Profiles with a realistic Go CPU profile:
// multiple stacks, multiple samples, realistic function names from Go stdlib.
func buildRealisticProfiles() pprofile.Profiles {
	pd := pprofile.NewProfiles()
	dict := pd.Dictionary()

	// StringTable: realistic Go function names + type/unit strings
	strings := []string{
		"cpu",                                 // 0
		"nanoseconds",                         // 1
		"runtime.gcBgMarkWorker",              // 2
		"runtime.gcDrain",                     // 3
		"runtime.mallocgc",                    // 4
		"net/http.(*conn).serve",              // 5
		"net/http.serverHandler.ServeHTTP",    // 6
		"main.handleRequest",                  // 7
		"database/sql.(*DB).QueryContext",     // 8
		"database/sql.(*DB).query",            // 9
		"database/sql.(*DB).execDC",           // 10
		"encoding/json.(*Encoder).Encode",     // 11
		"runtime.goexit",                      // 12
		"runtime.systemstack",                 // 13
		"runtime.gcMarkTermination",           // 14
		"runtime.(*mcache).nextFree",          // 15
		"github.com/lib/pq.(*conn).Query",    // 16
	}
	for _, s := range strings {
		dict.StringTable().Append(s)
	}

	// FunctionTable: one entry per function
	for i := range strings {
		fn := dict.FunctionTable().AppendEmpty()
		fn.SetNameStrindex(int32(i))
	}

	// LocationTable: one location per function (1:1 for simplicity)
	for i := range strings {
		loc := dict.LocationTable().AppendEmpty()
		line := loc.Lines().AppendEmpty()
		line.SetFunctionIndex(int32(i))
	}

	// StackTable: realistic call stacks
	// Stack 0: GC drain path — runtime.goexit → runtime.gcBgMarkWorker → runtime.gcDrain
	st0 := dict.StackTable().AppendEmpty()
	st0.LocationIndices().Append(12) // runtime.goexit
	st0.LocationIndices().Append(2)  // runtime.gcBgMarkWorker
	st0.LocationIndices().Append(3)  // runtime.gcDrain

	// Stack 1: GC mark termination — runtime.systemstack → runtime.gcMarkTermination
	st1 := dict.StackTable().AppendEmpty()
	st1.LocationIndices().Append(13) // runtime.systemstack
	st1.LocationIndices().Append(14) // runtime.gcMarkTermination

	// Stack 2: HTTP → handler → db query — full request path
	st2 := dict.StackTable().AppendEmpty()
	st2.LocationIndices().Append(12) // runtime.goexit
	st2.LocationIndices().Append(5)  // net/http.(*conn).serve
	st2.LocationIndices().Append(6)  // net/http.serverHandler.ServeHTTP
	st2.LocationIndices().Append(7)  // main.handleRequest
	st2.LocationIndices().Append(8)  // database/sql.(*DB).QueryContext
	st2.LocationIndices().Append(9)  // database/sql.(*DB).query
	st2.LocationIndices().Append(16) // github.com/lib/pq.(*conn).Query

	// Stack 3: HTTP → handler → json encode
	st3 := dict.StackTable().AppendEmpty()
	st3.LocationIndices().Append(12) // runtime.goexit
	st3.LocationIndices().Append(5)  // net/http.(*conn).serve
	st3.LocationIndices().Append(6)  // net/http.serverHandler.ServeHTTP
	st3.LocationIndices().Append(7)  // main.handleRequest
	st3.LocationIndices().Append(11) // encoding/json.(*Encoder).Encode

	// Stack 4: allocation — runtime.mallocgc from handler
	st4 := dict.StackTable().AppendEmpty()
	st4.LocationIndices().Append(12) // runtime.goexit
	st4.LocationIndices().Append(5)  // net/http.(*conn).serve
	st4.LocationIndices().Append(7)  // main.handleRequest
	st4.LocationIndices().Append(4)  // runtime.mallocgc
	st4.LocationIndices().Append(15) // runtime.(*mcache).nextFree

	// Profile from api-gateway
	rp := pd.ResourceProfiles().AppendEmpty()
	res := rp.Resource()
	res.Attributes().PutStr("service.name", "api-gateway")
	res.Attributes().PutStr("host.name", "node-01")
	res.Attributes().PutStr("k8s.pod.name", "api-gw-abc-1")
	res.Attributes().PutStr("service.version", "v3.1.0")

	sp := rp.ScopeProfiles().AppendEmpty()
	p := sp.Profiles().AppendEmpty()
	p.SampleType().SetTypeStrindex(0) // "cpu"
	p.SampleType().SetUnitStrindex(1) // "nanoseconds"
	p.SetTime(pcommon.Timestamp(time.Date(2026, 3, 15, 10, 32, 0, 0, time.UTC).UnixNano()))

	// Samples with varying counts — GC dominates
	sampleDefs := []struct {
		stackIdx int32
		value    int64
	}{
		{0, 450}, // gcDrain — hottest
		{1, 120}, // gcMarkTermination
		{2, 200}, // db query path
		{3, 80},  // json encode
		{4, 150}, // malloc/allocation
	}
	for _, sd := range sampleDefs {
		s := p.Samples().AppendEmpty()
		s.SetStackIndex(sd.stackIdx)
		s.Values().Append(sd.value)
	}

	return pd
}

// TestIntegration_RealDataPipeline is a full pipeline test:
// realistic OTel pdata → mappers → Parquet → DuckDB → validate.
func TestIntegration_RealDataPipeline(t *testing.T) {
	// --- Map all signals ---
	traces := buildRealisticTraces()
	metrics := buildRealisticMetrics()
	logs := buildRealisticLogs()
	profiles := buildRealisticProfiles()

	var rows []schema.UnifiedRow
	rows = append(rows, mapTraces(traces)...)
	rows = append(rows, mapMetrics(metrics)...)
	rows = append(rows, mapLogs(logs)...)
	rows = append(rows, mapProfiles(profiles)...)

	if len(rows) == 0 {
		t.Fatal("no rows produced")
	}

	// Sort like the writer does
	sort.Slice(rows, func(i, j int) bool {
		if rows[i].Service != rows[j].Service {
			return rows[i].Service < rows[j].Service
		}
		if rows[i].Signal != rows[j].Signal {
			return rows[i].Signal < rows[j].Signal
		}
		return rows[i].Timestamp < rows[j].Timestamp
	})

	// --- Write Parquet ---
	f, err := os.CreateTemp(t.TempDir(), "integration-*.parquet")
	if err != nil {
		t.Fatal(err)
	}
	path := f.Name()

	w := parquet.NewGenericWriter[schema.UnifiedRow](f,
		parquet.Compression(&zstd.Codec{Level: zstd.SpeedDefault}),
	)
	if _, err := w.Write(rows); err != nil {
		t.Fatal(err)
	}
	if err := w.Close(); err != nil {
		t.Fatal(err)
	}
	f.Close()

	t.Logf("wrote %d rows to %s", len(rows), path)

	// --- Query with DuckDB ---
	db, err := sql.Open("duckdb", "")
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()

	_, err = db.Exec(fmt.Sprintf(`CREATE VIEW telemetry AS SELECT * FROM read_parquet('%s')`, path))
	if err != nil {
		t.Fatal(err)
	}

	scalar := func(q string) string {
		var v string
		if err := db.QueryRow(q).Scan(&v); err != nil {
			t.Fatalf("query %q: %v", q, err)
		}
		return v
	}

	scalarInt := func(q string) int {
		var v int
		if err := db.QueryRow(q).Scan(&v); err != nil {
			t.Fatalf("query %q: %v", q, err)
		}
		return v
	}

	scalarFloat := func(q string) float64 {
		var v float64
		if err := db.QueryRow(q).Scan(&v); err != nil {
			t.Fatalf("query %q: %v", q, err)
		}
		return v
	}

	queryStrings := func(q string) []string {
		rows, err := db.Query(q)
		if err != nil {
			t.Fatalf("query: %v", err)
		}
		defer rows.Close()
		var result []string
		for rows.Next() {
			var s string
			if err := rows.Scan(&s); err != nil {
				t.Fatal(err)
			}
			result = append(result, s)
		}
		return result
	}

	// --- Validate schema ---
	t.Run("total_rows", func(t *testing.T) {
		n := scalarInt(`SELECT count(*) FROM telemetry`)
		if n != len(rows) {
			t.Errorf("got %d rows, want %d", n, len(rows))
		}
		t.Logf("total rows: %d", n)
	})

	t.Run("all_four_signals", func(t *testing.T) {
		signals := queryStrings(`SELECT DISTINCT signal FROM telemetry ORDER BY signal`)
		want := []string{"log", "metric", "profile", "span"}
		if len(signals) != 4 {
			t.Fatalf("signals = %v, want %v", signals, want)
		}
		for i, s := range signals {
			if s != want[i] {
				t.Errorf("signal[%d] = %q, want %q", i, s, want[i])
			}
		}
	})

	t.Run("two_services", func(t *testing.T) {
		services := queryStrings(`SELECT DISTINCT service FROM telemetry ORDER BY service`)
		if len(services) != 2 || services[0] != "api-gateway" || services[1] != "billing" {
			t.Errorf("services = %v, want [api-gateway, billing]", services)
		}
	})

	t.Run("sort_order", func(t *testing.T) {
		violations := scalarInt(`
			WITH numbered AS (
				SELECT service, signal, timestamp, row_number() OVER () AS rn
				FROM telemetry
			)
			SELECT count(*) FROM numbered a
			JOIN numbered b ON b.rn = a.rn + 1
			WHERE (a.service > b.service)
			   OR (a.service = b.service AND a.signal > b.signal)
			   OR (a.service = b.service AND a.signal = b.signal AND a.timestamp > b.timestamp)
		`)
		if violations > 0 {
			t.Errorf("%d sort order violations", violations)
		}
	})

	// --- Validate traces ---
	t.Run("spans/count", func(t *testing.T) {
		n := scalarInt(`SELECT count(*) FROM telemetry WHERE signal='span'`)
		if n != 4 {
			t.Errorf("span count = %d, want 4", n)
		}
	})

	t.Run("spans/cross_service_trace", func(t *testing.T) {
		// The error trace should have spans from both api-gateway and billing
		n := scalarInt(`
			SELECT count(DISTINCT service)
			FROM telemetry
			WHERE signal='span' AND trace_id='ffeeddccbbaa99887766554433221100'
		`)
		if n != 2 {
			t.Errorf("services in error trace = %d, want 2", n)
		}
	})

	t.Run("spans/parent_child", func(t *testing.T) {
		// billing's POST /charge should have parent = api-gateway's POST /orders
		parentSid := scalar(`
			SELECT span_id FROM telemetry
			WHERE signal='span' AND name='POST /orders' AND service='api-gateway'
		`)
		childParent := scalar(`
			SELECT parent_span_id FROM telemetry
			WHERE signal='span' AND name='POST /charge' AND service='billing'
		`)
		if parentSid != childParent {
			t.Errorf("parent-child mismatch: parent span_id=%s, child parent_span_id=%s", parentSid, childParent)
		}
	})

	t.Run("spans/error_status", func(t *testing.T) {
		n := scalarInt(`SELECT count(*) FROM telemetry WHERE signal='span' AND status='Error'`)
		if n != 2 {
			t.Errorf("error spans = %d, want 2 (POST /orders + POST /charge)", n)
		}
	})

	t.Run("spans/duration_ms", func(t *testing.T) {
		// GET /users/123: 150ms (converted from OTel ns at write time, see schema.UnitSpanDuration)
		dur := scalarFloat(`SELECT value FROM telemetry WHERE signal='span' AND name='GET /users/123'`)
		if dur != 150 {
			t.Errorf("GET /users/123 duration = %f, want 150 (ms)", dur)
		}
	})

	t.Run("spans/events_and_metadata", func(t *testing.T) {
		// Events is a nested Parquet struct — check presence via len() and content via CAST to VARCHAR
		eventCount := scalarInt(`
			SELECT len(events) FROM telemetry
			WHERE signal='span' AND name='POST /orders'
		`)
		if eventCount == 0 {
			t.Fatal("POST /orders span has no events, want exception event")
		}

		eventName := scalar(`
			SELECT events[1].name FROM telemetry
			WHERE signal='span' AND name='POST /orders'
		`)
		if eventName != "exception" {
			t.Errorf("first event name = %q, want exception", eventName)
		}

		// Metadata is a JSON string stored as varchar — check on the span that has attributes
		metadataJSON := scalar(`
			SELECT metadata FROM telemetry
			WHERE signal='span' AND name='GET /users/123'
		`)
		if !strings.Contains(metadataJSON, "http.method") {
			t.Errorf("metadata missing http.method: %s", metadataJSON)
		}
		if !strings.Contains(metadataJSON, "http.route") {
			t.Errorf("metadata missing http.route: %s", metadataJSON)
		}
	})

	// --- Validate metrics ---
	t.Run("metrics/gauge_heap", func(t *testing.T) {
		n := scalarInt(`
			SELECT count(*) FROM telemetry
			WHERE signal='metric' AND name='process.runtime.go.mem.heap_alloc'
		`)
		if n != 5 {
			t.Errorf("heap_alloc points = %d, want 5", n)
		}

		maxHeap := scalarFloat(`
			SELECT max(value) FROM telemetry
			WHERE signal='metric' AND name='process.runtime.go.mem.heap_alloc'
		`)
		if maxHeap != 400_000_000 {
			t.Errorf("max heap = %f, want 400000000", maxHeap)
		}
	})

	t.Run("metrics/sum_request_count", func(t *testing.T) {
		val := scalarFloat(`
			SELECT value FROM telemetry
			WHERE signal='metric' AND name='http.server.request.count'
		`)
		if val != 1542 {
			t.Errorf("request count = %f, want 1542", val)
		}
	})

	t.Run("metrics/histogram_avg", func(t *testing.T) {
		// Histogram: sum/count = 8500/100 = 85
		val := scalarFloat(`
			SELECT value FROM telemetry
			WHERE signal='metric' AND name='http.server.duration'
		`)
		if val != 85 {
			t.Errorf("histogram avg = %f, want 85", val)
		}
	})

	// --- Validate logs ---
	t.Run("logs/count", func(t *testing.T) {
		n := scalarInt(`SELECT count(*) FROM telemetry WHERE signal='log'`)
		if n != 4 {
			t.Errorf("log count = %d, want 4", n)
		}
	})

	t.Run("logs/severity_levels", func(t *testing.T) {
		levels := queryStrings(`
			SELECT DISTINCT status FROM telemetry
			WHERE signal='log' ORDER BY status
		`)
		if len(levels) != 3 {
			t.Errorf("severity levels = %v, want [ERROR, INFO, WARN]", levels)
		}
	})

	t.Run("logs/trace_correlation", func(t *testing.T) {
		// Error logs should be correlated to the error trace
		n := scalarInt(`
			SELECT count(*) FROM telemetry
			WHERE signal='log' AND trace_id='ffeeddccbbaa99887766554433221100'
		`)
		if n != 2 {
			t.Errorf("trace-correlated logs = %d, want 2 (one per service)", n)
		}
	})

	t.Run("logs/cross_service", func(t *testing.T) {
		services := queryStrings(`
			SELECT DISTINCT service FROM telemetry
			WHERE signal='log' AND status='ERROR' ORDER BY service
		`)
		if len(services) != 2 || services[0] != "api-gateway" || services[1] != "billing" {
			t.Errorf("error log services = %v, want [api-gateway, billing]", services)
		}
	})

	// --- Validate profiles ---
	t.Run("profiles/count", func(t *testing.T) {
		n := scalarInt(`SELECT count(*) FROM telemetry WHERE signal='profile'`)
		if n != 5 {
			t.Errorf("profile samples = %d, want 5", n)
		}
	})

	t.Run("profiles/top_function_is_gc", func(t *testing.T) {
		topFunc := scalar(`
			SELECT stacktrace[len(stacktrace)] AS leaf
			FROM telemetry
			WHERE signal='profile'
			GROUP BY leaf
			ORDER BY sum(value) DESC
			LIMIT 1
		`)
		if !strings.Contains(topFunc, "gcDrain") {
			t.Errorf("top profile function = %q, want to contain gcDrain", topFunc)
		}
	})

	t.Run("profiles/total_gc_samples", func(t *testing.T) {
		// gcDrain(450) + gcMarkTermination(120) = 570 GC samples
		gcSamples := scalarFloat(`
			SELECT sum(value) FROM telemetry
			WHERE signal='profile'
			  AND (stacktrace[len(stacktrace)] LIKE '%gc%' OR stacktrace[len(stacktrace)] LIKE '%GC%')
		`)
		if gcSamples != 570 {
			t.Errorf("GC samples = %f, want 570", gcSamples)
		}
	})

	t.Run("profiles/stack_depth", func(t *testing.T) {
		// DB query stack should have 7 frames
		maxDepth := scalarInt(`
			SELECT max(len(stacktrace)) FROM telemetry WHERE signal='profile'
		`)
		if maxDepth != 7 {
			t.Errorf("max stack depth = %d, want 7", maxDepth)
		}
	})

	t.Run("profiles/realistic_function_names", func(t *testing.T) {
		// Verify we can find real Go stdlib function names in stacks
		dbQueryRows := scalarInt(`
			SELECT count(*) FROM telemetry
			WHERE signal='profile'
			  AND list_contains(stacktrace, 'database/sql.(*DB).QueryContext')
		`)
		if dbQueryRows == 0 {
			t.Error("no profile samples contain database/sql.(*DB).QueryContext")
		}

		httpRows := scalarInt(`
			SELECT count(*) FROM telemetry
			WHERE signal='profile'
			  AND list_contains(stacktrace, 'net/http.(*conn).serve')
		`)
		if httpRows == 0 {
			t.Error("no profile samples contain net/http.(*conn).serve")
		}
	})

	t.Run("profiles/name_and_kind", func(t *testing.T) {
		name := scalar(`SELECT DISTINCT name FROM telemetry WHERE signal='profile'`)
		if name != "profile.cpu" {
			t.Errorf("profile name = %q, want profile.cpu", name)
		}
		kind := scalar(`SELECT DISTINCT kind FROM telemetry WHERE signal='profile'`)
		if kind != "cpu" {
			t.Errorf("profile kind = %q, want cpu", kind)
		}
	})

	// --- Cross-signal validation ---
	t.Run("cross_signal/all_columns_populated", func(t *testing.T) {
		// Every row should have timestamp, signal, service, host, pod, name
		nulls := scalarInt(`
			SELECT count(*) FROM telemetry
			WHERE timestamp IS NULL OR signal IS NULL OR service = '' OR host = '' OR name = ''
		`)
		if nulls > 0 {
			t.Errorf("%d rows have missing core fields", nulls)
		}
	})

	t.Run("cross_signal/deploy_version_set", func(t *testing.T) {
		n := scalarInt(`
			SELECT count(DISTINCT deploy_version) FROM telemetry
			WHERE deploy_version != ''
		`)
		if n != 2 {
			t.Errorf("deploy versions = %d, want 2 (v3.1.0 + v2.0.4)", n)
		}
	})
}
