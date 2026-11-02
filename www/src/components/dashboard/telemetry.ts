import * as duckdb from "@duckdb/duckdb-wasm";
import {
  DataType,
  Float64,
  Int32,
  Int64,
  Table,
  Utf8,
  type Vector,
  vectorFromArray,
} from "apache-arrow";
import * as Option from "effect/Option";
import * as Schema from "effect/Schema";

export const WINDOWS = {
  "1h": { ms: 3_600_000, bucket: "1 minute", bucketMs: 60_000 },
  "6h": { ms: 21_600_000, bucket: "5 minutes", bucketMs: 300_000 },
  "24h": { ms: 86_400_000, bucket: "15 minutes", bucketMs: 900_000 },
  "7d": { ms: 604_800_000, bucket: "2 hours", bucketMs: 7_200_000 },
} as const;
export type WindowKey = keyof typeof WINDOWS;
export const WINDOW_KEYS: readonly WindowKey[] = ["1h", "6h", "24h", "7d"];

export const RAW_ROW_CAP = 5000;
export const SPAN_SAMPLE_CAP = 20_000;

export type Window = { key: WindowKey; from: Date; to: Date };

export const windowFor = (key: WindowKey, now = Date.now()): Window => {
  const { ms, bucketMs } = WINDOWS[key];
  const to = Math.ceil(now / bucketMs) * bucketMs;
  return { key, from: new Date(to - ms), to: new Date(to) };
};

const literal = (date: Date) =>
  `TIMESTAMP '${date.toISOString().slice(0, 19).replace("T", " ")}'`;

const range = (w: Window) =>
  `timestamp >= ${literal(w.from)} AND timestamp < ${literal(w.to)}`;

const bucketOf = (w: Window) =>
  `time_bucket(INTERVAL '${WINDOWS[w.key].bucket}', timestamp)`;

/** Epoch microseconds on the wire, `TIMESTAMP` in DuckDB-WASM. */
const TS = "TIMESTAMP_US";

/** Columns are typed here, not inferred, so an empty window still loads typed, queryable tables. */
type ColumnType = typeof TS | "BIGINT" | "INTEGER" | "DOUBLE" | "VARCHAR";
type Columns = Readonly<Record<string, ColumnType>>;
type ServerTable = {
  name: string;
  description: string;
  columns: Columns;
  sql: (w: Window, context: { spanTotal: number }) => string;
};

