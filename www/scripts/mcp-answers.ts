/**
 * Record what Blume's own docs MCP server answers for a list of tool calls,
 * over a snapshot written by `export-mcp-data.ts`. `src/docs_mcp.rs` replays
 * the calls and must answer the same (tests/fixtures/docs-mcp/).
 *
 *   tsx scripts/mcp-answers.ts <snapshot.json> <calls.json> <answers.json>
 *
 * `calls.json` is `[[tool, arguments], ...]`. Each answer keeps the error flag
 * and the text, except that a `search_docs` answer keeps only the ranked
 * routes: the ranking is what this guards, and the hit shape is pinned by the
 * Rust unit tests.
 */
import { readFile, writeFile } from "node:fs/promises";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import type { CallToolRequest } from "@modelcontextprotocol/sdk/types.js";
import type { McpData } from "blume/ai/mcp/data.ts";
import { buildServer, createIndexProvider } from "blume/ai/mcp/server.ts";

type Navigation = McpData["navigation"];

/** What `export-mcp-data.ts` writes: McpData with the trees kept as text. */
type Snapshot = Omit<McpData, "navigation" | "navigationByLocale"> & {
  navigationJson: string;
  navigationByLocaleJson?: Record<string, string>;
};

type Call = [
  CallToolRequest["params"]["name"],
  CallToolRequest["params"]["arguments"],
];

const [snapshotPath, callsPath, out] = process.argv.slice(2);
if (!(snapshotPath && callsPath && out)) {
  throw new Error(
    "usage: tsx scripts/mcp-answers.ts <snapshot.json> <calls.json> <answers.json>"
  );
}

// SAFETY: the snapshot is the file export-mcp-data.ts wrote from Blume's McpData.
const snapshot = JSON.parse(await readFile(snapshotPath, "utf8")) as Snapshot;
// SAFETY: every value is a navigation tree export-mcp-data.ts stringified.
const parseTree = (json: string) => JSON.parse(json) as Navigation;
const data: McpData = {
  ...snapshot,
  navigation: parseTree(snapshot.navigationJson),
  navigationByLocale: snapshot.navigationByLocaleJson
    ? Object.fromEntries(
        Object.entries(snapshot.navigationByLocaleJson).map(
          ([locale, json]) => [locale, parseTree(json)]
        )
      )
    : undefined,
};

const server = buildServer(
  data,
  createIndexProvider(data.documents, data.defaultLocale)
);
const [clientSide, serverSide] = InMemoryTransport.createLinkedPair();
await server.connect(serverSide);
const client = new Client({ name: "sigiro-mcp-answers", version: "0" });
await client.connect(clientSide);

// SAFETY: calls.json is a hand-written list of [tool name, arguments] pairs.
const calls = JSON.parse(await readFile(callsPath, "utf8")) as Call[];
const answers = [];
for (const [name, args] of calls) {
  // oxlint-disable-next-line no-await-in-loop -- one call at a time keeps the answers in call order.
  const result = await client.callTool({ arguments: args, name });
  const content = Array.isArray(result.content) ? result.content : [];
  const first = content.find((block) => block.type === "text");
  const text = first?.type === "text" ? first.text : "";
  const isError = result.isError === true;
  if (name === "search_docs" && !isError) {
    // SAFETY: Blume's search_docs answers with JSON text of its hit list.
    const hits = JSON.parse(text) as { route: string }[];
    answers.push({ isError, routes: hits.map((hit) => hit.route) });
  } else {
    answers.push({ isError, text });
  }
}
await client.close();
await writeFile(out, `${JSON.stringify(answers, null, 1)}\n`);
console.log(`mcp-answers: ${answers.length} answers -> ${out}`);
