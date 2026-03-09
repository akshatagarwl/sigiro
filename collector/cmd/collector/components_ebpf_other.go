//go:build !linux

package main

import (
	"go.opentelemetry.io/collector/component"
	"go.opentelemetry.io/collector/receiver"
)

func platformReceivers() []receiver.Factory {
	return nil
}

func platformReceiverModules() map[component.Type]string {
	return nil
}
