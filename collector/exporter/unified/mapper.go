package unified

import (
	"encoding/hex"
	"encoding/json"
	"fmt"

	"github.com/AkshatM/observe/pkg/schema"
	"go.opentelemetry.io/collector/pdata/pcommon"
	"go.opentelemetry.io/collector/pdata/plog"
	"go.opentelemetry.io/collector/pdata/pmetric"
	"go.opentelemetry.io/collector/pdata/pprofile"
	"go.opentelemetry.io/collector/pdata/ptrace"
)

func serviceName(res pcommon.Resource) string {
	if v, ok := res.Attributes().Get("service.name"); ok {
		return v.AsString()
	}
	return "unknown"
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
	var rows []schema.UnifiedRow
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
				case pmetric.MetricTypeHistogram:
					for p := 0; p < m.Histogram().DataPoints().Len(); p++ {
						dp := m.Histogram().DataPoints().At(p)
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
	return 0
}

func mapTraces(td ptrace.Traces) []schema.UnifiedRow {
	var rows []schema.UnifiedRow
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
				dur := float64(s.EndTimestamp() - s.StartTimestamp())
				status := s.Status().Code().String()
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
						b, _ := json.Marshal(attrs)
						event.Attributes = ptr(string(b))
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
					b, _ := json.Marshal(attrs)
					metadata = ptr(string(b))
				}

				rows = append(rows, schema.UnifiedRow{
					Timestamp:    int64(s.StartTimestamp()),
					Signal:       schema.SignalSpan,
					Service:      svc, Host: host, Pod: pod, DeployVer: deploy,
					TraceID:      traceIDStr(s.TraceID()),
					SpanID:       spanIDStr(s.SpanID()),
					ParentSpanID: spanIDStr(s.ParentSpanID()),
					Name:         s.Name(),
					Value:        &dur,
					Unit:         ptr("ns"),
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
	var rows []schema.UnifiedRow
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

				name := scopeName
				if name == "" {
					name = "log"
				}

				rows = append(rows, schema.UnifiedRow{
					Timestamp: int64(lr.Timestamp()),
					Signal:    schema.SignalLog,
					Service:   svc, Host: host, Pod: pod, DeployVer: deploy,
					TraceID: traceIDStr(lr.TraceID()),
					Name:    name,
					Status:  &status,
					Message: &msg,
				})
			}
		}
	}
	return rows
}

func mapProfiles(pd pprofile.Profiles) []schema.UnifiedRow {
	var rows []schema.UnifiedRow

	// Dictionary is shared across all profiles in the batch
	dict := pd.Dictionary()
	stringTable := dict.StringTable()
	locationTable := dict.LocationTable()
	functionTable := dict.FunctionTable()
	stackTable := dict.StackTable()

	// Build location → function names index
	type locInfo struct {
		names []string
	}
	locations := make([]locInfo, locationTable.Len())
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
		locations[l] = locInfo{names: names}
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

				// Determine profile type from SampleType
				profileType := "cpu"
				typeIdx := p.SampleType().TypeStrindex()
				if int(typeIdx) < stringTable.Len() {
					t := stringTable.At(int(typeIdx))
					if t != "" {
						profileType = t
					}
				}

				ts := int64(p.Time())

				for s := 0; s < p.Samples().Len(); s++ {
					sample := p.Samples().At(s)

					// Resolve stack via StackTable
					var stack []string
					stackIdx := sample.StackIndex()
					if int(stackIdx) < stackTable.Len() {
						st := stackTable.At(int(stackIdx))
						locIndices := st.LocationIndices()
						for li := 0; li < locIndices.Len(); li++ {
							locIdx := locIndices.At(li)
							if int(locIdx) < len(locations) {
								stack = append(stack, locations[locIdx].names...)
							}
						}
					}

					var val float64
					if sample.Values().Len() > 0 {
						val = float64(sample.Values().At(0))
					}

					rows = append(rows, schema.UnifiedRow{
						Timestamp:  ts,
						Signal:     schema.SignalProfile,
						Service:    svc, Host: host, Pod: pod, DeployVer: deploy,
						Name:       fmt.Sprintf("profile.%s", profileType),
						Value:      &val,
						Stacktrace: stack,
						Unit:       ptr("count"),
						Kind:       &profileType,
					})
				}
			}
		}
	}
	return rows
}
