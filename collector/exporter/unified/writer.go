package unified

import (
	"bytes"
	"context"
	"fmt"
	"log/slog"
	"sort"
	"time"

	"github.com/akshatagarwl/observe/pkg/schema"
	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/service/s3"
	"github.com/parquet-go/parquet-go"
	"github.com/parquet-go/parquet-go/compress/zstd"
)

const nsPerHour = int64(time.Hour)

// ParquetWriter writes sorted UnifiedRows as Parquet files to S3.
type ParquetWriter struct {
	bucket   string
	s3Client *s3.Client
	nodeID   string
}

// NewParquetWriter creates a new ParquetWriter.
func NewParquetWriter(bucket string, s3Client *s3.Client) *ParquetWriter {
	return &ParquetWriter{
		bucket:   bucket,
		s3Client: s3Client,
		nodeID:   fmt.Sprintf("%d", time.Now().UnixNano()%100000),
	}
}

// EpochHour returns the Unix timestamp (seconds) of the start of the hour
// containing the given nanosecond timestamp.
func EpochHour(tsNano int64) int64 {
	return (tsNano / nsPerHour) * 3600
}

// Write groups rows by epoch-hour partition, sorts each group by
// (service, signal, timestamp), and uploads a Parquet file per hour.
func (w *ParquetWriter) Write(ctx context.Context, rows []schema.UnifiedRow) error {
	if len(rows) == 0 {
		return nil
	}

	// Group rows by hour bucket.
	byHour := map[int64][]schema.UnifiedRow{}
	for _, r := range rows {
		h := EpochHour(r.Timestamp)
		byHour[h] = append(byHour[h], r)
	}

	for hour, hourRows := range byHour {
		sort.Slice(hourRows, func(i, j int) bool {
			if hourRows[i].Service != hourRows[j].Service {
				return hourRows[i].Service < hourRows[j].Service
			}
			if hourRows[i].Signal != hourRows[j].Signal {
				return hourRows[i].Signal < hourRows[j].Signal
			}
			return hourRows[i].Timestamp < hourRows[j].Timestamp
		})

		key := fmt.Sprintf("%d/%s-%d.parquet",
			hour, w.nodeID, time.Now().UnixNano())

		var buf bytes.Buffer
		writer := parquet.NewGenericWriter[schema.UnifiedRow](&buf,
			parquet.Compression(&zstd.Codec{Level: zstd.SpeedBetterCompression}),
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
	}
	return nil
}
