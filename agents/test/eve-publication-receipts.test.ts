import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import worldChannel, { handleCancel, handleResult, handleStatus } from "../agent/channels/celld.ts";
import { parseReceipt, readReceipt, writeReceipt, type PublicationReceipt } from "../agent/lib/publication-receipts.ts";

const published: PublicationReceipt = { executionId: "job-1", status: "published", headSha: "a".repeat(40), sessionId: "session-1", updatedAt: new Date().toISOString(), branch: "sigiro/job-1", commitSha: "c".repeat(40), changedPaths: ["src/a.ts"], pullRequestNumber: 3, pullRequestUrl: "https://example.test/pr/3", tests: [{ name: "unit", command: "bun test", exitCode: 0 }] };
const failed: PublicationReceipt = { executionId: "job-1", status: "failed", headSha: "a".repeat(40), sessionId: "session-1", updatedAt: new Date().toISOString(), error: "Required test failed" };
let directory: string;

beforeEach(async () => { directory = await mkdtemp(join(tmpdir(), "sigiro-receipts-")); process.env.WORLD_JOB_RECEIPTS_DIR = directory; });
afterEach(async () => { delete process.env.WORLD_JOB_RECEIPTS_DIR; await rm(directory, { recursive: true, force: true }); });

describe("durable publication receipts", () => {
  test("round-trips published and failed receipts", async () => { await writeReceipt(published); expect(await readReceipt("job-1")).toEqual(published); await writeReceipt(failed); expect(await readReceipt("job-1")).toEqual(published); });
  test("replays publication without allowing a failure to retract it", async () => { await writeReceipt(published); await writeReceipt({ ...published, commitSha: "d".repeat(40) }); expect((await readReceipt("job-1"))?.commitSha).toBe("d".repeat(40)); await writeReceipt(failed); expect((await readReceipt("job-1"))?.status).toBe("published"); });
  test("parses only current receipt fields", () => { expect(parseReceipt({ ...published, tests: [] })).toBeUndefined(); expect(parseReceipt({ ...published, status: "unknown" })).toBeUndefined(); expect(parseReceipt(published)).toEqual(published); });
});

describe("direct world ingress", () => {
  test("exposes result route without a worker credential", () => { expect(worldChannel.routes.some((route) => route.path === "/internal/world/job/result")).toBe(true); });
  test("returns pending then the durable receipt", async () => {
    const request = (body: unknown) => new Request("http://example.test/internal/world/job/result", { method: "POST", body: JSON.stringify(body) });
    expect((await handleResult(request({ executionId: "missing" }))).status).toBe(404);
    await writeReceipt(published);
    expect(await (await handleResult(request({ executionId: "job-1" }))).json()).toEqual(published);
  });
  test("reports status and cancels the stable session address", async () => {
    await writeReceipt(published);
    const args = { params: { executionId: "job-1" }, resolveSession: async () => ({ id: "session-1" }), attachSession: (id: string) => ({ cancel: async () => ({ status: "accepted", turnId: id }) }) } as never;
    expect(await (await handleStatus(new Request("http://example.test"), args)).json()).toEqual({ executionId: "job-1", status: "published", sessionId: "session-1", receipt: published });
    const response = await handleCancel(new Request("http://example.test", { method: "DELETE" }), args);
    expect(response.status).toBe(202);
    expect(await response.json()).toMatchObject({ executionId: "job-1", sessionId: "session-1", status: "accepted" });
  });
});
