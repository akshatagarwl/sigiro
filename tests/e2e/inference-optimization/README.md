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
per-run evidence SQL, ledger discipline, and the candidate/control *ranking*
(robust to noise even when the intervals are not).

## Two settings that are not arbitrary

- **`--request-count 200`.** At 40 requests and 15 ms TTFT, one profile run is
  ~300 ms of work, so AIPerf's own startup and host scheduling dominate the rate
  being measured. 200 makes it ~1.5 s and the noise collapses. This was the actual
  cause of the flakiness — not interval width, which is what it looked like.
- **`num_profile_runs: 3`.** `t_critical` is 4.303 at n=3 against 12.706 at n=2,
  where the interval is wide enough to swallow a real 3× difference.

## The `roofline` block

Optional, and complete when present. The local E2E omits it: a mock server has no
memory floor to be near, so a headroom figure computed against one would be
fiction.

It records only the **inputs** to an analytic decode floor, never a derived
headroom ratio, because the ratio needs three choices the runner is in no position
to make — the engine's decode batch (client concurrency is not the same thing),
the context length, and whether the numerator is tokens or requests. Derive it
over the ledger instead, where those can vary without re-running the experiment:

```
batch-1 decode ceiling = peak_bw / (weight_bytes + kv_bytes_per_token * ctx)
KV-bound ceiling       = peak_bw / (kv_bytes_per_token * ctx)
```

For Qwen3-4B on one RTX 3090 at ctx 2048 that is **112 tok/s** and **3100 tok/s**.
Measure 95 and you are at 85% of the hardware limit — stop searching, no config
knob will find the missing 17. Measure 30 and something is stalling, so the search
is worth funding.

The block is folded into the **hardware fingerprint**: two runs claiming different
peak bandwidth for the same box are making different claims about the machine and
must not compare as equal. `source` is mandatory because a stale bandwidth figure
silently corrupts every number derived from it.

## The `telemetry` block

Required, and cross-checked against the flags actually passed. AIPerf defaults GPU
telemetry and server metrics **on** — `no_gpu_telemetry` and `no_server_metrics`
are both `= False` — so an undeclared manifest is silently instrumented. Enabling a
collector changes the performance being recorded, so the declaration is part of the
benchmark fingerprint, and a declaration that disagrees with the command fails
loudly rather than drifting.
