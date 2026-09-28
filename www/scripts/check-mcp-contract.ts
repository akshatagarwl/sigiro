import { readFile } from "node:fs/promises";

import * as Schema from "effect/Schema";

import {
  MCP_CATALOG,
  MCP_OPERATION_SCOPES,
  MCP_REQUIRED_SCOPE,
  MCP_RESOURCE,
  MCP_ISSUER,
} from "../src/mcp-catalog";
import { MCP_PROTECTED_RESOURCE_METADATA } from "../src/mcp-handler";

const rust = await readFile(
  new URL("../../src/mcp.rs", import.meta.url),
  "utf8"
);
const handler = await readFile(
  new URL("../src/mcp-handler.ts", import.meta.url),
  "utf8"
);
const worker = await readFile(
  new URL("../src/worker.ts", import.meta.url),
  "utf8"
);
const hostedSource = `${worker}\n${handler}`;

const rustTools = [
  ...rust.matchAll(
    /async fn (list_services|run_query)\s*\(/g
  ),
].map(([, name]) => name);
const hostedTools = MCP_CATALOG.filter(
  (entry) => entry.kind === "operation"
).map(({ name }) => name);
const expected = ["list_services", "run_query"];

if (JSON.stringify(rustTools) !== JSON.stringify(expected)) {
  throw new Error(
    `Rust MCP catalog changed: ${JSON.stringify(rustTools)} !== ${JSON.stringify(expected)}`
  );
}
if (JSON.stringify(hostedTools) !== JSON.stringify(expected)) {
  throw new Error(
    `Hosted MCP catalog drifted: ${JSON.stringify(hostedTools)} !== ${JSON.stringify(expected)}`
  );
}
for (const field of ["service", "sql"]) {
  if (!rust.includes(field) || !hostedSource.includes(field)) {
    throw new Error(`MCP catalog is missing shared field ${field}`);
  }
}
if (/from ['"]zod['"]|from ['"]zod\//.test(hostedSource)) {
  throw new Error("Hosted MCP adapter must not add a direct Zod dependency");
}
if (
  (hostedSource.match(/new Request\(["']https:\/\/sigiro\.com\/mcp\/docs["']/g) ?? [])
    .length !== 1
) {
  throw new Error(
    "All documentation tools must use the single Blume MCP handler"
  );
}
if (
  /headers\.set\(['"]authorization['"]/.test(
    hostedSource.match(
      /new Request\(["']https:\/\/sigiro\.com\/mcp\/docs["'][\s\S]*?\);/
    )?.[0] ?? ""
  )
) {
  throw new Error("Documentation calls must not forward Authorization");
}
if (
  /cookie/i.test(
    hostedSource.match(
      /new Request\(["']https:\/\/sigiro\.com\/mcp\/docs["'][\s\S]*?\);/
    )?.[0] ?? ""
  )
) {
  throw new Error("Documentation calls must not forward Cookie");
}
for (const entry of MCP_CATALOG.filter(
  (candidate) => candidate.kind === "operation"
)) {
  if (
    !hostedSource.includes("entry.route") ||
    !entry.route ||
    !entry.method ||
    !entry.scope
  )
    throw new Error(`Hosted operation mapping is missing ${entry.name}`);
}
const validInputs = {
  list_services: {},
  run_query: { sql: "select 1" },
};
for (const entry of MCP_CATALOG.filter(
  (candidate) => candidate.kind === "operation"
)) {
  Schema.decodeUnknownSync(entry.runtimeSchema)(
    // SAFETY: every operation catalog name is represented in validInputs above.
    validInputs[entry.name as keyof typeof validInputs]
  );
  const invalid: null | { unexpected?: boolean; sql?: unknown } =
    entry.name === "list_services" ? null : { unexpected: true };
  let rejected = false;
  if (invalid === null) {
    try {
      Schema.decodeUnknownSync(entry.runtimeSchema)(invalid);
    } catch {
      rejected = true;
    }
  }
  if (invalid !== null && entry.name === "run_query") invalid.sql = 42;
  if (invalid !== null) {
    try {
      Schema.decodeUnknownSync(entry.runtimeSchema)(invalid);
    } catch {
      rejected = true;
    }
  }
  if (!rejected)
    throw new Error(`Runtime schema accepted invalid input for ${entry.name}`);
}
if (
  JSON.stringify(MCP_PROTECTED_RESOURCE_METADATA.scopes_supported) !==
  JSON.stringify(["openid", ...MCP_OPERATION_SCOPES])
)
  throw new Error(
    "Protected-resource scopes are not derived from the operation catalog"
  );
for (const marker of [
  "status: 401",
  "www-authenticate",
  'resource_metadata="https://sigiro.com/.well-known/oauth-protected-resource"',
  'pathname === "/mcp"',
  'pathname === "/.well-known/mcp.json"',
  'const MCP_METADATA = "/.well-known/oauth-protected-resource"',
  "https://sigiro.com/mcp/docs",
]) {
  if (!hostedSource.includes(marker) && !worker.includes(marker)) {
    throw new Error(`Hosted MCP boundary is missing ${marker}`);
  }
}
if (
  MCP_RESOURCE !== "https://sigiro.com" ||
  MCP_ISSUER !== "https://sigiro.com/api/auth"
)
  throw new Error("MCP OAuth canonical endpoints drifted");
if (MCP_REQUIRED_SCOPE !== ["openid", ...MCP_OPERATION_SCOPES].join(" "))
  throw new Error("MCP OAuth scope catalog drifted");
console.log(`MCP catalog contract verified: ${expected.join(", ")}`);
