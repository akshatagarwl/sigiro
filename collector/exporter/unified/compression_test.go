package unified

import (
	"bytes"
	"fmt"
	"math/rand/v2"
	"sort"
	"testing"
	"time"

	"github.com/akshatagarwl/observe/pkg/schema"
	"github.com/parquet-go/parquet-go"
	"github.com/parquet-go/parquet-go/compress/zstd"
)

// TestCompression_DeltaTimestamp measures compression with delta encoding on
// timestamps and bloom filters on trace_id. Run with -v to see sizes.
func TestCompression_DeltaTimestamp(t *testing.T) {
	const numRows = 10000

	base := time.Date(2026, 3, 15, 10, 0, 0, 0, time.UTC)
	services := []string{"checkout", "payments", "inventory", "frontend", "cart"}
	signals := []string{"metric", "span", "log", "profile"}
	names := []string{
		"GET /api/checkout", "POST /api/pay", "GET /api/inventory",
		"http.request.duration", "process.cpu.time", "db.query.duration",
		"connection refused", "request completed", "timeout exceeded",
		"cpu", "alloc_space",
	}

	rows := make([]schema.UnifiedRow, numRows)
	for i := range rows {
		svc := services[i%len(services)]
		sig := signals[i%len(signals)]
		ts := base.Add(time.Duration(i) * 100 * time.Millisecond).UnixNano()
		name := names[i%len(names)]
		val := rand.Float64() * 1000
		status := "OK"

		rows[i] = schema.UnifiedRow{
			Timestamp: ts,
			Signal:    sig,
			Service:   svc,
			Host:      fmt.Sprintf("node-%d", i%3),
			Pod:       fmt.Sprintf("%s-pod-%d", svc, i%5),
			Name:      name,
			Value:     &val,
			Status:    &status,
		}

		if sig == "span" || sig == "log" {
			tid := fmt.Sprintf("%032x", rand.Uint64())
			rows[i].TraceID = &tid

			if sig == "span" {
				sid := fmt.Sprintf("%016x", rand.Uint64())
				rows[i].SpanID = &sid
			}
		}

		if sig == "log" {
			msg := fmt.Sprintf("Request to %s took %dms from %s", name, rand.IntN(5000), svc)
			rows[i].Message = &msg
		}

		if sig == "profile" {
			rows[i].Stacktrace = []string{
				"runtime.main",
				"net/http.(*Server).Serve",
				"net/http.(*conn).serve",
				fmt.Sprintf("%s.Handle%s", svc, name),
			}
		}
	}

	// Sort like the real writer does
	sortRows(rows)

	// Write with current config (delta on timestamp + bloom filters)
	var buf bytes.Buffer
	buf.Grow(len(rows) * 256)

	w := parquet.NewGenericWriter[schema.UnifiedRow](&buf,
		parquet.Compression(&zstd.Codec{Level: zstd.SpeedBetterCompression}),
		parquet.BloomFilters(
			parquet.SplitBlockFilter(10, "trace_id"),
		),
		parquet.SortingWriterConfig(
			parquet.SortingColumns(
				parquet.Ascending("service"),
				parquet.Ascending("signal"),
				parquet.Ascending("timestamp"),
			),
		),
	)
	if _, err := w.Write(rows); err != nil {
		t.Fatal(err)
	}

	if err := w.Close(); err != nil {
		t.Fatal(err)
	}

	optimizedSize := buf.Len()

	// Estimate raw size (rough: each row ~200 bytes uncompressed)
	rawSize := numRows * 200

	ratio := float64(rawSize) / float64(optimizedSize)
	t.Logf("rows=%d raw≈%d bytes compressed=%d bytes ratio=%.1fx",
		numRows, rawSize, optimizedSize, ratio)

	// Sanity: we should get at least 5x compression on structured observability data
	if ratio < 5.0 {
		t.Errorf("compression ratio %.1fx is below 5x target", ratio)
	}
}

func sortRows(rows []schema.UnifiedRow) {
	sort.Slice(rows, func(i, j int) bool {
		if rows[i].Service != rows[j].Service {
			return rows[i].Service < rows[j].Service
		}

		if rows[i].Signal != rows[j].Signal {
			return rows[i].Signal < rows[j].Signal
		}

		return rows[i].Timestamp < rows[j].Timestamp
	})
}
