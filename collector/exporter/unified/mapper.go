package unified

import (
	"encoding/hex"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/akshatagarwl/observe/pkg/schema"
	"go.opentelemetry.io/collector/pdata/pcommon"
	"go.opentelemetry.io/collector/pdata/plog"
	"go.opentelemetry.io/collector/pdata/pmetric"
	"go.opentelemetry.io/collector/pdata/pprofile"
	"go.opentelemetry.io/collector/pdata/ptrace"
)

var (
	ptrUnitSpanDuration = ptr(schema.UnitSpanDuration)
	ptrUnitCount        = ptr(schema.UnitProfileCount)
)

var spanStatusUpper = map[ptrace.StatusCode]string{
	ptrace.StatusCodeOk:    "OK",
	ptrace.StatusCodeError: "ERROR",
	ptrace.StatusCodeUnset: "UNSET",
}

func serviceName(res pcommon.Resource) string {
	if v, ok := res.Attributes().Get("service.name"); ok {
		svc := strings.TrimSpace(v.AsString())
		if svc != "" {
			return svc
		}
	}

	return "unknown_service"
}

func hostName(res pcommon.Resource) string {
	if v, ok := res.Attributes().Get("host.name"); ok {
		return v.AsString()
	}

	return ""
}

func podName(res pcommon.Resource) string {
	if v, ok := res.Attributes().Get("k8s.pod.name"); ok {
		return v.AsString()
	}

	return ""
}

func deployVersion(res pcommon.Resource) string {
	if v, ok := res.Attributes().Get("service.version"); ok {
		return v.AsString()
	}

	return ""
}

func ptr[T any](v T) *T { return &v }

func traceIDStr(tid pcommon.TraceID) *string {
	if tid.IsEmpty() {
		return nil
	}

	return ptr(hex.EncodeToString(tid[:]))
}

func spanIDStr(sid pcommon.SpanID) *string {
	if sid.IsEmpty() {
		return nil
	}

	return ptr(hex.EncodeToString(sid[:]))
}

func mapMetrics(md pmetric.Metrics) []schema.UnifiedRow {
	rows := make([]schema.UnifiedRow, 0, md.DataPointCount())

	for i := 0; i < md.ResourceMetrics().Len(); i++ {
		rm := md.ResourceMetrics().At(i)
		svc := serviceName(rm.Resource())
		host := hostName(rm.Resource())
		pod := podName(rm.Resource())
		deploy := deployVersion(rm.Resource())

		for j := 0; j < rm.ScopeMetrics().Len(); j++ {
			sm := rm.ScopeMetrics().At(j)
			for k := 0; k < sm.Metrics().Len(); k++ {
				m := sm.Metrics().At(k)
				unit := m.Unit()

				switch m.Type() {
				case pmetric.MetricTypeGauge:
					for p := 0; p < m.Gauge().DataPoints().Len(); p++ {
						dp := m.Gauge().DataPoints().At(p)
						rows = append(rows, schema.UnifiedRow{
							Timestamp: int64(dp.Timestamp()),
							Signal:    schema.SignalMetric,
							Service:   svc, Host: host, Pod: pod, DeployVer: deploy,
							Name: m.Name(), Value: ptr(dpValue(dp)), Unit: &unit,
						})
					}
				case pmetric.MetricTypeSum:
					for p := 0; p < m.Sum().DataPoints().Len(); p++ {
						dp := m.Sum().DataPoints().At(p)
						rows = append(rows, schema.UnifiedRow{
							Timestamp: int64(dp.Timestamp()),
							Signal:    schema.SignalMetric,
							Service:   svc, Host: host, Pod: pod, DeployVer: deploy,
							Name: m.Name(), Value: ptr(dpValue(dp)), Unit: &unit,
						})
					}
				// FRAGILE: ExponentialHistogram and Summary metric types are silently
				// dropped — no row is emitted and no error is raised. Data loss is
				// invisible to the operator.
				case pmetric.MetricTypeHistogram:
					for p := 0; p < m.Histogram().DataPoints().Len(); p++ {
						dp := m.Histogram().DataPoints().At(p)

						// FRAGILE: divide-by-zero when Count==0 (newly-reset cumulative
						// histograms). Also collapses a full distribution into a single
						// avg — loses p50/p95/p99 bucket information.
						var val float64
						if dp.HasSum() {
							val = dp.Sum() / float64(dp.Count())
						}

						rows = append(rows, schema.UnifiedRow{
							Timestamp: int64(dp.Timestamp()),
							Signal:    schema.SignalMetric,
							Service:   svc, Host: host, Pod: pod, DeployVer: deploy,
							Name: m.Name(), Value: &val, Unit: &unit,
						})
					}
				}
			}
		}
	}

	return rows
}

