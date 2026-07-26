/**
 * The series behind the changepoint illustration on the landing page.
 *
 * Shape matches what the anomaly pass actually reads: one point per 5-minute
 * detection bucket, error rate as a percentage and p95 in microseconds — the raw
 * signal units the `sigiro_anomalies` row stores in `before` / `after`. The shift
 * lands in the same bucket for both signals, which is exactly the condition that
 * makes `correlate_shifts` group them under one incident_id.
 */
export type Bucket = { t: string; errorPct: number; p95Us: number };

const BEFORE: Array<[number, number]> = [
  [0.3, 402_000],
  [0.5, 418_000],
  [0.4, 396_000],
  [0.4, 431_000],
  [0.6, 405_000],
  [0.3, 388_000],
  [0.4, 424_000],
  [0.5, 412_000],
];

const AFTER: Array<[number, number]> = [
  [14.9, 2_980_000],
  [18.2, 3_310_000],
  [16.4, 3_120_000],
  [17.8, 3_240_000],
  [17.1, 3_050_000],
  [16.9, 3_290_000],
  [17.6, 3_180_000],
];

/** Bucket index where the new regime starts — 18:35, two after the deploy. */
export const SHIFT_INDEX = BEFORE.length;
export const DEPLOY_INDEX = BEFORE.length - 2;

export const SERIES: Bucket[] = [...BEFORE, ...AFTER].map(([errorPct, p95Us], i) => {
  const minutes = i * 5;
  const start = 17 * 60 + 55; // first bucket at 17:55 UTC
  const clock = start + minutes;
  const t = `${String(Math.floor(clock / 60) % 24).padStart(2, '0')}:${String(clock % 60).padStart(2, '0')}`;
  return { t, errorPct, p95Us };
});

const mean = (xs: number[]) => xs.reduce((a, b) => a + b, 0) / xs.length;

/** The two numbers the anomaly row reports, computed from the series above. */
export const SHIFT = {
  errorPctBefore: Number(mean(BEFORE.map((b) => b[0])).toFixed(1)),
  errorPctAfter: Number(mean(AFTER.map((b) => b[0])).toFixed(1)),
  p95UsBefore: Math.round(mean(BEFORE.map((b) => b[1]))),
  p95UsAfter: Math.round(mean(AFTER.map((b) => b[1]))),
  at: SERIES[SHIFT_INDEX].t,
  deployAt: SERIES[DEPLOY_INDEX].t,
};
