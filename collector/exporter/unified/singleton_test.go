package unified

import (
	"context"
	"testing"

	"go.opentelemetry.io/collector/component"
	"go.opentelemetry.io/collector/component/componenttest"
	"go.opentelemetry.io/collector/exporter"
	"go.opentelemetry.io/collector/exporter/xexporter"
	"go.opentelemetry.io/collector/pdata/plog"
	"go.opentelemetry.io/collector/pdata/pmetric"
	"go.opentelemetry.io/collector/pdata/ptrace"
)

func TestSingleton_AllSignalsShareOneExporter(t *testing.T) {
	ResetSingleton()
	t.Cleanup(ResetSingleton)

	cfg := createDefaultConfig()
	ctx := context.Background()
	set := exporter.Settings{}

	me, err := createMetricsExporter(ctx, set, cfg)
	if err != nil {
		t.Fatal(err)
	}

	te, err := createTracesExporter(ctx, set, cfg)
	if err != nil {
		t.Fatal(err)
	}

	le, err := createLogsExporter(ctx, set, cfg)
	if err != nil {
		t.Fatal(err)
	}

	pe, err := createProfilesExporter(ctx, set, cfg)
	if err != nil {
		t.Fatal(err)
	}

	// All wrappers must embed the same *Exporter.
	shared := me.(*metricsExporter).Exporter
	if te.(*tracesExporter).Exporter != shared {
		t.Fatal("traces exporter has a different *Exporter than metrics")
	}

	if le.(*logsExporter).Exporter != shared {
		t.Fatal("logs exporter has a different *Exporter than metrics")
	}

	if pe.(*profilesExporter).Exporter != shared {
		t.Fatal("profiles exporter has a different *Exporter than metrics")
	}
}

func TestSingleton_MixedSignalsShareBuffer(t *testing.T) {
	ResetSingleton()
	t.Cleanup(ResetSingleton)

	cfg := createDefaultConfig()
	ctx := context.Background()
	set := exporter.Settings{}

	me, _ := createMetricsExporter(ctx, set, cfg)
	te, _ := createTracesExporter(ctx, set, cfg)
	le, _ := createLogsExporter(ctx, set, cfg)

	// Consume one metric.
	md := pmetric.NewMetrics()
	rm := md.ResourceMetrics().AppendEmpty()
	rm.Resource().Attributes().PutStr("service.name", "svc")
	sm := rm.ScopeMetrics().AppendEmpty()
	sm.Scope().SetName("test")
	g := sm.Metrics().AppendEmpty()
	g.SetName("m1")
	g.SetEmptyGauge().DataPoints().AppendEmpty().SetDoubleValue(1)

	_ = me.(exporter.Metrics).ConsumeMetrics(ctx, md)

	// Consume one trace.
	td := ptrace.NewTraces()
	rs := td.ResourceSpans().AppendEmpty()
	rs.Resource().Attributes().PutStr("service.name", "svc")
	ss := rs.ScopeSpans().AppendEmpty()
	ss.Scope().SetName("test")
	sp := ss.Spans().AppendEmpty()
	sp.SetName("op")
	sp.SetTraceID([16]byte{1})
	sp.SetSpanID([8]byte{1})

	_ = te.(exporter.Traces).ConsumeTraces(ctx, td)

	// Consume one log.
	ld := plog.NewLogs()
	rl := ld.ResourceLogs().AppendEmpty()
	rl.Resource().Attributes().PutStr("service.name", "svc")
	sl := rl.ScopeLogs().AppendEmpty()
	sl.Scope().SetName("test")
	sl.LogRecords().AppendEmpty().Body().SetStr("hello")

	_ = le.(exporter.Logs).ConsumeLogs(ctx, ld)

	// All rows must be in the shared buffer.
	shared := me.(*metricsExporter).Exporter
	shared.mu.Lock()
	n := len(shared.buffer)
	shared.mu.Unlock()

	if n != 3 {
		t.Fatalf("expected 3 rows in shared buffer, got %d", n)
	}
}

func TestSingleton_StartShutdownIdempotent(t *testing.T) {
	ResetSingleton()
	t.Cleanup(ResetSingleton)

	cfg := createDefaultConfig()
	ctx := context.Background()
	set := exporter.Settings{}

	me, _ := createMetricsExporter(ctx, set, cfg)
	te, _ := createTracesExporter(ctx, set, cfg)
	le, _ := createLogsExporter(ctx, set, cfg)
	pe, _ := createProfilesExporter(ctx, set, cfg)

	host := componenttest.NewNopHost()

	// Start called 4 times — must not panic or start multiple goroutines.
	for _, s := range []component.Component{
		me.(component.Component),
		te.(component.Component),
		le.(component.Component),
		pe.(component.Component),
	} {
		if err := s.Start(ctx, host); err != nil {
			t.Fatalf("Start: %v", err)
		}
	}

	// Shutdown called 4 times — must not panic (double close).
	for _, s := range []component.Component{
		me.(component.Component),
		te.(component.Component),
		le.(component.Component),
		pe.(component.Component),
	} {
		if err := s.Shutdown(ctx); err != nil {
			t.Fatalf("Shutdown: %v", err)
		}
	}
}

func TestSingleton_ResetAllowsNewInstance(t *testing.T) {
	ResetSingleton()
	t.Cleanup(ResetSingleton)

	cfg := createDefaultConfig()
	ctx := context.Background()
	set := exporter.Settings{}

	me1, _ := createMetricsExporter(ctx, set, cfg)
	first := me1.(*metricsExporter).Exporter

	ResetSingleton()

	me2, _ := createMetricsExporter(ctx, set, cfg)
	second := me2.(*metricsExporter).Exporter

	if first == second {
		t.Fatal("ResetSingleton did not create a new instance")
	}
}

// Verify wrapper types satisfy both the signal interface and component.Component
// (which provides Start/Shutdown) so the collector can manage lifecycle.
func TestSingleton_WrappersSatisfyInterfaces(t *testing.T) {
	ResetSingleton()
	t.Cleanup(ResetSingleton)

	cfg := createDefaultConfig()
	ctx := context.Background()
	set := exporter.Settings{}

	me, _ := createMetricsExporter(ctx, set, cfg)
	te, _ := createTracesExporter(ctx, set, cfg)
	le, _ := createLogsExporter(ctx, set, cfg)
	pe, _ := createProfilesExporter(ctx, set, cfg)

	// Compile-time interface checks via type assertions.
	_ = me.(exporter.Metrics)
	_ = me.(component.Component)

	_ = te.(exporter.Traces)
	_ = te.(component.Component)

	_ = le.(exporter.Logs)
	_ = le.(component.Component)

	_ = pe.(xexporter.Profiles)
	_ = pe.(component.Component)
}
