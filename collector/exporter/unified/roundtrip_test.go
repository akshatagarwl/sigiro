package unified

import (
	"context"
	"database/sql"
	"fmt"
	"os"
	"sort"
	"testing"
	"time"

	"github.com/akshatagarwl/observe/internal/query"
	"github.com/akshatagarwl/observe/pkg/schema"
	_ "github.com/duckdb/duckdb-go/v2"
	"github.com/parquet-go/parquet-go"
	"github.com/parquet-go/parquet-go/compress/zstd"
	"go.opentelemetry.io/collector/pdata/pcommon"
	"go.opentelemetry.io/collector/pdata/ptrace"
)

// TestRoundtrip_SpanDurationUnit is a regression test for the ns/ms unit mismatch.
// It writes spans through the OTel mapper, reads them through the query layer,
// and verifies that durations are in milliseconds end-to-end.
// If this test fails, the mapper and query layer disagree on the unit of span duration.
func TestRoundtrip_SpanDurationUnit(t *testing.T) {
	// Build OTel traces: one fast span (50ms) and one slow span (2500ms).
	td := ptrace.NewTraces()
	rs := td.ResourceSpans().AppendEmpty()
	res := rs.Resource()
	res.Attributes().PutStr("service.name", "roundtrip-svc")
	res.Attributes().PutStr("host.name", "test-node")
	res.Attributes().PutStr("k8s.pod.name", "test-pod")

	ss := rs.ScopeSpans().AppendEmpty()

	base := time.Date(2026, 3, 15, 10, 0, 0, 0, time.UTC)

	// Fast span: 50ms = 50,000,000 ns
	fast := ss.Spans().AppendEmpty()
	fast.SetName("GET /fast")
	fast.SetTraceID([16]byte{0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10})
	fast.SetSpanID([8]byte{0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08})
	fast.SetStartTimestamp(pcommon.Timestamp(base.UnixNano()))
	fast.SetEndTimestamp(pcommon.Timestamp(base.Add(50 * time.Millisecond).UnixNano()))
	fast.Status().SetCode(ptrace.StatusCodeOk)
	fast.SetKind(ptrace.SpanKindServer)

	// Slow span: 2500ms = 2,500,000,000 ns — should be caught by value > 1000 filter
	slow := ss.Spans().AppendEmpty()
	slow.SetName("POST /slow")
	slow.SetTraceID([16]byte{0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20})
	slow.SetSpanID([8]byte{0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18})
	slow.SetStartTimestamp(pcommon.Timestamp(base.Add(1 * time.Second).UnixNano()))
	slow.SetEndTimestamp(pcommon.Timestamp(base.Add(1*time.Second + 2500*time.Millisecond).UnixNano()))
	slow.Status().SetCode(ptrace.StatusCodeOk)
	slow.SetKind(ptrace.SpanKindServer)

	// Map through the OTel mapper
	rows := mapTraces(td)
	if len(rows) != 2 {
		t.Fatalf("expected 2 rows, got %d", len(rows))
	}

	// Verify mapper output directly: values should be in ms
	for _, r := range rows {
		if r.Name == "GET /fast" && *r.Value != 50 {
			t.Errorf("mapper: GET /fast duration = %f, want 50 (ms)", *r.Value)
		}

		if r.Name == "POST /slow" && *r.Value != 2500 {
			t.Errorf("mapper: POST /slow duration = %f, want 2500 (ms)", *r.Value)
		}

		if *r.Unit != schema.UnitSpanDuration {
			t.Errorf("mapper: unit = %q, want %q", *r.Unit, schema.UnitSpanDuration)
		}
	}

	// Sort and write to Parquet
	sort.Slice(rows, func(i, j int) bool {
		if rows[i].Service != rows[j].Service {
			return rows[i].Service < rows[j].Service
		}

		if rows[i].Signal != rows[j].Signal {
			return rows[i].Signal < rows[j].Signal
		}

		return rows[i].Timestamp < rows[j].Timestamp
	})

	f, err := os.CreateTemp(t.TempDir(), "roundtrip-*.parquet")
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

	// Load into DuckDB and query through the investigation layer
	db, err := sql.Open("duckdb", "")
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()

	_, err = db.Exec(fmt.Sprintf(`CREATE VIEW roundtrip AS SELECT * FROM read_parquet('%s')`, path))
	if err != nil {
		t.Fatal(err)
	}

	block, err := query.Investigate(context.Background(), db, "roundtrip", query.InvestigateRequest{
		Service: "roundtrip-svc",
		From:    base.Add(-1 * time.Minute),
		To:      base.Add(5 * time.Minute),
	})
	if err != nil {
		t.Fatal(err)
	}

	// All spans returned, sorted by duration desc. POST /slow (2500ms) should be first.
	if len(block.Spans) != 2 {
		t.Fatalf("expected 2 spans, got %d", len(block.Spans))
	}

	if block.Spans[0].Name != "POST /slow" {
		t.Errorf("first span = %q, want POST /slow (highest duration)", block.Spans[0].Name)
	}

	if block.Spans[0].DurationMs != 2500 {
		t.Errorf("span DurationMs = %f, want 2500", block.Spans[0].DurationMs)
	}
}
