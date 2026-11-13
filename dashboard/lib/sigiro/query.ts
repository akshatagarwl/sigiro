import { BROWSER_CLIENT_SCOPE, createBrowserOAuth } from "./browser-oauth";

/**
 * The dashboard's only data source: `POST /v1/query` with the signed-in
 * reader's bearer token. Every panel sends its own SQL and the server does the
 * aggregation, so nothing is capped or sampled in the browser.
 */

export const auth = createBrowserOAuth({
  redirectPath: "/dashboard/",
  scope: BROWSER_CLIENT_SCOPE,
  transientKey: "sigiro.dashboard.oauth",
  tokenKey: "sigiro.dashboard.token",
});

export type Cell = string | number | boolean | null;
export type Row = Record<string, Cell>;

export class QueryError extends Error {
  constructor(
    readonly status: number,
    detail: string
  ) {
    super(`${status}: ${detail}`);
    this.name = "QueryError";
  }
}

/** The query API admits a few queries per workspace at once and answers 429 past that. */
const MAX_IN_FLIGHT = 4;
let inFlight = 0;
const waiting: (() => void)[] = [];

const acquire = () =>
  inFlight < MAX_IN_FLIGHT
    ? (inFlight++, Promise.resolve())
    : new Promise<void>((resolve) => waiting.push(resolve));

const release = () => {
  const next = waiting.shift();
  if (next) next();
  else inFlight--;
};

const sleep = (ms: number, signal?: AbortSignal) =>
  new Promise<void>((resolve, reject) => {
    const timer = setTimeout(resolve, ms);
    signal?.addEventListener("abort", () => {
      clearTimeout(timer);
      reject(signal.reason);
    });
  });

const detailOf = (text: string) => {
  try {
    const problem = JSON.parse(text) as { detail?: string; title?: string };
    return problem.detail ?? problem.title ?? text;
  } catch {
    return text;
  }
};

/**
 * A dashboard served by a self-hosted sigiro on this machine talks to an
 * open-mode server, which has no sign-in and ignores the bearer. On a
 * loopback origin whose `/v1/query` answers without a token, a placeholder
 * stands in for the OAuth token.
 */
let openMode = false;
export const localToken = (): string | undefined =>
  openMode ? "local-open-mode" : undefined;

export async function detectOpenMode(): Promise<boolean> {
  const host = globalThis.location?.hostname;
  if (host !== "localhost" && host !== "127.0.0.1" && host !== "[::1]")
    return false;
  try {
    const response = await fetch("/v1/query", {
      method: "POST",
      credentials: "omit",
      headers: { "content-type": "text/plain", accept: "application/json" },
      body: "SELECT 1 AS ok",
    });
    openMode = response.ok;
  } catch {
    openMode = false;
  }
  return openMode;
}

export async function query(sql: string, signal?: AbortSignal): Promise<Row[]> {
  const token = auth.currentToken() ?? localToken();
  if (!token) throw new QueryError(401, "Not signed in.");
  await acquire();
  try {
    for (let attempt = 0; ; attempt++) {
      const response = await fetch("/v1/query", {
        method: "POST",
        credentials: "omit",
        headers: {
          authorization: `Bearer ${token}`,
          "content-type": "text/plain",
          accept: "application/json",
        },
        body: sql,
        signal,
      });
      if (response.status === 429 && attempt < 6) {
        const after = Number(response.headers.get("retry-after"));
        await sleep(after > 0 ? after * 1000 : 200 * 2 ** attempt, signal);
        continue;
      }
      const text = await response.text();
      if (!response.ok) {
        if (response.status === 401) auth.forget();
        throw new QueryError(response.status, detailOf(text));
      }
      return JSON.parse(text) as Row[];
    }
  } finally {
    release();
  }
}

export const sqlString = (value: string) => `'${value.replaceAll("'", "''")}'`;

/** `/v1/query` sends TIMESTAMP columns as epoch microseconds. */
export const fromMicros = (value: Cell) => new Date(Number(value) / 1000);

/** A TIMESTAMP literal for a JS date, microsecond precision. */
export const ts = (date: Date) => `make_timestamp(${date.getTime() * 1000})`;

export const num = (value: Cell | undefined) =>
  value === null || value === undefined ? 0 : Number(value);

/** The bucket a chart over `[from, to)` groups by. */
export const bucketFor = (from: Date, to: Date) => {
  const span = to.getTime() - from.getTime();
  if (span <= 3_600_000) return { sql: "1 minute", ms: 60_000 };
  if (span <= 21_600_000) return { sql: "5 minutes", ms: 300_000 };
  if (span <= 86_400_000) return { sql: "15 minutes", ms: 900_000 };
  if (span <= 604_800_000) return { sql: "2 hours", ms: 7_200_000 };
  return { sql: "1 day", ms: 86_400_000 };
};

/** A failed span: OTLP status code ERROR. */
export const FAILED_SQL = "status_code = 2";

/**
 * Failed ÷ all spans as a percentage, rounded on the server over the
 * header's time window and selected service.
 */
export const ERROR_RATE_SQL = `round(100.0 * count(*) FILTER (WHERE ${FAILED_SQL}) / nullif(count(*), 0), 2)`;

/** Severity buckets used by every logs view. */
export const LEVELS = ["error", "warning", "info", "debug"] as const;
export type Level = (typeof LEVELS)[number];
export const LEVEL_SQL =
  "CASE WHEN severity_number >= 17 THEN 'error' WHEN severity_number >= 13 THEN 'warning' WHEN severity_number >= 9 THEN 'info' ELSE 'debug' END";
