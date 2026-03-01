package unified

import (
	"context"
	"sync"
	"time"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/credentials"
	"github.com/aws/aws-sdk-go-v2/service/s3"
	"go.opentelemetry.io/collector/component"
	"go.opentelemetry.io/collector/consumer"
	"go.opentelemetry.io/collector/exporter"
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
		S3Bucket:      "observe",
		S3Region:      "us-east-1",
		FlushInterval: 15 * time.Second,
	}
}

// singleton holds the single shared Exporter instance. All four signal
// types feed into the same buffer, flush loop, and S3 connection.
var (
	singletonMu sync.Mutex
	singleton   *Exporter
)

func sharedExporter(cfg component.Config) *Exporter {
	singletonMu.Lock()
	defer singletonMu.Unlock()

	if singleton == nil {
		c := cfg.(*Config)
		s3Client := newS3Client(c)
		writer := NewParquetWriter(c.S3Bucket, s3Client)
		singleton = NewExporter(*c, writer)
	}

	return singleton
}

// ResetSingleton tears down the shared exporter (for tests).
func ResetSingleton() {
	singletonMu.Lock()
	defer singletonMu.Unlock()

	singleton = nil
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

func createMetricsExporter(_ context.Context, _ exporter.Settings, cfg component.Config) (exporter.Metrics, error) {
	return &metricsExporter{sharedExporter(cfg)}, nil
}

func createTracesExporter(_ context.Context, _ exporter.Settings, cfg component.Config) (exporter.Traces, error) {
	return &tracesExporter{sharedExporter(cfg)}, nil
}

func createLogsExporter(_ context.Context, _ exporter.Settings, cfg component.Config) (exporter.Logs, error) {
	return &logsExporter{sharedExporter(cfg)}, nil
}

func createProfilesExporter(_ context.Context, _ exporter.Settings, cfg component.Config) (xexporter.Profiles, error) {
	return &profilesExporter{sharedExporter(cfg)}, nil
}

// Wrapper types that satisfy the exporter interfaces.
// Each embeds the shared Exporter and adds the required Capabilities method.

type metricsExporter struct{ *Exporter }

func (e *metricsExporter) Capabilities() consumer.Capabilities {
	return consumer.Capabilities{}
}

func (e *metricsExporter) ConsumeMetrics(ctx context.Context, md pmetric.Metrics) error {
	return e.Exporter.ConsumeMetrics(ctx, md)
}

type tracesExporter struct{ *Exporter }

func (e *tracesExporter) Capabilities() consumer.Capabilities {
	return consumer.Capabilities{}
}

func (e *tracesExporter) ConsumeTraces(ctx context.Context, td ptrace.Traces) error {
	return e.Exporter.ConsumeTraces(ctx, td)
}

type logsExporter struct{ *Exporter }

func (e *logsExporter) Capabilities() consumer.Capabilities {
	return consumer.Capabilities{}
}

func (e *logsExporter) ConsumeLogs(ctx context.Context, ld plog.Logs) error {
	return e.Exporter.ConsumeLogs(ctx, ld)
}

type profilesExporter struct{ *Exporter }

func (e *profilesExporter) Capabilities() consumer.Capabilities {
	return consumer.Capabilities{}
}

func (e *profilesExporter) ConsumeProfiles(ctx context.Context, pd pprofile.Profiles) error {
	return e.Exporter.ConsumeProfiles(ctx, pd)
}