func dpValue(dp pmetric.NumberDataPoint) float64 {
	switch dp.ValueType() {
	case pmetric.NumberDataPointValueTypeInt:
		return float64(dp.IntValue())
	case pmetric.NumberDataPointValueTypeDouble:
		return dp.DoubleValue()
	}

	// FRAGILE: panics on unrecognized value types — newer OTel versions could
	// add value types, crashing the collector on upgrade.
	panic(fmt.Sprintf("unsupported metric datapoint value type: %v", dp.ValueType()))
}

func mustJSONString(attrs map[string]string) *string {
	b, err := json.Marshal(attrs)
	if err != nil {
		panic(fmt.Sprintf("marshal attributes: %v", err))
	}

	return ptr(string(b))
}

func mapTraces(td ptrace.Traces) []schema.UnifiedRow {
	rows := make([]schema.UnifiedRow, 0, td.SpanCount())

	for i := 0; i < td.ResourceSpans().Len(); i++ {
		rs := td.ResourceSpans().At(i)
		svc := serviceName(rs.Resource())
		host := hostName(rs.Resource())
		pod := podName(rs.Resource())
		deploy := deployVersion(rs.Resource())

		for j := 0; j < rs.ScopeSpans().Len(); j++ {
			ss := rs.ScopeSpans().At(j)
			for k := 0; k < ss.Spans().Len(); k++ {
				s := ss.Spans().At(k)
				// OTel timestamps are nanoseconds; convert to milliseconds at write time.
				// This is the canonical unit for span durations — see schema.UnitSpanDuration.
				// FRAGILE: unsigned subtraction on pcommon.Timestamp (uint64) — if
				// EndTimestamp < StartTimestamp (clock skew, unfinished spans where
				// EndTimestamp==0), this wraps to a huge positive number rather than
				// going negative, producing nonsensical duration values.
				dur := float64(s.EndTimestamp()-s.StartTimestamp()) / 1e6
				// Normalize to uppercase to match the query layer, which filters
				// on status='ERROR'. ptrace.StatusCode.String() returns title-case
				// (e.g. "Error"), so we upper-case here at write time.
				status := spanStatusUpper[s.Status().Code()]
				kind := s.Kind().String()

				var events []schema.Event

				for e := 0; e < s.Events().Len(); e++ {
					ev := s.Events().At(e)

					event := schema.Event{
						Timestamp: int64(ev.Timestamp()),
						Name:      ev.Name(),
					}
					if ev.Attributes().Len() > 0 {
						attrs := map[string]string{}

						ev.Attributes().Range(func(k string, v pcommon.Value) bool {
							attrs[k] = v.AsString()
							return true
						})
						event.Attributes = mustJSONString(attrs)
					}

					events = append(events, event)
				}

				var metadata *string

				if s.Attributes().Len() > 0 {
					attrs := map[string]string{}

					s.Attributes().Range(func(k string, v pcommon.Value) bool {
						attrs[k] = v.AsString()
						return true
					})

					metadata = mustJSONString(attrs)
				}

				rows = append(rows, schema.UnifiedRow{
					Timestamp: int64(s.StartTimestamp()),
					Signal:    schema.SignalSpan,
					Service:   svc, Host: host, Pod: pod, DeployVer: deploy,
					TraceID:      traceIDStr(s.TraceID()),
					SpanID:       spanIDStr(s.SpanID()),
					ParentSpanID: spanIDStr(s.ParentSpanID()),
					Name:         s.Name(),
					Value:        &dur,
					Unit:         ptrUnitSpanDuration,
					Status:       &status,
					Kind:         &kind,
					Events:       events,
					Metadata:     metadata,
				})
			}
		}
	}

	return rows
}

