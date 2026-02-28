package unified

import "time"

// Config defines the configuration for the unified Parquet exporter.
type Config struct {
	S3Bucket      string        `mapstructure:"s3_bucket"`
	S3Region      string        `mapstructure:"s3_region"`
	S3Endpoint    string        `mapstructure:"s3_endpoint"`
	S3AccessKey   string        `mapstructure:"s3_access_key"`
	S3SecretKey   string        `mapstructure:"s3_secret_key"`
	FlushInterval time.Duration `mapstructure:"flush_interval"`
}
