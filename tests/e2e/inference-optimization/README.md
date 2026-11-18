# Inference experiment E2E

Two entry points, both driving `scripts/inference_experiment.py`:

- `just check-inference-optimization-e2e` — real AIPerf 0.12.0 and the real sigiro
  binary against a local mock OpenAI server. Exercises the whole contract with no
  GPU. **Not** the target-hardware proof.
- `just check-inference-optimization-gpu <manifest>` — the same contract against a
  real GPU. Start from `gpu-manifest.example.json`.

## Exit codes from the local E2E

| Code | Meaning |
|---|---|
| 0 | contract and statistical verdict both passed |
| 2 | **contract passed, verdict skipped** — the host was too noisy to judge |
| other | a real failure |

Code 2 exists because the A/A calibration compares control periods against *each
other*. On a laptop whose load wanders during the ~8 minute run, the control's
goodput moves more than the confidence intervals allow and `drift_detected` is the
correct refusal, not a regression. Measured on this hardware: at load ~3 the
control spread is **0.5%** and the run passes; at load ~18 it reached **38%** and
the guard tripped. A spread above roughly 10% will keep tripping it.

Everything deterministic is asserted before that point and is a hard gate on any
host: six periods in order, evidence isolated per `optimization.run.id`, all four
required GenAI metrics present, observed timestamps inside their declared windows,
per-run evidence SQL, ledger discipline, one `sigiro_experiment_runs` row per
period chained by `parent_run_id` (and a re-record of one refused), the verdict
written onto every one of those rows, both documented ledger queries answered by
a real `/v1/query`, a sidecar started and stopped in every period, and the
candidate/control *ranking* (robust to noise even when the intervals are not).

## Two settings that are not arbitrary

- **`--request-count 200`.** At 40 requests and 15 ms TTFT, one profile run is
  ~300 ms of work, so AIPerf's own startup and host scheduling dominate the rate
  being measured. 200 makes it ~1.5 s and the noise collapses. This was the actual
  cause of the flakiness — not interval width, which is what it looked like.
- **`num_profile_runs: 3`.** `t_critical` is 4.303 at n=3 against 12.706 at n=2,
  where the interval is wide enough to swallow a real 3× difference.
- **`--wait-for-model-timeout 60`.** The mock server loads a 1.15M-token corpus
  before it binds: 0.74 s warm, but 6.1 s on a cold HuggingFace cache, and ~17 s
  of total startup — past the 15 s this used to allow, so the *first* run on a
  new machine failed A0-1 with a readiness-probe timeout. The probe returns the
  moment the server answers, so a larger budget costs a warm run nothing.

## The `roofline` block

Optional, and complete when present. The local E2E omits it: a mock server has no
memory floor to be near, so a headroom figure computed against one would be
fiction.

It is deliberately **accelerator-neutral**, which is why it takes a `resources`
map plus a `weight_residence` rather than one bandwidth number. A single
`peak_memory_bandwidth_bytes_per_sec` assumed two things that only hold for
HBM-resident GPUs: that weights live in device memory, and that device memory is
what binds. On SRAM-resident silicon the weights sit on-chip at orders of magnitude
more bandwidth and the **interconnect** binds instead, because the model must shard
across chips.

```
weight_residence ceiling = resources[weight_residence]
                           / (weight_bytes + kv_bytes_per_token * ctx)
KV-bound ceiling         = resources[weight_residence]
                           / (kv_bytes_per_token * ctx)
```

For Qwen3-4B on one RTX 3090 at ctx 2048 that is **112 tok/s** and **3100 tok/s**.
Measure 95 and you are at 85% of the limit — stop searching, no config knob finds
the missing 17. Measure 30 and something is stalling, so the search is worth funding.

**Only the residence ceiling is derivable from these inputs.** Taking the minimum
over every resource is wrong: it charges the whole weight read to every tier, and a
single-GPU decode step puts zero bytes on NVLink. An interconnect ceiling needs a
sharding descriptor this block does not carry yet — TP/EP degree, and bytes crossing
per step, which for MoE means the expert traffic matrix. The other resources are
recorded so the supply side is on file for when that arrives; until then they are
context, not ceilings.

