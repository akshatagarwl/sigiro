package unified

import (
	"bytes"
	"context"
	"fmt"
	"log/slog"
	"sort"
	"sync"
	"time"

	"github.com/akshatagarwl/sigiro/pkg/schema"
	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/service/s3"
	"github.com/parquet-go/parquet-go"
	"github.com/parquet-go/parquet-go/compress/zstd"
)

const nsPerHour = int64(time.Hour)

// ParquetWriter writes sorted UnifiedRows as Parquet files to S3.
//
// Design: parquet-go serializes directly from []UnifiedRow, then AWS SDK uploads
// via PutObject. Append-only — files are never modified or deleted by the app.
// S3 lifecycle rules handle TTL. See docs/architecture/tradeoffs.md §7 for
// alternatives considered (DuckDB httpfs, raw net/http, local WAL + sync)
// and when to revisit.
type ParquetWriter struct {
	bucket   string
	s3Client *s3.Client
	nodeID   string
	bufPool  sync.Pool // pool of *bytes.Buffer, reused across flushes
}

// NewParquetWriter creates a new ParquetWriter.
func NewParquetWriter(bucket string, s3Client *s3.Client) *ParquetWriter {
	// FRAGILE: nodeID is UnixNano mod 100k — only 100k possible values, so
	// two collector instances started in the same 100µs window (or any time
	// mod-collision) will write to the same S3 key, silently overwriting
	// each other's data. Use a UUID instead.
	return &ParquetWriter{
		bucket:   bucket,
		s3Client: s3Client,
		nodeID:   fmt.Sprintf("%d", time.Now().UnixNano()%100000),
	}
}

// EpochHour returns the Unix timestamp (seconds) of the start of the hour
// containing the given nanosecond timestamp.
// FRAGILE: mixes nanosecond input with a seconds output — callers must know
// the convention. The S3 key uses this seconds value, but the Parquet
// timestamp column stores nanoseconds. If anyone passes microseconds or
// seconds by mistake, rows land in the wrong partition silently.
func EpochHour(tsNano int64) int64 {
	return (tsNano / nsPerHour) * 3600
}

// Write sorts rows in-place by (hour, signal, service, timestamp),
// walks (hour, signal) boundaries as zero-copy sub-slices, and uploads
// a Parquet file per (hour, signal) partition. This produces Hive-style
// partitioning on S3: {epochHour}/signal={signal}/{node}-{ts}.parquet
//
// Hive partitioning by signal enables two access patterns from one layout:
// - AI agents glob signal=* to get all signals in one unified scan
// - Dashboard tools (Perses) glob signal=metric to get only metrics
func (w *ParquetWriter) Write(ctx context.Context, rows []schema.UnifiedRow) error {
	if len(rows) == 0 {
		return nil
	}

	// Sort in-place: primary key is (hour, signal), then (service, timestamp).
	sort.Slice(rows, func(i, j int) bool {
		hi, hj := EpochHour(rows[i].Timestamp), EpochHour(rows[j].Timestamp)
		if hi != hj {
			return hi < hj
		}

		if rows[i].Signal != rows[j].Signal {
			return rows[i].Signal < rows[j].Signal
		}

		if rows[i].Service != rows[j].Service {
			return rows[i].Service < rows[j].Service
		}

		return rows[i].Timestamp < rows[j].Timestamp
	})

	// Walk sorted slice, find (hour, signal) boundaries, take sub-slices (zero copy).
	start := 0
	for start < len(rows) {
		hour := EpochHour(rows[start].Timestamp)
		signal := rows[start].Signal

		end := start + 1
		for end < len(rows) && EpochHour(rows[end].Timestamp) == hour && rows[end].Signal == signal {
			end++
		}

		if err := w.writePartition(ctx, hour, signal, rows[start:end]); err != nil {
			return err
		}

		start = end
	}

	return nil
}

func (w *ParquetWriter) writePartition(ctx context.Context, hour int64, signal string, hourRows []schema.UnifiedRow) error {
	key := fmt.Sprintf("%d/signal=%s/%s-%d.parquet",
		hour, signal, w.nodeID, time.Now().UnixNano())

	// FRAGILE: entire hour-partition is buffered in memory before upload.
	// A burst of data for a single hour can OOM the process. No size
	// limit or chunking.
	//
	// FUTURE: if this becomes a problem, the local WAL + DuckDB sync
	// architecture decouples flushing from uploading — collector writes
	// to local disk (no OOM on S3 failure), DuckDB merges + re-sorts +
	// uploads in the background. See docs/architecture/tradeoffs.md §7.
	buf, _ := w.bufPool.Get().(*bytes.Buffer)
	if buf == nil {
		buf = new(bytes.Buffer)
	}

	buf.Reset()

	buf.Grow(len(hourRows) * 256)
	defer w.bufPool.Put(buf)

	writer := parquet.NewGenericWriter[schema.UnifiedRow](buf,
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
	if _, err := writer.Write(hourRows); err != nil {
		return fmt.Errorf("parquet write: %w", err)
	}

	if err := writer.Close(); err != nil {
		return fmt.Errorf("parquet close: %w", err)
	}

	_, err := w.s3Client.PutObject(ctx, &s3.PutObjectInput{
		Bucket: aws.String(w.bucket),
		Key:    aws.String(key),
		Body:   bytes.NewReader(buf.Bytes()),
	})
	if err != nil {
		return fmt.Errorf("s3 put %s: %w", key, err)
	}

	slog.Info("flushed parquet", "key", key, "rows", len(hourRows), "bytes", buf.Len())

	return nil
}
