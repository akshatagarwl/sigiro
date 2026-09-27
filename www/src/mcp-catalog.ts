import { MCP_TOOLS } from "blume/ai/mcp/tools.ts";
import * as Schema from "effect/Schema";

type SchemaValue =
  | string
  | number
  | boolean
  | SchemaValue[]
  | { [key: string]: SchemaValue };
export type JsonSchema = { type: string; [key: string]: SchemaValue };
type ToolAnnotations = Readonly<Record<string, boolean>>;
type DocsEntry = {
  name: string;
  title: string;
  description: string;
  annotations: ToolAnnotations;
  inputSchema: JsonSchema;
  kind: "docs";
  scope: null;
};
type OperationScope =
  | "services:read"
  | "anomalies:read"
  | "diagnose:read"
  | "query:read";
type OperationEntry = {
  name: string;
  title: string;
  description: string;
  annotations: ToolAnnotations;
  inputSchema: JsonSchema;
  kind: "operation";
  scope: OperationScope;
  route: string;
  method: "GET" | "POST";
  runtimeSchema: Schema.Schema<any> & { readonly DecodingServices: never };
};
export type CatalogEntry = DocsEntry | OperationEntry;

const docsSchemas = {
  search_docs: {
    type: "object",
    properties: {
      contentTypes: { type: "array", items: { type: "string" } },
      filters: { type: "object", additionalProperties: { type: "string" } },
      limit: { type: "integer", minimum: 1, maximum: 20 },
      locale: { type: "string" },
      query: { type: "string" },
      version: { type: "string" },
    },
    required: ["query"],
    additionalProperties: false,
  },
  get_page: {
    type: "object",
    properties: { route: { type: "string" } },
    required: ["route"],
    additionalProperties: false,
  },
  list_pages: {
    type: "object",
    properties: {
      contentTypes: { type: "array", items: { type: "string" } },
      filters: { type: "object", additionalProperties: { type: "string" } },
      locale: { type: "string" },
      version: { type: "string" },
    },
    additionalProperties: false,
  },
  get_navigation: {
    type: "object",
    properties: { locale: { type: "string" }, version: { type: "string" } },
    additionalProperties: false,
  },
} satisfies { [key: string]: JsonSchema };
const operationRuntimeSchemas = {
  list_services: Schema.Struct({}),
  list_anomalies: Schema.Struct({
    service: Schema.optional(Schema.String),
    from_ts: Schema.optional(Schema.Int),
    to_ts: Schema.optional(Schema.Int),
  }),
  diagnose_service: Schema.Struct({
    service: Schema.String,
    from_ts: Schema.optional(Schema.Int),
    to_ts: Schema.optional(Schema.Int),
  }),
  run_query: Schema.Struct({ sql: Schema.String }),
};
const operationSchemas = {
  list_services: {
    type: "object",
    properties: {},
    additionalProperties: false,
  },
  list_anomalies: {
    type: "object",
    properties: {
      service: { type: "string" },
      from_ts: { type: "integer" },
      to_ts: { type: "integer" },
    },
    additionalProperties: false,
  },
  diagnose_service: {
    type: "object",
    properties: {
      service: { type: "string" },
      from_ts: { type: "integer" },
      to_ts: { type: "integer" },
    },
    required: ["service"],
    additionalProperties: false,
  },
  run_query: {
    type: "object",
    properties: { sql: { type: "string" } },
    required: ["sql"],
    additionalProperties: false,
  },
} satisfies { [key: string]: JsonSchema };

const docsCatalog: DocsEntry[] = MCP_TOOLS.map((tool) => ({
  ...tool,
  kind: "docs",
  inputSchema: docsSchemas[tool.name]!,
  scope: null,
}));
const operations: OperationEntry[] = [
  {
    name: "list_services",
    title: "List services",
    description:
      "List services with telemetry when you begin an investigation; returns each service and its observed time range.",
    annotations: { readOnlyHint: true, openWorldHint: false },
    inputSchema: operationSchemas.list_services!,
    runtimeSchema: operationRuntimeSchemas.list_services,
    kind: "operation",
    scope: "services:read",
    route: "/v1/services",
    method: "GET",
  },
  {
    name: "list_anomalies",
    title: "List anomalies",
    description:
      "List persisted telemetry anomalies when you need to choose an investigation; returns ranked anomalies and drill-down SQL.",
    annotations: { readOnlyHint: true, openWorldHint: false },
    inputSchema: operationSchemas.list_anomalies!,
    runtimeSchema: operationRuntimeSchemas.list_anomalies,
    kind: "operation",
    scope: "anomalies:read",
    route: "/v1/anomalies",
    method: "GET",
  },
  {
    name: "diagnose_service",
    title: "Diagnose service",
    description:
      "Diagnose one service over a time window when an incident needs root-cause analysis; returns ranked findings, anomaly flags, bounded samples, and drill-down SQL.",
    annotations: { readOnlyHint: true, openWorldHint: false },
    inputSchema: operationSchemas.diagnose_service!,
    runtimeSchema: operationRuntimeSchemas.diagnose_service,
    kind: "operation",
    scope: "diagnose:read",
    route: "/v1/diagnose",
    method: "POST",
  },
  {
    name: "run_query",
    title: "Run read-only query",
    description:
      "Run read-only SQL when you need a telemetry drill-down; returns the query rows.",
    annotations: { readOnlyHint: true, openWorldHint: false },
    inputSchema: operationSchemas.run_query!,
    runtimeSchema: operationRuntimeSchemas.run_query,
    kind: "operation",
    scope: "query:read",
    route: "/v1/query",
    method: "POST",
  },
];

export const MCP_CATALOG: readonly CatalogEntry[] = [
  ...docsCatalog,
  ...operations,
];
export const MCP_OPERATION_CATALOG: readonly OperationEntry[] = operations;
export const MCP_OPERATION_SCOPES: readonly OperationScope[] = operations.map(
  (entry) => entry.scope
);
export const MCP_RESOURCE = "https://sigiro.com" as const;
export const MCP_ISSUER = "https://sigiro.com/api/auth" as const;
export const MCP_REQUIRED_SCOPE = ["openid", ...MCP_OPERATION_SCOPES].join(" ");
export const catalogEntry = (name: string): CatalogEntry | undefined =>
  MCP_CATALOG.find((entry) => entry.name === name);