func mapLogs(ld plog.Logs) []schema.UnifiedRow {
	rows := make([]schema.UnifiedRow, 0, ld.LogRecordCount())

	for i := 0; i < ld.ResourceLogs().Len(); i++ {
		rl := ld.ResourceLogs().At(i)
		svc := serviceName(rl.Resource())
		host := hostName(rl.Resource())
		pod := podName(rl.Resource())
		deploy := deployVersion(rl.Resource())

		for j := 0; j < rl.ScopeLogs().Len(); j++ {
			sl := rl.ScopeLogs().At(j)

			scopeName := sl.Scope().Name()
			for k := 0; k < sl.LogRecords().Len(); k++ {
				lr := sl.LogRecords().At(k)
				status := lr.SeverityText()
				msg := lr.Body().AsString()

				if scopeName == "" {
					scopeName = "unknown"
				}

				rows = append(rows, schema.UnifiedRow{
					Timestamp: int64(lr.Timestamp()),
					Signal:    schema.SignalLog,
					Service:   svc, Host: host, Pod: pod, DeployVer: deploy,
					TraceID: traceIDStr(lr.TraceID()),
					Name:    scopeName,
					Status:  &status,
					Message: &msg,
				})
			}
		}
	}

	return rows
}

// FRAGILE: assumes Dictionary is populated at the top-level Profiles container
// and shared across all ResourceProfiles/ScopeProfiles. The pprofile API is
// experimental (v0.146.1) and this layout could change. If a future version
// moves dictionaries per-Profile, every index lookup below silently returns
// wrong data (wrong function names, corrupt stacks) with no error.
func mapProfiles(pd pprofile.Profiles) []schema.UnifiedRow {
	rows := make([]schema.UnifiedRow, 0, 256)

	// Dictionary is shared across all profiles in the batch
	dict := pd.Dictionary()
	stringTable := dict.StringTable()
	locationTable := dict.LocationTable()
	functionTable := dict.FunctionTable()
	stackTable := dict.StackTable()

	// Pre-compute resolved function names per location index.
	locationNames := make([][]string, locationTable.Len())
	for l := 0; l < locationTable.Len(); l++ {
		loc := locationTable.At(l)

		var names []string

		for li := 0; li < loc.Lines().Len(); li++ {
			funcIdx := loc.Lines().At(li).FunctionIndex()
			if int(funcIdx) < functionTable.Len() {
				fn := functionTable.At(int(funcIdx))

				nameIdx := fn.NameStrindex()
				if int(nameIdx) < stringTable.Len() {
					names = append(names, stringTable.At(int(nameIdx)))
				}
			}
		}

		locationNames[l] = names
	}

	for i := 0; i < pd.ResourceProfiles().Len(); i++ {
		rp := pd.ResourceProfiles().At(i)
		svc := serviceName(rp.Resource())
		host := hostName(rp.Resource())
		pod := podName(rp.Resource())
		deploy := deployVersion(rp.Resource())

		for j := 0; j < rp.ScopeProfiles().Len(); j++ {
			sp := rp.ScopeProfiles().At(j)
			for k := 0; k < sp.Profiles().Len(); k++ {
				p := sp.Profiles().At(k)

				typeIdx := p.SampleType().TypeStrindex()
				if int(typeIdx) >= stringTable.Len() || stringTable.At(int(typeIdx)) == "" {
					panic("profile sample type is required")
				}

				profileType := stringTable.At(int(typeIdx))

				ts := int64(p.Time())

				for s := 0; s < p.Samples().Len(); s++ {
					sample := p.Samples().At(s)

					// Resolve stacktrace from sample → stack table → location names.
					var stack []string

					if stackIdx := sample.StackIndex(); int(stackIdx) < stackTable.Len() {
						locIndices := stackTable.At(int(stackIdx)).LocationIndices()

						stack = make([]string, 0, locIndices.Len())
						for li := 0; li < locIndices.Len(); li++ {
							locIdx := locIndices.At(li)
							if int(locIdx) < len(locationNames) {
								stack = append(stack, locationNames[locIdx]...)
							}
						}
					}

					// FRAGILE: only reads Values().At(0) — silently drops all other
					// value columns (e.g. alloc_objects vs alloc_space in heap
					// profiles). Multi-value profiles lose data without warning.
					var val float64
					if sample.Values().Len() > 0 {
						val = float64(sample.Values().At(0))
					}

					rows = append(rows, schema.UnifiedRow{
						Timestamp: ts,
						Signal:    schema.SignalProfile,
						Service:   svc, Host: host, Pod: pod, DeployVer: deploy,
						Name:       fmt.Sprintf("profile.%s", profileType),
						Value:      &val,
						Stacktrace: stack,
						Unit:       ptrUnitCount,
						Kind:       &profileType,
					})
				}
			}
		}
	}

	return rows
}
