package unified

import (
	"go.opentelemetry.io/collector/config/configoptional"
	"go.opentelemetry.io/collector/config/configretry"
	"go.opentelemetry.io/collector/exporter/exporterhelper"
)

// Config defines the configuration for the unified Parquet exporter.
type Config struct {
	S3Bucket    string `mapstructure:"s3_bucket"`
	S3Region    string `mapstructure:"s3_region"`
	S3Endpoint  string `mapstructure:"s3_endpoint"`
	S3AccessKey string `mapstructure:"s3_access_key"`
	S3SecretKey string `mapstructure:"s3_secret_key"`

	Retry   configretry.BackOffConfig                                `mapstructure:"retry_on_failure"`
	Timeout exporterhelper.TimeoutConfig                             `mapstructure:"timeout"`
	Queue   configoptional.Optional[exporterhelper.QueueBatchConfig] `mapstructure:"sending_queue"`
}
