package unified

import (
	"bytes"
	"fmt"
	"math/rand/v2"
	"sort"
	"testing"
	"time"

	"github.com/akshatagarwl/sigiro/pkg/schema"
	"github.com/parquet-go/parquet-go"
	"github.com/parquet-go/parquet-go/compress/zstd"
)

// TestCompression measures compression with the current writer config:
// zstd SpeedDefault, 1MB page buffers, bloom on trace_id.
func TestCompression(t *testing.T) {
	const numRows = 10000

	rows := generateTestRows(numRows)
	sortRows(rows)

	var buf bytes.Buffer
	buf.Grow(len(rows) * 256)

	w := parquet.NewGenericWriter[schema.UnifiedRow](&buf,
		parquet.Compression(&zstd.Codec{Level: zstd.SpeedDefault}),
		parquet.PageBufferSize(1*1024*1024),
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

	compressedSize := buf.Len()
	rawSize := numRows * 200 // rough estimate: ~200 bytes per uncompressed row
	ratio := float64(rawSize) / float64(compressedSize)

	t.Logf("rows=%d raw≈%d compressed=%d ratio=%.1fx", numRows, rawSize, compressedSize, ratio)

	if ratio < 7.0 {
		t.Errorf("compression ratio %.1fx is below 7x floor", ratio)
	}
}

// TestCompression_SpeedDefaultVsBest compares SpeedDefault (current, fast) vs
// SpeedBestCompression (old, slow) to quantify the size tradeoff.
// SpeedDefault is ~3-5x faster with only ~5-8% larger files.
func TestCompression_SpeedDefaultVsBest(t *testing.T) {
	const numRows = 10000

	rows := generateTestRows(numRows)
	sortRows(rows)

	writeWith := func(opts ...parquet.WriterOption) int {
		var buf bytes.Buffer
		buf.Grow(len(rows) * 256)

		w := parquet.NewGenericWriter[schema.UnifiedRow](&buf, opts...)
		if _, err := w.Write(rows); err != nil {
			t.Fatal(err)
		}

		if err := w.Close(); err != nil {
			t.Fatal(err)
		}

		return buf.Len()
	}

	sortOpts := parquet.SortingWriterConfig(
		parquet.SortingColumns(
			parquet.Ascending("service"),
			parquet.Ascending("signal"),
			parquet.Ascending("timestamp"),
		),
	)
	bloom := parquet.BloomFilters(parquet.SplitBlockFilter(10, "trace_id"))

	defaultSize := writeWith(
		parquet.Compression(&zstd.Codec{Level: zstd.SpeedDefault}),
		parquet.PageBufferSize(1*1024*1024),
		bloom, sortOpts,
	)

	bestSize := writeWith(
		parquet.Compression(&zstd.Codec{Level: zstd.SpeedBestCompression}),
		parquet.PageBufferSize(8*1024*1024),
		bloom, sortOpts,
	)

	overhead := 100.0 * float64(defaultSize-bestSize) / float64(bestSize)
	t.Logf("SpeedDefault=%d bytes  SpeedBestCompression=%d bytes  overhead=%.1f%%", defaultSize, bestSize, overhead)

	// SpeedDefault should be within 15% of SpeedBestCompression.
	// Background recompression can re-compress settled data offline.
	if overhead > 15.0 {
		t.Errorf("SpeedDefault overhead %.1f%% exceeds 15%% threshold", overhead)
	}
}

func generateTestRows(n int) []schema.UnifiedRow {
	base := time.Date(2026, 3, 15, 10, 0, 0, 0, time.UTC)
	services := []string{"checkout", "payments", "inventory", "frontend", "cart"}
	signals := []string{"metric", "span", "log", "profile"}
	names := []string{
		"GET /api/checkout", "POST /api/pay", "GET /api/inventory",
		"http.request.duration", "process.cpu.time", "db.query.duration",
		"connection refused", "request completed", "timeout exceeded",
		"cpu", "alloc_space",
	}

	rows := make([]schema.UnifiedRow, n)
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

	return rows
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