export const SERVER_TABLES: ServerTable[] = [
  {
    name: "log_buckets",
    description: "log count per bucket, service and severity (exact)",
    columns: {
      bucket: TS,
      service_name: "VARCHAR",
      severity_number: "INTEGER",
      n: "BIGINT",
    },
    sql: (w) =>
      `SELECT ${bucketOf(w)} AS bucket, service_name, severity_number, count(*) AS n FROM sigiro_logs WHERE ${range(w)} GROUP BY 1, 2, 3`,
  },
  {
    name: "log_templates",
    description: "warning-and-above log templates per bucket (exact)",
    columns: {
      bucket: TS,
      service_name: "VARCHAR",
      severity_number: "INTEGER",
      template_id: "VARCHAR",
      template_text: "VARCHAR",
      example: "VARCHAR",
      n: "BIGINT",
    },
    sql: (w) =>
      `SELECT ${bucketOf(w)} AS bucket, service_name, severity_number, CAST(template_id AS VARCHAR) AS template_id, any_value(template_text) AS template_text, any_value(body) AS example, count(*) AS n FROM sigiro_logs WHERE ${range(w)} AND severity_number >= 13 GROUP BY 1, 2, 3, 4 ORDER BY n DESC LIMIT ${RAW_ROW_CAP}`,
  },
  {
    name: "sigiro_logs",
    description: `newest ${RAW_ROW_CAP.toLocaleString("en")} log rows`,
    columns: {
      timestamp: TS,
      service_name: "VARCHAR",
      severity_number: "INTEGER",
      severity_text: "VARCHAR",
      body: "VARCHAR",
      template_text: "VARCHAR",
      log_attributes: "VARCHAR",
      trace_id: "VARCHAR",
      span_id: "VARCHAR",
    },
    sql: (w) =>
      `SELECT timestamp, service_name, severity_number, severity_text, body, template_text, log_attributes, trace_id, span_id FROM sigiro_logs WHERE ${range(w)} ORDER BY timestamp DESC LIMIT ${RAW_ROW_CAP}`,
  },
  {
    name: "span_buckets",
    description:
      "span count and total duration per bucket, service, span and status (exact)",
    columns: {
      bucket: TS,
      service_name: "VARCHAR",
      span_name: "VARCHAR",
      status_code: "INTEGER",
      n: "BIGINT",
      total_us: "DOUBLE",
    },
    sql: (w) =>
      `SELECT ${bucketOf(w)} AS bucket, service_name, span_name, status_code, count(*) AS n, sum(duration) AS total_us FROM sigiro_spans WHERE ${range(w)} GROUP BY 1, 2, 3, 4`,
  },
  {
    name: "failed_spans",
    description:
      "failed spans per bucket, service, span and status message (exact)",
    columns: {
      bucket: TS,
      service_name: "VARCHAR",
      span_name: "VARCHAR",
      status_message: "VARCHAR",
      n: "BIGINT",
      trace_id: "VARCHAR",
    },
    sql: (w) =>
      `SELECT ${bucketOf(w)} AS bucket, service_name, span_name, status_message, count(*) AS n, any_value(trace_id) AS trace_id FROM sigiro_spans WHERE ${range(w)} AND status_code = 2 GROUP BY 1, 2, 3, 4 ORDER BY n DESC LIMIT ${RAW_ROW_CAP}`,
  },
  {
    name: "sigiro_spans",
    description: "uniform sample of spans, keyed on span_id",
    columns: {
      timestamp: TS,
      service_name: "VARCHAR",
      span_name: "VARCHAR",
      duration: "BIGINT",
      status_code: "INTEGER",
      status_message: "VARCHAR",
      trace_id: "VARCHAR",
      span_id: "VARCHAR",
      parent_span_id: "VARCHAR",
      span_kind: "INTEGER",
    },
    sql: (w, { spanTotal }) => {
      const keep = spanSampleThreshold(spanTotal);
      const sample =
        keep < 256
          ? ` AND substr(span_id, 15, 2) < '${keep.toString(16).padStart(2, "0")}'`
          : "";
      return `SELECT timestamp, service_name, span_name, duration, status_code, status_message, trace_id, span_id, parent_span_id, span_kind FROM sigiro_spans WHERE ${range(w)}${sample} LIMIT ${SPAN_SAMPLE_CAP}`;
    },
  },
  {
    name: "sigiro_metrics_histogram",
    description: `newest ${RAW_ROW_CAP.toLocaleString("en")} histogram points`,
    columns: {
      timestamp: TS,
      service_name: "VARCHAR",
      metric_name: "VARCHAR",
      metric_unit: "VARCHAR",
      count: "BIGINT",
      sum: "DOUBLE",
      min: "DOUBLE",
      max: "DOUBLE",
      metric_attributes: "VARCHAR",
    },
    sql: (w) =>
      `SELECT timestamp, service_name, metric_name, metric_unit, count, sum, min, max, metric_attributes FROM sigiro_metrics_histogram WHERE ${range(w)} ORDER BY timestamp DESC LIMIT ${RAW_ROW_CAP}`,
  },
  {
    name: "sigiro_profiles",
    description:
      "profile value per service, type and leaf frame (exact, top 5,000)",
    columns: {
      service_name: "VARCHAR",
      profile_type: "VARCHAR",
      unit: "VARCHAR",
      leaf_frame: "VARCHAR",
      samples: "BIGINT",
      value: "DOUBLE",
    },
    sql: (w) =>
      `SELECT service_name, profile_type, unit, list_extract(stack_trace, 1) AS leaf_frame, count(*) AS samples, sum(value) AS value FROM sigiro_profiles WHERE ${range(w)} GROUP BY 1, 2, 3, 4 ORDER BY value DESC LIMIT ${RAW_ROW_CAP}`,
  },
];

/** Out of 256: keep spans whose last span_id byte is below this. */
export const spanSampleThreshold = (total: number) =>
  total <= SPAN_SAMPLE_CAP
    ? 256
    : Math.max(1, Math.floor((SPAN_SAMPLE_CAP / total) * 256));

export class QueryError extends Error {
  constructor(
    readonly status: number,
    readonly table: string,
    detail: string
  ) {
    super(`${table}: ${status} ${detail}`);
  }
}

export type LoadReport = {
  table: string;
  description: string;
  rows: number;
  truncated: boolean;
};

