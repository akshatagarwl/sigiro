package pyroscope

import (
	"context"
	"time"

	"go.opentelemetry.io/collector/component"
	"go.opentelemetry.io/collector/config/confighttp"
	"go.opentelemetry.io/collector/config/confignet"
	"go.opentelemetry.io/collector/consumer/xconsumer"
	"go.opentelemetry.io/collector/receiver"
	"go.opentelemetry.io/collector/receiver/xreceiver"
)

const typeStr = "pyroscope"

func NewFactory() receiver.Factory {
	return xreceiver.NewFactory(
		component.MustNewType(typeStr),
		createDefaultConfig,
		xreceiver.WithProfiles(createProfilesReceiver, component.StabilityLevelAlpha),
	)
}

func createDefaultConfig() component.Config {
	return &Config{
		HTTP: confighttp.ServerConfig{
			NetAddr: confignet.AddrConfig{
				Endpoint:  "0.0.0.0:4040",
				Transport: confignet.TransportTypeTCP,
			},
		},
		Timeout: 10 * time.Second,
	}
}

func createProfilesReceiver(
	_ context.Context,
	set receiver.Settings,
	cfg component.Config,
	consumer xconsumer.Profiles,
) (xreceiver.Profiles, error) {
	c := cfg.(*Config)
	return newPyroscopeReceiver(c, consumer, set)
}
