/**
 * Write the docs MCP snapshot the sigiro binary answers from when the hosted
 * docs MCP (sigiro.com/mcp/docs) can't be reached (`src/docs_mcp.rs`).
 *
 *   tsx scripts/export-mcp-data.ts <out.json>
 *
 * It is the same `McpData` the hosted endpoint serves, built from the
 * sigiro.com config (run WITHOUT `SIGIRO_DOCS_TARGET=binary`), so routes and
 * URLs match the hosted answers byte for byte. Three additions make the
 * snapshot self-describing for a reader that is not Blume:
 *
 * - `tools` and `server`: the `tools/list` and `initialize` answers of Blume's
 *   own MCP server over this data, so the binary advertises exactly the hosted
 *   names, titles, descriptions and input schemas.
 * - `navigationJson`, `navigationByLocaleJson`, `navigationByVersionJson`: the
 *   navigation trees as the text `get_navigation` returns, so key order is
 *   Blume's, not whatever a Rust map does with it.
 *
 * WHY THIS IS A SCRIPT AND NOT A BUILD OUTPUT. Blume builds this snapshot on
 * every server build, but only into an in-memory virtual module
 * (`blume:mcp-data`); nothing public writes it to disk except `blume eval`,
 * into a temp dir. `scanProject` and `buildMcpData` are documented deep
 * imports (blume 2.2.5 changelog), and `buildServer` is the server Blume's
 * own `mcp-stdio` command runs, so this uses Blume as published rather than
 * reimplementing it. The upstream ask is in the commit that added this file.
 */
import { writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { buildMcpData } from "blume/ai/mcp/data.ts";
import type { McpData } from "blume/ai/mcp/data.ts";
import { buildServer, createIndexProvider } from "blume/ai/mcp/server.ts";
import { scanProject } from "blume/core/project-graph.ts";

const out = process.argv[2];
if (!out) {
  throw new Error("usage: tsx scripts/export-mcp-data.ts <out.json>");
}
if (process.env.SIGIRO_DOCS_TARGET) {
  throw new Error(
    "export-mcp-data: unset SIGIRO_DOCS_TARGET; the snapshot must match sigiro.com"
  );
}

const root = fileURLToPath(new URL("..", import.meta.url));
const project = await scanProject(root, { mode: "build" });
const data = await buildMcpData(project);

const server = buildServer(
  data,
  createIndexProvider(data.documents, data.defaultLocale)
);
const [clientSide, serverSide] = InMemoryTransport.createLinkedPair();
await server.connect(serverSide);
const client = new Client({ name: "sigiro-export", version: "0" });
await client.connect(clientSide);
const { tools } = await client.listTools();
const serverInfo = client.getServerVersion();
const instructions = client.getInstructions();
await client.close();

const pretty = (navigation: McpData["navigation"]): string =>
  JSON.stringify(navigation, null, 2);
const mapValues = <T, U>(
  record: Record<string, T> | undefined,
  map: (value: T) => U
): Record<string, U> | undefined =>
  record
    ? Object.fromEntries(Object.entries(record).map(([k, v]) => [k, map(v)]))
    : undefined;

const { navigation, navigationByLocale, navigationByVersion, ...rest } = data;
const snapshot = {
  ...rest,
  navigationJson: pretty(navigation),
  navigationByLocaleJson: mapValues(navigationByLocale, pretty),
  navigationByVersionJson: mapValues(navigationByVersion, (byLocale) =>
    mapValues(byLocale, pretty)
  ),
  server: { ...serverInfo, instructions },
  tools,
};
await writeFile(out, JSON.stringify(snapshot));
console.log(
  `export-mcp-data: ${data.routes.length} routes, ${data.documents.length} documents, ${tools.length} tools -> ${out}`
);