const fetchTable = async (
  token: string,
  table: { name: string },
  sql: string
): Promise<{ text: string; truncated: boolean }> => {
  const response = await fetch("/v1/query", {
    method: "POST",
    credentials: "omit",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "text/plain",
      accept: "application/json",
    },
    body: sql,
  });
  const text = await response.text();
  if (!response.ok) {
    const problem = Option.getOrUndefined(
      Schema.decodeUnknownOption(Problem)(text)
    );
    throw new QueryError(
      response.status,
      table.name,
      problem?.detail ?? problem?.title ?? text
    );
  }
  return {
    text,
    truncated: response.headers.get("x-sigiro-truncated") === "true",
  };
};

let dbPromise: Promise<duckdb.AsyncDuckDB> | undefined;

/** DuckDB-WASM from this origin, in a Worker; nothing is fetched from a CDN. */
export const database = () => {
  dbPromise ??= (async () => {
    const features = await duckdb.getPlatformFeatures();
    if (!features.wasmExceptions)
      throw new Error(
        "This browser has no WebAssembly exception handling, which DuckDB-WASM needs."
      );
    const base = new URL(`/duckdb/${duckdb.PACKAGE_VERSION}/`, location.origin);
    const worker = new Worker(new URL("duckdb-browser-eh.worker.js", base));
    const db = new duckdb.AsyncDuckDB(new duckdb.VoidLogger(), worker);
    await db.instantiate(new URL("duckdb-eh.wasm", base).href);
    const conn = await db.connect();
    await conn.query(
      "SET autoinstall_known_extensions = false; SET autoload_known_extensions = false"
    );
    await conn.close();
    return db;
  })();
  return dbPromise;
};

const Problem = Schema.fromJsonString(
  Schema.Struct({
    detail: Schema.optional(Schema.String),
    title: Schema.optional(Schema.String),
  })
);

/** `/v1/query` answers one JSON object per row; every column this page selects is a scalar. */
const Rows = Schema.fromJsonString(
  Schema.Array(
    Schema.Record(
      Schema.String,
      Schema.Union([Schema.String, Schema.Number, Schema.Boolean, Schema.Null])
    )
  )
);
type Row = (typeof Rows)["Type"][number];
type Scalar = Row[string];

const SQL_TYPE = (type: ColumnType) => (type === TS ? "TIMESTAMP" : type);

const vectorOf = (type: ColumnType, values: Scalar[]) => {
  switch (type) {
    case TS:
    case "BIGINT":
      return vectorFromArray(
        values.map((value) =>
          value === null ? null : BigInt(Math.trunc(Number(value)))
        ),
        new Int64()
      );
    case "INTEGER":
      return vectorFromArray(
        values.map((value) => (value === null ? null : Number(value))),
        new Int32()
      );
    case "DOUBLE":
      return vectorFromArray(
        values.map((value) => (value === null ? null : Number(value))),
        new Float64()
      );
    case "VARCHAR":
      return vectorFromArray(
        values.map((value) => (value === null ? null : String(value))),
        new Utf8()
      );
  }
};

/**
 * JSON rows become a typed Arrow table, so DuckDB-WASM needs no `json`
 * extension (it is not built in, and fetching it would leave the origin).
 * Timestamps arrive as epoch microseconds and are converted in SQL.
 */
const insertRows = async (
  conn: duckdb.AsyncDuckDBConnection,
  name: string,
  columns: Columns,
  rows: readonly Row[]
) => {
  const entries = Object.entries(columns);
  await conn.query(
    `CREATE OR REPLACE TABLE ${name} (${entries.map(([column, type]) => `"${column}" ${SQL_TYPE(type)}`).join(", ")})`
  );
  if (rows.length === 0) return;
  const arrow = new Table(
    Object.fromEntries(
      entries.map(([column, type]) => [
        column,
        vectorOf(
          type,
          rows.map((row) => row[column] ?? null)
        ),
      ])
    )
  );
  const staging = `${name}__staging`;
  await conn.insertArrowTable(arrow, { name: staging, create: true });
  await conn.query(
    `INSERT INTO ${name} SELECT ${entries.map(([column, type]) => (type === TS ? `make_timestamp("${column}")` : `"${column}"`)).join(", ")} FROM ${staging}`
  );
  await conn.query(`DROP TABLE ${staging}`);
};

