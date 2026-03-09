//go:build linux

package main

import (
	"go.opentelemetry.io/collector/component"
	"go.opentelemetry.io/collector/receiver"
	collector "go.opentelemetry.io/ebpf-profiler/collector"
)

func platformReceivers() []receiver.Factory {
	return []receiver.Factory{collector.NewFactory()}
}

func platformReceiverModules() map[component.Type]string {
	return map[component.Type]string{
		collector.NewFactory().Type(): "go.opentelemetry.io/ebpf-profiler v0.0.202610",
	}
}
