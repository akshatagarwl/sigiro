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
type OperationScope = "services:read" | "query:read";
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
  run_query: Schema.Struct({ sql: Schema.String }),
};
const operationSchemas = {
  list_services: {
    type: "object",
    properties: {},
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
