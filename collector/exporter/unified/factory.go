package unified

import (
	"context"
	"fmt"
	"sync"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/credentials"
	"github.com/aws/aws-sdk-go-v2/service/s3"
	"go.opentelemetry.io/collector/component"
	"go.opentelemetry.io/collector/config/configoptional"
	"go.opentelemetry.io/collector/config/configretry"
	"go.opentelemetry.io/collector/exporter"
	"go.opentelemetry.io/collector/exporter/exporterhelper"
	"go.opentelemetry.io/collector/exporter/exporterhelper/xexporterhelper"
	"go.opentelemetry.io/collector/exporter/xexporter"
	"go.opentelemetry.io/collector/pdata/plog"
	"go.opentelemetry.io/collector/pdata/pmetric"
	"go.opentelemetry.io/collector/pdata/pprofile"
	"go.opentelemetry.io/collector/pdata/ptrace"
)

const typeStr = "unified_parquet"

// NewFactory creates a factory for the unified Parquet exporter.
func NewFactory() xexporter.Factory {
	return xexporter.NewFactory(
		component.MustNewType(typeStr),
		createDefaultConfig,
		xexporter.WithMetrics(createMetricsExporter, component.StabilityLevelAlpha),
		xexporter.WithTraces(createTracesExporter, component.StabilityLevelAlpha),
		xexporter.WithLogs(createLogsExporter, component.StabilityLevelAlpha),
		xexporter.WithProfiles(createProfilesExporter, component.StabilityLevelAlpha),
	)
}

func createDefaultConfig() component.Config {
	return &Config{
		S3Bucket: "observe",
		S3Region: "us-east-1",
		Retry:    configretry.NewDefaultBackOffConfig(),
		Timeout:  exporterhelper.NewDefaultTimeoutConfig(),
		Queue:    configoptional.Some(exporterhelper.NewDefaultQueueConfig()),
	}
}

// sharedWriter holds the single ParquetWriter instance shared across all
// signal-specific exporters. Each push function writes through this writer.
var (
	writerMu     sync.Mutex
	sharedWriter *ParquetWriter
)

func getSharedWriter(cfg *Config) *ParquetWriter {
	writerMu.Lock()
	defer writerMu.Unlock()

	if sharedWriter == nil {
		s3Client := newS3Client(cfg)
		sharedWriter = NewParquetWriter(cfg.S3Bucket, s3Client)
	}

	return sharedWriter
}

// resetSingleton tears down the shared writer (for tests).
func resetSingleton() {
	writerMu.Lock()
	defer writerMu.Unlock()

	sharedWriter = nil
}

func newS3Client(cfg *Config) *s3.Client {
	opts := func(o *s3.Options) {
		o.Region = cfg.S3Region
		if cfg.S3Endpoint != "" {
			o.BaseEndpoint = aws.String(cfg.S3Endpoint)
			o.UsePathStyle = true
		}

		if cfg.S3AccessKey != "" {
			o.Credentials = credentials.NewStaticCredentialsProvider(
				cfg.S3AccessKey, cfg.S3SecretKey, "",
			)
		}
	}

	return s3.New(s3.Options{}, opts)
}

func helperOptions(cfg *Config) []exporterhelper.Option {
	return []exporterhelper.Option{
		exporterhelper.WithTimeout(cfg.Timeout),
		exporterhelper.WithRetry(cfg.Retry),
		exporterhelper.WithQueue(cfg.Queue),
	}
}

func createMetricsExporter(ctx context.Context, set exporter.Settings, cfg component.Config) (exporter.Metrics, error) {
	c := cfg.(*Config)
	w := getSharedWriter(c)

	return exporterhelper.NewMetrics(ctx, set, cfg,
		func(ctx context.Context, md pmetric.Metrics) error {
			rows := mapMetrics(md)
			return w.Write(ctx, rows)
		},
		helperOptions(c)...,
	)
}

func createTracesExporter(ctx context.Context, set exporter.Settings, cfg component.Config) (exporter.Traces, error) {
	c := cfg.(*Config)
	w := getSharedWriter(c)

	return exporterhelper.NewTraces(ctx, set, cfg,
		func(ctx context.Context, td ptrace.Traces) error {
			rows := mapTraces(td)
			return w.Write(ctx, rows)
		},
		helperOptions(c)...,
	)
}

func createLogsExporter(ctx context.Context, set exporter.Settings, cfg component.Config) (exporter.Logs, error) {
	c := cfg.(*Config)
	w := getSharedWriter(c)

	return exporterhelper.NewLogs(ctx, set, cfg,
		func(ctx context.Context, ld plog.Logs) error {
			rows := mapLogs(ld)
			return w.Write(ctx, rows)
		},
		helperOptions(c)...,
	)
}

func createProfilesExporter(ctx context.Context, set exporter.Settings, cfg component.Config) (xexporter.Profiles, error) {
	c := cfg.(*Config)
	w := getSharedWriter(c)

	return xexporterhelper.NewProfiles(ctx, set, cfg,
		func(ctx context.Context, pd pprofile.Profiles) error {
			rows, err := mapProfiles(pd)
			if err != nil {
				return fmt.Errorf("map profiles: %w", err)
			}

			return w.Write(ctx, rows)
		},
		helperOptions(c)...,
	)
}
