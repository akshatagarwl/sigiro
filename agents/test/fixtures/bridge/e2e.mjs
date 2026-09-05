import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";

process.env.PORT = "0";
process.env.GIT_SHA ??= execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim();
process.env.OTEL_SDK_DISABLED = "true";
const { url, close } = await import("./app.mjs");

try {
  for (const [query, totalCents, discountCents] of [
    ["?coupon=SAVE10", 9000, 1000],
    ["?coupon=%20save10%20", 9000, 1000],
    ["?coupon=", 10000, 0],
    ["?coupon=OTHER", 10000, 0],
    ["", 10000, 0],
  ]) {
    const response = await fetch(`${url}/checkout${query}`, { signal: AbortSignal.timeout(5000) });
    const body = await response.json();
    assert.equal(response.status, 200, `GET /checkout${query}: ${JSON.stringify(body)}`);
    assert.deepEqual(body, { totalCents, discountCents, currency: "USD" });
    console.log(`PASS GET /checkout${query}`);
  }
  for (const [path, method] of [["/missing", "GET"], ["/checkout", "POST"]]) {
    const response = await fetch(`${url}${path}`, { method, signal: AbortSignal.timeout(5000) });
    await response.text();
    assert.equal(response.status, 404, `${method} ${path}`);
    console.log(`PASS ${method} ${path}`);
  }
} finally {
  await close();
}