It records only **inputs**, never a derived headroom ratio, because the ratio needs
three choices the runner cannot make — the engine's decode batch (client concurrency
is not the same thing), the context length, and tokens-versus-requests. Derive it
over the ledger, where those can vary without re-running the experiment.

The block is folded into the **hardware fingerprint**: two runs claiming different
peaks for the same box are making different claims about the machine and must not
compare as equal. `source` is mandatory because a stale figure silently corrupts
every number derived from it.

## The run ledger

Each period writes one `sigiro_experiment_runs` row the moment it finishes, via
`sigiro record-run --file <period>/run-record.json`. The row carries only what
the runner observed: `experiment_id`/`run_id`/`arm`, the measured window, the
four fingerprints (`config_sha`, `model_sha`, `hardware_sha`, `benchmark_sha`),
the model id and revision, `config_json` (the arm's argv and env — both are
inside `config_sha`, so recording one without the other would make two different
configurations look identical), and the goodput AIPerf measured for that period.
`parent_run_id` points at the preceding period, so `A0-1 → A0-2 → A1 → B1 → B2 →
A2` is a chain; the first period has none.

Everything else the row can hold is left **NULL unless `topology` declares it**.
A guessed `gpu_model` or `tp_size` is indistinguishable from a measured one to
every query that later reads the table, so the runner never invents one.

Rows are written per period rather than at the end so that a run which dies in
period four still leaves three finished periods in the ledger. The cost of that
is `outcome` and `drift_detected`, which cannot be known while a period is still
the only thing that has happened — see "Two queries over the ledger" below for
how they get filled in.

**The ledger is a separate catalog** at `<output_root>/ledger`, not the
observer's `<experiment>/sigiro-data`. Two reasons, both hard: DuckDB holds a
single-writer lock on a catalog file, so a `record-run` against the serving
catalog fails with `Conflicting lock is held`; and `sigiro-data` is created fresh
per experiment, so a ledger inside it could never carry lineage past the
experiment that wrote it — which is the only thing `parent_run_id` is for.

## Two queries over the ledger

The verdict lands on the rows only after the comparison exists, so the runner
calls `sigiro finalize-experiment --experiment-id <id> --outcome <verdict>` with
whatever `compare-experiment` returned. `outcome` and `drift_detected` are
properties of the whole comparison rather than of one period, so they are
written to **every** row of the experiment: "did it help" then needs one table
and no join, which is the entire point of these columns.

Both queries below are the request body of `POST /v1/query` — raw SQL, no JSON
envelope, exactly one read-only SELECT. There are no CTEs anywhere in them
because the AST firewall rejects `WITH`; a lineage walk is a self-join and an
arm-versus-arm diff is conditional aggregation. Both are asserted in `run.sh`
against the ledger the local E2E produces, so they are tested rather than
documented and rotting.

### 1. Lineage: each run beside its parent

```sql
SELECT child.run_id, child.arm, child.parent_run_id, parent.arm AS parent_arm,
       substr(child.config_sha, 1, 12) AS config_sha,
       substr(parent.config_sha, 1, 12) AS parent_config_sha,
       child.config_json <> parent.config_json AS config_changed,
       round(child.goodput - parent.goodput, 3) AS goodput_delta,
       child.outcome, child.drift_detected
FROM lake.sigiro_experiment_runs AS child
JOIN lake.sigiro_experiment_runs AS parent
  ON parent.experiment_id = child.experiment_id
 AND parent.run_id = child.parent_run_id
WHERE child.experiment_id = 'local-known-slower'
ORDER BY child.started_at
```

Answers "what changed between each run and the one it came from, and did it
help" — the lineage edge, the configuration difference, and the verdict, from
one table.

