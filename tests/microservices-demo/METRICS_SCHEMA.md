# Sigiro Microservices Demo Test — North Star Metrics

> Every test run must record these metrics. Stored in `runs/YYYY-MM-DD-<demo>/metrics.json`.

## 1. Instrumentation Metrics (per service)

| Field | Description |
|-------|-------------|
| `demo_name` | e.g. "otel-demo", "robot-shop" |
| `service_name` | Individual microservice |
| `language` | Go, Python, Java, Node.js, Rust, Ruby, PHP, .NET, C++ |
| `instrumentation_method` | `auto` (zero-code agent), `sdk` (manual code), `already_instrumented` |
| `steps_required` | Number of discrete steps |
| `time_minutes` | Wall-clock time to first trace in Sigiro |
| `code_changes` | Lines added/modified (0 for auto) |
| `config_changes` | Env vars, YAML, etc. |
| `deployment_changes` | Dockerfile/compose changes |
| `failures` | Failed attempts before success |
| `failure_reasons` | Array of why each failed |

## 2. Signal Coverage (per service)

| Field | Description |
|-------|-------------|
| `traces_received` | Bool |
| `logs_received` | Bool |
| `metrics_received` | Bool |
| `profiles_received` | Bool |
| `trace_count` | Total spans ingested |
| `log_count` | Total log records |
| `metric_datapoint_count` | Total metric points |
| `profile_sample_count` | Total profile samples |
| `missing_signals` | What was expected but missing |

## 3. Data Quality

| Field | Description |
|-------|-------------|
| `null_timestamps` | Count |
| `null_durations` | Count |
| `orphan_spans` | Count |
| `corrupted_ids` | Count |
| `duration_consistency_ok` | Bool |
| `service_name_coverage_pct` | 0-100 |
| `critical_path_accuracy` | Bool |

## 4. Investigation Effectiveness (per scenario)

| Field | Description |
|-------|-------------|
| `scenario` | What was tested |
| `investigate_sufficient` | Could investigate answer alone? |
| `required_direct_query` | Needed raw trace/log/metric query? |
| `query_type_needed` | trace_detail, raw_logs, raw_metrics, raw_spans, none |
| `correct_diagnosis` | Root cause correctly identified? |
| `false_positives` | Count |
| `false_negatives` | Count |
| `anomaly_detected` | Bool |
| `timeline_helpful` | Bool |
| `operation_summaries_helpful` | Bool |
| `log_patterns_helpful` | Bool |
| `metrics_helpful` | Bool |

## 5. Profiling (per language)

| Field | Description |
|-------|-------------|
| `profile_type` | cpu, memory, goroutines, etc. |
| `language` | |
| `instrumentation_method` | auto (eBPF), manual (Pyroscope SDK), pprof endpoint |
| `steps_required` | |
| `time_minutes` | |
| `profiles_in_investigate` | Bool |
| `profile_quality` | good, partial, missing |

## 6. Per-Run Summary

```json
{
  "demo_name": "otel-demo",
  "total_services": 10,
  "services_instrumented": 10,
  "languages_covered": ["Go", "Python", "Java", "Node.js"],
  "auto_instrumented_count": 6,
  "manual_instrumented_count": 4,
  "avg_instrumentation_time_min": 3.5,
  "total_instrumentation_failures": 2,
  "investigate_pass_rate": 0.75,
  "data_quality_score": 92,
  "timestamp": "2026-05-07T10:00:00Z",
  "sigiro_version": "0.4.0",
  "tester_notes": "..."
}
```

## Directory Structure

```
tests/microservices-demo/
  METRICS_SCHEMA.md          # This file
  RESULTS.md                 # Aggregated results across all runs
  runs/
    2026-05-07-otel-demo/
      config/                # docker-compose, collector config
      artifacts/             # Raw query responses, logs
      metrics.json           # Structured metrics
      NOTES.md               # Human observations
```

## Key Questions Every Test Must Answer

1. How long from clone to first trace in Sigiro?
2. Which services worked out-of-the-box (zero code changes)?
3. Which needed manual intervention — what kind?
4. Did investigate surface root cause without direct queries?
5. What data was missing or wrong?
6. Would an AI agent debug an incident using only investigate?
7. What would make instrumentation easier?
8. What would make investigation more effective?

## Targets

- >80% services instrumented with zero code changes
- >90% incidents resolvable via investigate alone
- <5 min avg instrumentation time per service
- >90% data quality score
- >80% investigate pass rate

