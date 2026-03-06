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

var ptrUnitSpanDuration = ptr(schema.UnitSpanDuration)

var spanStatusUpper = map[ptrace.StatusCode]string{
	ptrace.StatusCodeOk:    "OK",
	ptrace.StatusCodeError: "ERROR",
	ptrace.StatusCodeUnset: "UNSET",
}

// resAttr returns a resource attribute value, or "" if missing.
func resAttr(res pcommon.Resource, key string) string {
	if v, ok := res.Attributes().Get(key); ok {
		return v.AsString()
	}

	return ""
}

// resolveServiceName returns the best service name from resource attributes.
// If service.name is missing or is an OTel sentinel (unknown_service, unknown_service:go, etc.),
// falls back through k8s workload names, container name, and process executable name.
func resolveServiceName(res pcommon.Resource) string {
	if svc := strings.TrimSpace(resAttr(res, "service.name")); svc != "" && !strings.HasPrefix(svc, "unknown_service") {
		return svc
	}

	for _, key := range []string{
		"k8s.deployment.name",
		"k8s.statefulset.name",
		"k8s.daemonset.name",
		"k8s.container.name",
		"process.executable.name",
	} {
		if v := strings.TrimSpace(resAttr(res, key)); v != "" {
			return v
		}
	}

	return "unknown_service"
}

// resourceInfo holds the promoted resource attributes extracted into columns.
type resourceInfo struct {
	svc, host, pod, deploy string
	extra                  map[string]string
}

func extractResource(res pcommon.Resource) resourceInfo {
	return resourceInfo{
		svc:    resolveServiceName(res),
		host:   resAttr(res, "host.name"),
		pod:    resAttr(res, "k8s.pod.name"),
		deploy: resAttr(res, "service.version"),
		extra:  extraResourceAttrs(res),
	}
}

// promotedResourceKeys are the resource attributes extracted into their own columns.
// Everything else goes into metadata.
var promotedResourceKeys = map[string]bool{
	"service.name":    true,
	"host.name":       true,
	"k8s.pod.name":    true,
	"service.version": true,
}

// extraResourceAttrs returns resource attributes that aren't promoted to columns.
func extraResourceAttrs(res pcommon.Resource) map[string]string {
	var attrs map[string]string

	res.Attributes().Range(func(k string, v pcommon.Value) bool {
		if promotedResourceKeys[k] {
			return true
		}

		if attrs == nil {
			attrs = make(map[string]string)
		}

		attrs[k] = v.AsString()

		return true
	})

	return attrs
}

// attrsToMap converts pcommon attributes to a string map.
func attrsToMap(pattrs pcommon.Map) map[string]string {
	if pattrs.Len() == 0 {
		return nil
	}

	attrs := make(map[string]string, pattrs.Len())

	pattrs.Range(func(k string, v pcommon.Value) bool {
		attrs[k] = v.AsString()
		return true
	})

	return attrs
}

// mergeMetadata merges resource attrs (baseline) with signal-specific attrs
// (override on conflict) and returns the JSON string. Returns nil if both are empty.
func mergeMetadata(resourceAttrs, signalAttrs map[string]string) *string {
	if len(resourceAttrs) == 0 {
		if len(signalAttrs) == 0 {
			return nil
		}

		return mustJSONString(signalAttrs)
	}

	if len(signalAttrs) == 0 {
		return mustJSONString(resourceAttrs)
	}

	merged := make(map[string]string, len(resourceAttrs)+len(signalAttrs))

	for k, v := range resourceAttrs {
		merged[k] = v
	}

	// Signal-specific attrs override resource attrs.
	for k, v := range signalAttrs {
		merged[k] = v
	}

	return mustJSONString(merged)
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
		r := extractResource(rm.Resource())
		metadata := mergeMetadata(r.extra, nil)

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
							Service:   r.svc, Host: r.host, Pod: r.pod, DeployVer: r.deploy,
							Name: m.Name(), Value: ptr(dpValue(dp)), Unit: &unit,
							Metadata: metadata,
						})
					}
				case pmetric.MetricTypeSum:
					for p := 0; p < m.Sum().DataPoints().Len(); p++ {
						dp := m.Sum().DataPoints().At(p)
						rows = append(rows, schema.UnifiedRow{
							Timestamp: int64(dp.Timestamp()),
							Signal:    schema.SignalMetric,
							Service:   r.svc, Host: r.host, Pod: r.pod, DeployVer: r.deploy,
							Name: m.Name(), Value: ptr(dpValue(dp)), Unit: &unit,
							Metadata: metadata,
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
							Service:   r.svc, Host: r.host, Pod: r.pod, DeployVer: r.deploy,
							Name: m.Name(), Value: &val, Unit: &unit,
							Metadata: metadata,
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
		r := extractResource(rs.Resource())

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
						attrs := attrsToMap(ev.Attributes())
						event.Attributes = mustJSONString(attrs)
					}

					events = append(events, event)
				}

				spanAttrs := attrsToMap(s.Attributes())
				metadata := mergeMetadata(r.extra, spanAttrs)

				rows = append(rows, schema.UnifiedRow{
					Timestamp: int64(s.StartTimestamp()),
					Signal:    schema.SignalSpan,
					Service:   r.svc, Host: r.host, Pod: r.pod, DeployVer: r.deploy,
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
		r := extractResource(rl.Resource())

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

				logAttrs := attrsToMap(lr.Attributes())
				metadata := mergeMetadata(r.extra, logAttrs)

				rows = append(rows, schema.UnifiedRow{
					Timestamp: int64(lr.Timestamp()),
					Signal:    schema.SignalLog,
					Service:   r.svc, Host: r.host, Pod: r.pod, DeployVer: r.deploy,
					TraceID:  traceIDStr(lr.TraceID()),
					Name:     scopeName,
					Status:   &status,
					Message:  &msg,
					Metadata: metadata,
				})
			}
		}
	}

	return rows
}