```json
[
  {"run_id": "A0-2", "arm": "control",   "parent_run_id": "A0-1", "parent_arm": "control",
   "config_sha": "cecf16db8b0b", "parent_config_sha": "cecf16db8b0b",
   "config_changed": false, "goodput_delta": -2.109,  "outcome": "rejected", "drift_detected": false},
  {"run_id": "A1",   "arm": "control",   "parent_run_id": "A0-2", "parent_arm": "control",
   "config_sha": "cecf16db8b0b", "parent_config_sha": "cecf16db8b0b",
   "config_changed": false, "goodput_delta": -1.119,  "outcome": "rejected", "drift_detected": false},
  {"run_id": "B1",   "arm": "candidate", "parent_run_id": "A1",   "parent_arm": "control",
   "config_sha": "fcdd205df41f", "parent_config_sha": "cecf16db8b0b",
   "config_changed": true,  "goodput_delta": -43.999, "outcome": "rejected", "drift_detected": false},
  {"run_id": "B2",   "arm": "candidate", "parent_run_id": "B1",   "parent_arm": "candidate",
   "config_sha": "fcdd205df41f", "parent_config_sha": "fcdd205df41f",
   "config_changed": false, "goodput_delta": -0.012,  "outcome": "rejected", "drift_detected": false},
  {"run_id": "A2",   "arm": "control",   "parent_run_id": "B2",   "parent_arm": "candidate",
   "config_sha": "cecf16db8b0b", "parent_config_sha": "fcdd205df41f",
   "config_changed": true,  "goodput_delta": 44.818,  "outcome": "rejected", "drift_detected": false}
]
```

Five rows for six periods: `A0-1` has no parent, so the join drops it. The two
`config_changed` rows are exactly the two arm switches (`A1 → B1`, `B2 → A2`),
and the two 44-unit swings around them are the candidate being 8× slower.
Before `finalize-experiment` existed, the last two columns were `null` on every
row and this query could not answer its own question.

### 2. Op-class diff: which kernels moved between the arms

```sql
SELECT kernel,
       sum(CASE WHEN run_id = 'A1' THEN total_ns ELSE 0 END) AS control_ns,
       sum(CASE WHEN run_id = 'B1' THEN total_ns ELSE 0 END) AS candidate_ns,
       sum(CASE WHEN run_id = 'B1' THEN total_ns ELSE 0 END)
         - sum(CASE WHEN run_id = 'A1' THEN total_ns ELSE 0 END) AS delta_ns,
       sum(executions) AS executions,
       any_value(bucket) AS bucket
FROM lake.sigiro_gpu_kernels
WHERE experiment_id = 'local-known-slower' AND run_id IN ('A1', 'B1')
GROUP BY kernel
ORDER BY abs(sum(CASE WHEN run_id = 'B1' THEN total_ns ELSE 0 END)
             - sum(CASE WHEN run_id = 'A1' THEN total_ns ELSE 0 END)) DESC, kernel
```

Answers "where did the GPU time go between these two runs" — biggest mover
first, with a kernel that exists in only one arm showing a zero on the other
side.

```json
[
  {"kernel": "paged_attn_fwd",  "control_ns": 0,    "candidate_ns": 9000, "delta_ns": 9000,
   "executions": 2, "bucket": "attributed"},
  {"kernel": "flash_attn_fwd",  "control_ns": 6000, "candidate_ns": 0,    "delta_ns": -6000,
   "executions": 2, "bucket": "attributed"},
  {"kernel": "sm90_gemm",       "control_ns": 4000, "candidate_ns": 5000, "delta_ns": 1000,
   "executions": 8, "bucket": "attributed"},
  {"kernel": "elementwise_add", "control_ns": 1000, "candidate_ns": 1000, "delta_ns": 0,
   "executions": 2, "bucket": "residual_no_provenance"}
]
```

The candidate swapped `flash_attn_fwd` for `paged_attn_fwd`: one appears, the
other vanishes, and the rest is a 1000 ns GEMM drift. This needs **no layer
attribution** — it groups by `kernel`, a column every profiler export carries —
so it works at any quantization and on an engine whose graph provenance the
collector could not resolve. Because `bucket` is never null, the residual row is
in the diff too and the columns sum to each run's whole GPU time (11000 ns
control, 15000 ns candidate) rather than to the attributable part of it.

The local E2E has no GPU, so its `sigiro_gpu_kernels` rows are a fixture written
to Parquet and ingested through the real `sigiro ingest-kernels`. The query is
what is under test there; the numbers come from a real profiler only under
`check-inference-optimization-gpu` (see `scripts/gpu_kernel_collect.py`).

## The `topology` block

Optional, and every field inside it is optional too. It exists for the facts the
runner cannot observe but the operator knows, and its field names *are* the
ledger's column names, passed straight through:

