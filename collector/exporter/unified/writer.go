package unified

import (
	"bytes"
	"context"
	"fmt"
	"sort"
	"time"

	"github.com/AkshatM/observe/pkg/schema"
	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/service/s3"
	"github.com/parquet-go/parquet-go"
	"github.com/parquet-go/parquet-go/compress/zstd"
	"go.uber.org/zap"
)

// ParquetWriter writes sorted UnifiedRows as Parquet files to S3.
type ParquetWriter struct {
	bucket   string
	s3Client *s3.Client
	logger   *zap.Logger
	nodeID   string
}

// NewParquetWriter creates a new ParquetWriter.
func NewParquetWriter(bucket string, s3Client *s3.Client, logger *zap.Logger) *ParquetWriter {
	return &ParquetWriter{
		bucket:   bucket,
		s3Client: s3Client,
		logger:   logger,
		nodeID:   fmt.Sprintf("%d", time.Now().UnixNano()%100000),
	}
}

// Write sorts rows by (service, signal, timestamp), writes a Parquet file,
// and uploads it to S3 at the appropriate partition path.
func (w *ParquetWriter) Write(ctx context.Context, rows []schema.UnifiedRow) error {
	if len(rows) == 0 {
		return nil
	}

	sort.Slice(rows, func(i, j int) bool {
		if rows[i].Service != rows[j].Service {
			return rows[i].Service < rows[j].Service
		}
		if rows[i].Signal != rows[j].Signal {
			return rows[i].Signal < rows[j].Signal
		}
		return rows[i].Timestamp < rows[j].Timestamp
	})

	// Determine partition from first row's timestamp
	ts := time.Unix(0, rows[0].Timestamp).UTC()
	key := fmt.Sprintf("year=%04d/month=%02d/day=%02d/hour=%02d/%s-%d.parquet",
		ts.Year(), ts.Month(), ts.Day(), ts.Hour(),
		w.nodeID, time.Now().UnixNano())

	var buf bytes.Buffer
	writer := parquet.NewGenericWriter[schema.UnifiedRow](&buf,
		parquet.Compression(&zstd.Codec{Level: zstd.SpeedBetterCompression}),
	)
	if _, err := writer.Write(rows); err != nil {
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

	w.logger.Info("flushed parquet",
		zap.String("key", key),
		zap.Int("rows", len(rows)),
		zap.Int("bytes", buf.Len()),
	)
	return nil
}