// GAP: Dictionary location assumption.
//
// Assumes Dictionary is populated at the top-level Profiles container and
// shared across all ResourceProfiles/ScopeProfiles. The pprofile API is
// experimental (v0.147.0) and this layout could change. If a future version
// moves dictionaries per-Profile, every index lookup below silently returns
// wrong data (wrong function names, corrupt stacks) with no error.
//
// See docs/architecture/profiles.md §Known Gaps.
//
// Unlike mapMetrics/mapTraces/mapLogs which panic on malformed data (programmer
// errors like unknown enum values), mapProfiles returns errors because the
// failure modes are data-shape problems from external profilers (multi-value
// samples, missing values) that can happen at runtime with valid code.
func mapProfiles(pd pprofile.Profiles) ([]schema.UnifiedRow, error) {
	rows := make([]schema.UnifiedRow, 0, 256)

	// Dictionary is shared across all profiles in the batch.
	dict := pd.Dictionary()
	stringTable := dict.StringTable()
	locationTable := dict.LocationTable()
	functionTable := dict.FunctionTable()
	stackTable := dict.StackTable()
	attrTable := dict.AttributeTable()

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
		r := extractResource(rp.Resource())

		for j := 0; j < rp.ScopeProfiles().Len(); j++ {
			sp := rp.ScopeProfiles().At(j)
			for k := 0; k < sp.Profiles().Len(); k++ {
				p := sp.Profiles().At(k)

				typeIdx := p.SampleType().TypeStrindex()
				if int(typeIdx) >= stringTable.Len() || stringTable.At(int(typeIdx)) == "" {
					return nil, fmt.Errorf("profile sample type is required (type_strindex=%d, string_table_len=%d)", typeIdx, stringTable.Len())
				}

				profileType := stringTable.At(int(typeIdx))

				unitIdx := p.SampleType().UnitStrindex()

				unit := schema.UnitProfileCount
				if int(unitIdx) < stringTable.Len() && stringTable.At(int(unitIdx)) != "" {
					unit = stringTable.At(int(unitIdx))
				}

				ts := int64(p.Time())
				name := "profile." + profileType

				for s := 0; s < p.Samples().Len(); s++ {
					sample := p.Samples().At(s)

					// GAP: Multi-value profiles.
					//
					// OTel pprofile SampleType is singular per Profile, but Sample.Values()
					// can contain multiple entries. In the Google pprof format, a heap profile
					// carries 4 value types (alloc_objects, alloc_space, inuse_objects,
					// inuse_space) as parallel arrays. The OTel pprofile format expects
					// separate Profile objects per value type, so Values().Len() > 1 is
					// non-conformant or indicates a format we don't understand.
					//
					// We refuse to silently drop values. If a profiler sends multi-value
					// samples, this error surfaces immediately so we can implement proper
					// fan-out (one row per value type) with correct naming.
					//
					// See docs/architecture/profiles.md §Gap 1.
					if sample.Values().Len() > 1 {
						return nil, fmt.Errorf("multi-value profile samples not supported: got %d values, want 1 (service=%s, profile_type=%s, sample_index=%d)", sample.Values().Len(), r.svc, profileType, s)
					}

					// OTel eBPF profiler emits CPU/probe samples with empty
					// Values() by design. Each sample represents one stack
					// observation at the configured samples_per_second rate.
					// Default to count=1: each empty-value sample is one CPU hit.
					//
					// See: opentelemetry-ebpf-profiler reporter/internal/pdata
					// TestGenerate_NativeFrame asserts Values().Len() == 0.
					var val float64
					if sample.Values().Len() == 0 {
						val = 1.0
					} else {
						val = float64(sample.Values().At(0))
					}

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

					// Read sample attributes from the attribute table.
					// eBPF profiler puts service.name, process.executable.name, process.pid here.
					sampleSvc := r.svc

					var sampleAttrs map[string]string

					attrIndices := sample.AttributeIndices()
					for ai := 0; ai < attrIndices.Len(); ai++ {
						idx := attrIndices.At(ai)
						if int(idx) >= attrTable.Len() {
							continue
						}

						kv := attrTable.At(int(idx))

						key := ""
						if ki := kv.KeyStrindex(); int(ki) < stringTable.Len() {
							key = stringTable.At(int(ki))
						}

						v := kv.Value().AsString()
						if key == "service.name" && v != "" && !strings.HasPrefix(v, "unknown_service") {
							sampleSvc = v
							continue
						}

						if key != "" {
							if sampleAttrs == nil {
								sampleAttrs = make(map[string]string)
							}

							sampleAttrs[key] = v
						}
					}

					sampleMetadata := mergeMetadata(r.extra, sampleAttrs)

					rows = append(rows, schema.UnifiedRow{
						Timestamp: ts,
						Signal:    schema.SignalProfile,
						Service:   sampleSvc, Host: r.host, Pod: r.pod, DeployVer: r.deploy,
						Name:       name,
						Value:      &val,
						Stacktrace: stack,
						Unit:       ptr(unit),
						Kind:       ptr(profileType),
						Metadata:   sampleMetadata,
					})
				}
			}
		}
	}

	return rows, nil
}