```json
"topology": {
  "engine": "sglang",
  "engine_version": "0.4.6",
  "gpu_model": "NVIDIA GeForce RTX 3090",
  "gpu_count": 1,
  "tp_size": 1,
  "quantization": "bf16",
  "price_per_hour_usd": 0.22
}
```

A typo fails the period: `sigiro record-run` rejects unknown fields, so
`gpu_modle` is an error rather than a column that silently stays null.

It is deliberately **not** folded into any fingerprint. It is one top-level
block, so both arms of a comparison always declare the same topology and it can
never be the difference between them; across experiments the columns are right
there in the table to filter on. The fingerprints stay what they are — hashes of
things actually read from the machine.

## The `sidecars` block

Optional array of processes started **before the target in every period** and
stopped after it, so both arms see identical conditions:

```json
"sidecars": [
  {"name": "otelcol", "argv": ["otelcol", "--config", "./otelcol.yaml"], "env": {}}
]
```

Each sidecar writes `<period>/sidecar-<name>.log`, runs in its own process group
(so stopping it takes its children with it), and is restarted per period rather
than left running across the six — a collector that leaked state from A1 into B1
would make the reversal meaningless.

The name and argv are folded into the **benchmark fingerprint**: a collector
scraping the engine every second changes what is being measured, so a run with
one is not comparable to a run without. `env` is not, being host wiring rather
than a change in what is measured.

**A sidecar that exits before the period ends fails the experiment.** A dead
collector does not fail AIPerf — it just stops recording, and the period would
look clean while measuring something the other arm did not.

### Example: an OTel Collector sidecar for engine and GPU metrics

This is documentation, not a test dependency: the suite ships no collector binary
and no Docker requirement for it. It matters because AIPerf's OTLP exporter only
subscribes to `metric_records` and `credit_phase_stats` — its `server_metrics`
and `gpu_telemetry` collectors write JSONL files and **never** reach the network,
so engine and GPU metrics do not arrive in Sigiro on their own. A collector
sidecar is how they get there, and running it in both arms is what keeps the
comparison honest.

`otelcol.yaml`, scraping the engine's `/metrics` and a running `dcgm-exporter`,
exporting to the manifest's `otlp_http_port`:

```yaml
receivers:
  prometheus:
    config:
      scrape_configs:
        - job_name: engine
          scrape_interval: 5s
          static_configs:
            - targets: ["127.0.0.1:30000"]   # the engine's port, as in control.argv
        - job_name: dcgm
          scrape_interval: 5s
          static_configs:
            - targets: ["127.0.0.1:9400"]    # dcgm-exporter

processors:
  # Same resource attributes AIPerf stamps on its own metrics, so a query can
  # join engine and GPU series to the period that produced them.
  resource:
    attributes:
      - key: optimization.experiment.id
        value: gpu-qwen3-4b-example
        action: upsert
      - key: service.name
        value: engine
        action: upsert

exporters:
  otlphttp:
    endpoint: http://127.0.0.1:4318      # manifest.sigiro.otlp_http_port
    tls:
      insecure: true

service:
  pipelines:
    metrics:
      receivers: [prometheus]
      processors: [resource]
      exporters: [otlphttp]
```

```json
"sidecars": [
  {
    "name": "otelcol",
    "argv": ["otelcol-contrib", "--config", "./otelcol.yaml"],
    "env": {}
  },
  {
    "name": "dcgm-exporter",
    "argv": ["dcgm-exporter", "--address", "127.0.0.1:9400"],
    "env": {}
  }
]
```

Two limits to know before relying on it. The collector cannot stamp
`optimization.run.id` — it is one process across all six periods and does not
know which one is running — so engine and GPU series are separated by *time
window*, using the `from_ts`/`to_ts` the comparison request already records per
period, not by run id the way AIPerf's own metrics are. And the collector starts
before the engine does, so its first scrapes fail until the engine binds its
port; the Prometheus receiver retries, and those failures are in
`<period>/sidecar-otelcol.log`.

## The `telemetry` block

Required, and cross-checked against the flags actually passed. AIPerf defaults GPU
telemetry and server metrics **on** — `no_gpu_telemetry` and `no_server_metrics`
are both `= False` — so an undeclared manifest is silently instrumented. Enabling a
collector changes the performance being recorded, so the declaration is part of the
benchmark fingerprint, and a declaration that disagrees with the command fails
loudly rather than drifting.