export const loadWindow = async (
  token: string,
  w: Window,
  onProgress: (report: LoadReport) => void
): Promise<LoadReport[]> => {
  const db = await database();
  const conn = await db.connect();
  const reports: LoadReport[] = [];
  const context = { spanTotal: 0 };
  try {
    for (const table of SERVER_TABLES) {
      const { text, truncated } = await fetchTable(
        token,
        table,
        table.sql(w, context)
      );
      const parsed = Schema.decodeUnknownSync(Rows)(text);
      await insertRows(conn, table.name, table.columns, parsed);
      const rows = parsed.length;
      if (table.name === "span_buckets")
        context.spanTotal = Number(
          (
            await conn.query(
              "SELECT coalesce(sum(n), 0) AS n FROM span_buckets"
            )
          ).get(0)?.n
        );
      const report = {
        table: table.name,
        description:
          table.name === "sigiro_spans"
            ? `${table.description}, ${spanSampleThreshold(context.spanTotal)}/256 kept`
            : table.description,
        rows,
        truncated,
      };
      reports.push(report);
      onProgress(report);
    }
    await conn.query(
      `CREATE OR REPLACE TABLE dashboard_window AS SELECT ${literal(w.from)} AS window_from, ${literal(w.to)} AS window_to, ${spanSampleThreshold(context.spanTotal)} / 256.0 AS span_sample_rate`
    );
  } finally {
    await conn.close();
  }
  return reports;
};

export type Cell = string | number | boolean | null;
export type Result = { columns: string[]; numeric: boolean[]; rows: Cell[][] };

const isNumeric = (type: DataType) =>
  DataType.isInt(type) || DataType.isFloat(type) || DataType.isDecimal(type);

const cell = (vector: Vector | null, row: number, type: DataType): Cell => {
  const value = vector?.get(row);
  if (value === null || value === undefined) return null;
  if (DataType.isTimestamp(type) || DataType.isDate(type))
    return new Date(Number(value))
      .toISOString()
      .replace("T", " ")
      .replace(".000Z", "");
  if (DataType.isDecimal(type)) {
    const scaled = Number(String(value)) / 10 ** type.scale;
    return Number.isSafeInteger(Math.trunc(scaled)) ? scaled : String(value);
  }
  if (DataType.isInt(type) || DataType.isFloat(type))
    return Number.isSafeInteger(Math.trunc(Number(value)))
      ? Number(value)
      : String(value);
  if (DataType.isBool(type)) return Boolean(value);
  return String(value);
};

const toResult = (table: Table): Result => {
  const fields = table.schema.fields;
  const vectors = fields.map((_, index) => table.getChildAt(index));
  const rows: Cell[][] = [];
  for (let row = 0; row < table.numRows; row++)
    rows.push(
      fields.map((field, index) =>
        cell(vectors[index] ?? null, row, field.type)
      )
    );
  return {
    columns: fields.map((field) => field.name),
    numeric: fields.map((field) => isNumeric(field.type)),
    rows,
  };
};

export const runLocal = async (sql: string): Promise<Result> => {
  const db = await database();
  const conn = await db.connect();
  try {
    return toResult(await conn.query(sql));
  } finally {
    await conn.close();
  }
};

export const sqlString = (value: string) => `'${value.replaceAll("'", "''")}'`;

/**
 * One trace, read from the server rather than the local sample: the sample
 * keeps a fraction of spans, so a waterfall built from it would have holes.
 */
export const fetchTrace = async (token: string, traceId: string) => {
  if (!/^[0-9a-f]{32}$/i.test(traceId)) throw new Error("Not a trace id.");
  const id = sqlString(traceId.toLowerCase());
  // One at a time: the query API admits one query per tenant at once (429).
  const spans = await fetchTable(
    token,
    { name: "trace" },
    `SELECT timestamp AS start_us, duration, service_name, span_name, span_id, parent_span_id, span_kind, status_code, status_message, span_attributes FROM sigiro_spans WHERE trace_id = ${id} ORDER BY timestamp LIMIT 2000`
  );
  const logs = await fetchTable(
    token,
    { name: "trace logs" },
    `SELECT timestamp AS ts_us, service_name, severity_text, body, span_id FROM sigiro_logs WHERE trace_id = ${id} ORDER BY timestamp LIMIT 500`
  );
  return {
    spans: Schema.decodeUnknownSync(Rows)(spans.text),
    logs: Schema.decodeUnknownSync(Rows)(logs.text),
  };
};
export type TraceData = Awaited<ReturnType<typeof fetchTrace>>;
