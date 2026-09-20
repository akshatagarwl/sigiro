import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import test from "node:test";

import { FIRST_PARTY_CLIENT_ID, FIRST_PARTY_CLIENT_METADATA } from "../src/client-metadata.js";

/**
 * The auth Worker serves the CLI's client metadata from memory because it
 * cannot fetch its own zone (Cloudflare 1014). That makes the in-code copy a
 * second source of truth for a document the public also fetches as a static
 * asset. If the two drift, clients and the authorization server disagree about
 * the client's scopes and redirect URIs — silently, and only for some callers.
 */

const servedDocumentPath = fileURLToPath(
  new URL("../../www/public/.well-known/oauth-client.json", import.meta.url),
);

void test("the in-code client metadata matches the publicly served document", () => {
  const served: unknown = JSON.parse(readFileSync(servedDocumentPath, "utf8"));
  assert.deepEqual(
    served,
    // Strip the `as const` readonly markers for comparison.
    JSON.parse(JSON.stringify(FIRST_PARTY_CLIENT_METADATA)),
    `${servedDocumentPath} has drifted from auth/src/client-metadata.ts — update both`,
  );
});

void test("the served document is self-consistent about its own client_id", () => {
  const served = JSON.parse(readFileSync(servedDocumentPath, "utf8")) as { client_id: string };
  // A CIMD document's client_id MUST be the URL it is served from, or the
  // authorization server rejects it.
  assert.equal(served.client_id, FIRST_PARTY_CLIENT_ID);
});
