package unified

import (
	"context"
	"sync"

	"github.com/akshatagarwl/observe/pkg/schema"
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

// pushFunc returns a push function that maps pdata to UnifiedRows and writes them.
func pushFunc[T any](w *ParquetWriter, mapper func(T) []schema.UnifiedRow) func(context.Context, T) error {
	return func(ctx context.Context, data T) error {
		return w.Write(ctx, mapper(data))
	}
}

// pushFuncErr is like pushFunc but for mappers that can return errors (profiles).
func pushFuncErr[T any](w *ParquetWriter, mapper func(T) ([]schema.UnifiedRow, error)) func(context.Context, T) error {
	return func(ctx context.Context, data T) error {
		rows, err := mapper(data)
		if err != nil {
			return err
		}

		return w.Write(ctx, rows)
	}
}

func createMetricsExporter(ctx context.Context, set exporter.Settings, cfg component.Config) (exporter.Metrics, error) {
	c := cfg.(*Config)
	w := getSharedWriter(c)

	return exporterhelper.NewMetrics(ctx, set, cfg, pushFunc(w, mapMetrics), helperOptions(c)...)
}

func createTracesExporter(ctx context.Context, set exporter.Settings, cfg component.Config) (exporter.Traces, error) {
	c := cfg.(*Config)
	w := getSharedWriter(c)

	return exporterhelper.NewTraces(ctx, set, cfg, pushFunc(w, mapTraces), helperOptions(c)...)
}

func createLogsExporter(ctx context.Context, set exporter.Settings, cfg component.Config) (exporter.Logs, error) {
	c := cfg.(*Config)
	w := getSharedWriter(c)

	return exporterhelper.NewLogs(ctx, set, cfg, pushFunc(w, mapLogs), helperOptions(c)...)
}

func createProfilesExporter(ctx context.Context, set exporter.Settings, cfg component.Config) (xexporter.Profiles, error) {
	c := cfg.(*Config)
	w := getSharedWriter(c)

	return xexporterhelper.NewProfiles(ctx, set, cfg, pushFuncErr(w, mapProfiles), helperOptions(c)...)
}
