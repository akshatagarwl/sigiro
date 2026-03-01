package unified

import (
	"context"
	"sync"
	"time"

	"github.com/akshatagarwl/observe/pkg/schema"
	"go.opentelemetry.io/collector/component"
	"go.opentelemetry.io/collector/pdata/plog"
	"go.opentelemetry.io/collector/pdata/pmetric"
	"go.opentelemetry.io/collector/pdata/pprofile"
	"go.opentelemetry.io/collector/pdata/ptrace"
	"go.uber.org/zap"
)

// Exporter buffers incoming OTLP data, maps it to UnifiedRow,
// and periodically flushes Parquet files to S3.
type Exporter struct {
	mu     sync.Mutex
	buffer []schema.UnifiedRow
	writer *ParquetWriter
	config Config
	logger *zap.Logger
	done   chan struct{}
}

// NewExporter creates a new Exporter.
func NewExporter(cfg Config, writer *ParquetWriter, logger *zap.Logger) *Exporter {
	return &Exporter{
		writer: writer,
		config: cfg,
		logger: logger,
		done:   make(chan struct{}),
	}
}

// Start begins the periodic flush loop.
func (e *Exporter) Start(_ context.Context, _ component.Host) error {
	interval := e.config.FlushInterval
	if interval == 0 {
		interval = 15 * time.Second
	}
	ticker := time.NewTicker(interval)
	go func() {
		defer ticker.Stop()
		for {
			select {
			case <-ticker.C:
				if err := e.flush(context.Background()); err != nil {
					e.logger.Error("flush failed", zap.Error(err))
				}
			case <-e.done:
				return
			}
		}
	}()
	return nil
}

// Shutdown flushes remaining data and stops the exporter.
func (e *Exporter) Shutdown(ctx context.Context) error {
	close(e.done)
	return e.flush(ctx)
}

// ConsumeMetrics maps OTLP metrics to UnifiedRows and appends to buffer.
func (e *Exporter) ConsumeMetrics(_ context.Context, md pmetric.Metrics) error {
	rows := mapMetrics(md)
	e.mu.Lock()
	e.buffer = append(e.buffer, rows...)
	e.mu.Unlock()
	return nil
}

// ConsumeTraces maps OTLP traces to UnifiedRows and appends to buffer.
func (e *Exporter) ConsumeTraces(_ context.Context, td ptrace.Traces) error {
	rows := mapTraces(td)
	e.mu.Lock()
	e.buffer = append(e.buffer, rows...)
	e.mu.Unlock()
	return nil
}

// ConsumeLogs maps OTLP logs to UnifiedRows and appends to buffer.
func (e *Exporter) ConsumeLogs(_ context.Context, ld plog.Logs) error {
	rows := mapLogs(ld)
	e.mu.Lock()
	e.buffer = append(e.buffer, rows...)
	e.mu.Unlock()
	return nil
}

// ConsumeProfiles maps OTLP profiles to UnifiedRows and appends to buffer.
func (e *Exporter) ConsumeProfiles(_ context.Context, pd pprofile.Profiles) error {
	rows := mapProfiles(pd)
	e.mu.Lock()
	e.buffer = append(e.buffer, rows...)
	e.mu.Unlock()
	return nil
}

func (e *Exporter) flush(ctx context.Context) error {
	e.mu.Lock()
	if len(e.buffer) == 0 {
		e.mu.Unlock()
		return nil
	}
	snapshot := e.buffer
	e.buffer = make([]schema.UnifiedRow, 0, cap(snapshot))
	e.mu.Unlock()

	if err := e.writer.Write(ctx, snapshot); err != nil {
		// Prepend failed batch so it is retried on the next tick.
		// Rows that arrived during the write attempt end up after the
		// retried rows, preserving chronological order.
		// NOTE: sustained failures will grow the buffer without bound
		// until memory-bounded ring buffers are implemented.
		e.mu.Lock()
		e.buffer = append(snapshot, e.buffer...)
		e.mu.Unlock()
		return err
	}
	return nil
}
