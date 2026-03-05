package unified

import (
	"context"
	"testing"

	"go.opentelemetry.io/collector/exporter"
	"go.opentelemetry.io/collector/exporter/exportertest"
)

func TestSharedWriter_AllSignalsShareOneWriter(t *testing.T) {
	resetSingleton()
	t.Cleanup(resetSingleton)

	cfg := createDefaultConfig()
	ctx := context.Background()
	set := exportertest.NewNopSettings(exportertest.NopType)

	_, err := createMetricsExporter(ctx, set, cfg)
	if err != nil {
		t.Fatal(err)
	}

	w1 := sharedWriter

	_, err = createTracesExporter(ctx, set, cfg)
	if err != nil {
		t.Fatal(err)
	}

	w2 := sharedWriter

	_, err = createLogsExporter(ctx, set, cfg)
	if err != nil {
		t.Fatal(err)
	}

	w3 := sharedWriter

	_, err = createProfilesExporter(ctx, set, cfg)
	if err != nil {
		t.Fatal(err)
	}

	w4 := sharedWriter

	if w1 != w2 || w2 != w3 || w3 != w4 {
		t.Fatal("expected all signal types to share the same ParquetWriter")
	}
}

func TestSharedWriter_ResetAllowsNewInstance(t *testing.T) {
	resetSingleton()
	t.Cleanup(resetSingleton)

	cfg := createDefaultConfig()
	ctx := context.Background()
	set := exportertest.NewNopSettings(exportertest.NopType)

	_, _ = createMetricsExporter(ctx, set, cfg)
	first := sharedWriter

	resetSingleton()

	_, _ = createMetricsExporter(ctx, set, cfg)
	second := sharedWriter

	if first == second {
		t.Fatal("resetSingleton did not create a new ParquetWriter instance")
	}
}

func TestFactory_AllSignalTypesCreateValidExporters(t *testing.T) {
	resetSingleton()
	t.Cleanup(resetSingleton)

	cfg := createDefaultConfig()
	ctx := context.Background()
	set := exportertest.NewNopSettings(exportertest.NopType)

	me, err := createMetricsExporter(ctx, set, cfg)
	if err != nil {
		t.Fatalf("metrics: %v", err)
	}

	if me == nil {
		t.Fatal("metrics exporter is nil")
	}

	te, err := createTracesExporter(ctx, set, cfg)
	if err != nil {
		t.Fatalf("traces: %v", err)
	}

	if te == nil {
		t.Fatal("traces exporter is nil")
	}

	le, err := createLogsExporter(ctx, set, cfg)
	if err != nil {
		t.Fatalf("logs: %v", err)
	}

	if le == nil {
		t.Fatal("logs exporter is nil")
	}

	pe, err := createProfilesExporter(ctx, set, cfg)
	if err != nil {
		t.Fatalf("profiles: %v", err)
	}

	if pe == nil {
		t.Fatal("profiles exporter is nil")
	}

	// Verify exporterhelper-managed exporters satisfy the exporter interface.
	_ = me.(exporter.Metrics)
	_ = te.(exporter.Traces)
	_ = le.(exporter.Logs)
}
