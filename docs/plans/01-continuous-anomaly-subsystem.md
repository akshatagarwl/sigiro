# Continuous, threshold-free anomaly subsystem

> STATUS: P0+P1+P2 SHIPPED — ALL SIGNALS live: error_rate + latency_p95 per
> (service, operation), log_volume + error_log_rate + profile_cost (total
> profiled cost — per-function was ~880 rows of pure noise at ~10x the scan
> cost on real data) per service. One `sigiro_anomalies` table (the `operation`
> column holds the entity), scheduled BOCPD pass (detect on reader / persist on
> writer — ingest never blocked), investigate + `/v1/anomalies` read it, ranked
> CROSS-SIGNAL by the unit-free symmetric relative change
> (after − before)/(after + before), raw |Δ| breaking zero-baseline ties (no
> filter). 5-min buckets. P2 cross-signal correlation live: per service, shifts
> in the same or ADJACENT detection bucket (the pass's own 5-min resolution —
> structural, not a threshold; `anomaly::correlate_shifts`) merge into ONE
> ranked "incident" finding in /diagnose (member shifts nested, each with
> its drill; deploy in/just before the window attached as prime suspect;
> incidents rank by (member count, magnitude, |Δ|) so multi-signal beats lone);
> lone shifts stay single-signal findings, nothing dropped. `/v1/anomalies`
> keeps raw rows and adds `ts_shift_us` + a null-able `incident_id` shared by
> correlated rows. NEXT: P3 seasonality + conformal (needs longer retention).


## Two hard constraints (from the user)
1. **No arbitrary thresholds.** No `> 10%`, `> 2×`, `> 100ms`, `> 5`, `> 1s`.
   Detection is Bayesian/statistical only. The single tunable allowed is a
   *principled statistical level* (BOCPD prior, conformal coverage like 99%) —
   a standard parameter, not a domain magic number.
2. **No temporary features — long-term only.** Anomaly detection does not live
   inline in each `/diagnose` call (that only sees the request window and
   recomputes every time). It runs continuously over persisted history; the API
   reads precomputed results.

## What exists today (to be superseded / removed)
- **Inline augurs-changepoint in `investigate_one`** (5a808ff) — the interim.
  Correct, but window-scoped and recomputed per call. Superseded by the
  scheduled pass; removed once the pass lands (no gap).
- **`sigiro_anomalies` view** (schema.rs) — leftover arbitrary thresholds
  (`error_pct > 5`, `p95_us > 1000000`) and a fixed now()-5min window. Not
  wired into investigate. **Remove** — the anomalies table replaces it.
- No `compactor.rs`; the periodic home is `inserter.rs` (already runs flush +
  retention on a timer). The scheduled pass extends that loop.

## Architecture
```
signals (spans/logs/metrics/profiles)
  → [1] series extraction: regular-cadence rollups, persisted
  → [2] retention: downsampled long-history (weeks, for seasonality)
  → [3] scheduled pass (extends inserter timer): augurs over continuous series
        - changepoint (BOCPD, threshold-free) — shipped algorithm
        - forecast band via CONFORMAL intervals (distribution-free) — later
        - seasonality (periodogram) once history is long enough — later
  → [4] anomalies table: {ts, service, operation, signal, kind, direction,
        before, after, score, drill_down_sql}  (persisted, no thresholds)
  → [5] cross-signal correlation: align anomalies by time → ranked findings
  → [6] /diagnose + /v1/anomalies READ the table (cheap, no inline compute)
```

## Threshold-free ledger (what's removed → what replaces it)
| Removed arbitrary threshold | Replacement |
|---|---|
| `err_pct > 10 & n>=20`, `> 2× baseline`, `p95 > 100ms` (already gone) | BOCPD changepoint |
| `sigiro_anomalies` view `err_pct > 5`, `p95_us > 1e6` | anomalies table from the pass |
| inline `MIN_SERIES_POINTS = 8` abstain gate | BOCPD needs no minimum; on a constant/short series it emits no changepoint by construction → drop the gate |
| "worsening = mean after > before" | keep — a direction comparison, not a threshold |

## Phases
- **P0 — remove leftover thresholds + de-inline.** Delete the `sigiro_anomalies`
  view. Keep detection working by having the (interim) pass write the anomalies
  table; investigate reads it. Removes all remaining magic numbers.
- **P1 — series extraction + scheduled changepoint pass + anomalies table.**
  Roll up spans→(latency/error/throughput per op), logs→(volume/per-template),
  profiles→(per-function) at fixed cadence; run BOCPD in the inserter timer;
  persist. Investigate/`/v1/anomalies` read.
- **P2 — cross-signal correlation.** SHIPPED. Align changepoints across
  signals by timestamp + deploy events → one ranked root-cause finding, not N
  alerts. Alignment window = same-or-adjacent detection bucket (transitive),
  i.e. the pass's own 5-min resolution — BOCPD places every changepoint on a
  bucket boundary, so adjacent buckets are the finest alignment the detector
  supports; no picked constant. Deploys correlate when their bucket falls in
  the incident's bucket span or the bucket immediately before its first shift.
- **P3 — seasonality + conformal forecast bands.** Needs longer retention
  (weeks). Seasonality-aware expectations; conformal intervals (coverage level,
  not a threshold).

## Open decisions (resolve before P1)
- Rollup cadence + retention/downsampling tiers (raw 7d → 1m rollup Nd → 1h
  rollup weeks?).
- Anomalies table: DuckLake table vs control-plane Postgres.
- Scheduled cadence (reuse inserter timer interval vs its own).
- Cross-signal alignment window (P2). RESOLVED: same-or-adjacent 5-min
  detection bucket, per service (see P2 above).

## Non-goals
- No `augurs-outlier` (MAD DivideByZero on mostly-zero error series — rejected).
- No `augurs-ets` point forecast (marginal; conformal band in P3 instead).
- No new anomaly *thresholds* anywhere, ever.
