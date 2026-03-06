package pyroscope

import (
	"fmt"
	"time"

	"go.opentelemetry.io/collector/config/confighttp"
)

type Config struct {
	HTTP    confighttp.ServerConfig `mapstructure:"http"`
	Timeout time.Duration           `mapstructure:"timeout"`
}

func (cfg *Config) Validate() error {
	if cfg.Timeout <= 0 {
		return fmt.Errorf("timeout must be positive")
	}

	return nil
}
