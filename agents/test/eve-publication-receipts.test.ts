import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { mkdtemp, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import celldChannel, { handleResult } from "../agent/channels/celld.ts";
import {
  DEFAULT_RECEIPTS_DIRNAME,
  parseReceipt,
  readReceipt,
  receiptsDirectory,
  writeReceipt,
  type PublicationReceipt,
} from "../agent/lib/publication-receipts.ts";
import { MAX_BODY_BYTES } from "../agent/lib/celld-dispatch.ts";

process.env.EVE_WORKER_TOKEN = "secret-token";

const published: PublicationReceipt = {
  executionId: "job-1:1:1",
  status: "published",
  branch: "sigiro/eve-abc",
  commitSha: "c".repeat(40),
  changedPaths: ["src/a.ts"],
  pullRequestNumber: 3,
  pullRequestUrl: "https://example.test/pr/3",
  headSha: "a".repeat(40),
  sessionId: "session-1",
  updatedAt: "2026-09-04T00:00:00.000Z",
};

const failed: PublicationReceipt = {
  executionId: "job-1:1:1",
  status: "failed",
  error: "Required test failed: unit (exit 1)",
  headSha: "a".repeat(40),
  sessionId: "session-1",
  updatedAt: "2026-09-04T00:01:00.000Z",
};

describe("Publication receipt store", () => {
  let directory: string;

  beforeEach(async () => {
    directory = await mkdtemp(join(tmpdir(), "sigiro-receipts-"));
  });

  afterEach(async () => {
    await rm(directory, { recursive: true, force: true });
  });

  test("round-trips a published receipt through real files", async () => {
    await writeReceipt(published, directory);
    expect(await readReceipt(published.executionId, directory)).toEqual(published);
  });

  test("round-trips a failed receipt", async () => {
    await writeReceipt(failed, directory);
    expect(await readReceipt(failed.executionId, directory)).toEqual(failed);
  });

  test("reads a missing receipt as undefined", async () => {
    expect(await readReceipt("never-written", directory)).toBeUndefined();
  });

  /**
   * A published receipt is evidence that the branch, the commit, and the PR
   * exist on GitHub. Nothing later in this process may retract that.
   */
  test("refuses to overwrite a published receipt with a failure", async () => {
    await writeReceipt(published, directory);
    await writeReceipt(failed, directory);
    expect(await readReceipt(published.executionId, directory)).toEqual(published);
  });

  test("lets a replay overwrite published with published", async () => {
    await writeReceipt(published, directory);
    const replayed: PublicationReceipt = { ...published, commitSha: "d".repeat(40), updatedAt: "2026-09-04T00:02:00.000Z" };
    await writeReceipt(replayed, directory);
    expect(await readReceipt(published.executionId, directory)).toEqual(replayed);
  });

  test("lets a failure be replaced by a later publication or another failure", async () => {
    await writeReceipt(failed, directory);
    const second: PublicationReceipt = { ...failed, error: "Celld publication produced no changes" };
    await writeReceipt(second, directory);
    expect(await readReceipt(failed.executionId, directory)).toEqual(second);
    await writeReceipt(published, directory);
    expect(await readReceipt(failed.executionId, directory)).toEqual(published);
  });

  /**
   * The write is temp-plus-rename, so a reader never sees a partial file and no
   * temporary is left behind once it has landed.
   */
  test("leaves exactly one file per execution and no temporaries", async () => {
    await writeReceipt(published, directory);
    await writeReceipt({ ...published, executionId: "job-2:1:1" }, directory);
    const entries = await readdir(directory);
    expect(entries).toHaveLength(2);
    expect(entries.every((entry) => entry.endsWith(".json"))).toBe(true);
  });

  /** An execution id is not a safe file name, so the record is addressed by hash. */
  test("does not let an execution id escape the receipts directory", async () => {
    await writeReceipt({ ...published, executionId: "../../escaped" }, directory);
    expect(await readdir(directory)).toHaveLength(1);
    expect(await readReceipt("../../escaped", directory)).toEqual({ ...published, executionId: "../../escaped" });
  });

  test("reads a truncated or hand-edited file as undefined", async () => {
    await writeReceipt(published, directory);
    const [entry] = await readdir(directory);
    if (entry === undefined) throw new Error("fixture missing");
    await writeFile(join(directory, entry), '{"executionId":"job-1:1:1","stat');
    expect(await readReceipt(published.executionId, directory)).toBeUndefined();
    await writeFile(join(directory, entry), JSON.stringify({ ...published, status: "maybe" }));
    expect(await readReceipt(published.executionId, directory)).toBeUndefined();
  });

  test("validates the shape it parses", () => {
    expect(parseReceipt(null)).toBeUndefined();
    expect(parseReceipt([published])).toBeUndefined();
    expect(parseReceipt({ ...published, executionId: "" })).toBeUndefined();
    expect(parseReceipt({ ...published, headSha: 7 })).toBeUndefined();
    expect(parseReceipt({ ...published, sessionId: undefined })).toBeUndefined();
    expect(parseReceipt({ ...published, updatedAt: 1 })).toBeUndefined();
    expect(parseReceipt(published)).toEqual(published);
    // Wrong-typed optionals are dropped rather than carried through untyped.
    expect(parseReceipt({ ...failed, branch: 3, changedPaths: [1], pullRequestNumber: 1.5 })).toEqual(failed);
  });

  test("defaults the directory to the app working directory and honours the override", () => {
    const configured = process.env.CELLD_RECEIPTS_DIR;
    delete process.env.CELLD_RECEIPTS_DIR;
    expect(receiptsDirectory()).toBe(join(process.cwd(), DEFAULT_RECEIPTS_DIRNAME));
    process.env.CELLD_RECEIPTS_DIR = "/tmp/elsewhere";
    expect(receiptsDirectory()).toBe("/tmp/elsewhere");
    if (configured === undefined) delete process.env.CELLD_RECEIPTS_DIR;
    else process.env.CELLD_RECEIPTS_DIR = configured;
  });
});

const resultRequest = (body: unknown, token = "secret-token", headers: Record<string, string> = {}) =>
  new Request("http://example.test/internal/celld/job/result", {
    method: "POST",
    headers: { authorization: `Bearer ${token}`, ...headers },
    body: typeof body === "string" ? body : JSON.stringify(body),
  });

describe("Celld result route", () => {
  let directory: string;
  let configured: string | undefined;

  beforeEach(async () => {
    directory = await mkdtemp(join(tmpdir(), "sigiro-result-route-"));
    configured = process.env.CELLD_RECEIPTS_DIR;
    process.env.CELLD_RECEIPTS_DIR = directory;
  });

  afterEach(async () => {
    if (configured === undefined) delete process.env.CELLD_RECEIPTS_DIR;
    else process.env.CELLD_RECEIPTS_DIR = configured;
    await rm(directory, { recursive: true, force: true });
  });

  test("is registered on the celld channel as a POST route", () => {
    const route = celldChannel.routes.find((entry) => entry.path === "/internal/celld/job/result");
    expect(route).toBeDefined();
    expect(route?.method).toBe("POST");
  });

  test("returns 404 pending while no receipt exists", async () => {
    const response = await handleResult(resultRequest({ executionId: "job-1:1:1" }));
    expect(response.status).toBe(404);
    expect(await response.json() as unknown).toEqual({ status: "pending" });
  });

  test("returns 200 with the receipt once it exists", async () => {
    await writeReceipt(published, directory);
    const response = await handleResult(resultRequest({ executionId: published.executionId }));
    expect(response.status).toBe(200);
    expect(await response.json() as unknown).toEqual(published);
  });

  test("returns a failed receipt with its recorded error", async () => {
    await writeReceipt(failed, directory);
    const response = await handleResult(resultRequest({ executionId: failed.executionId }));
    expect(response.status).toBe(200);
    expect(await response.json() as unknown).toEqual(failed);
  });

  test("rejects a wrong or missing bearer token before reading anything", async () => {
    await writeReceipt(published, directory);
    for (const request of [
      resultRequest({ executionId: published.executionId }, "wrong-token"),
      new Request("http://example.test/internal/celld/job/result", { method: "POST", body: "{}" }),
    ]) {
      const response = await handleResult(request);
      expect(response.status).toBe(401);
      expect(await response.text()).toBe('{"error":"unauthorized"}');
    }
  });

  test("bounds the body and the execution id", async () => {
    const oversizedHeader = await handleResult(
      resultRequest({ executionId: "job-1:1:1" }, "secret-token", { "content-length": String(MAX_BODY_BYTES + 1) }),
    );
    expect(oversizedHeader.status).toBe(400);

    for (const body of [
      { executionId: "" },
      { executionId: "x".repeat(513) },
      { executionId: 7 },
      { executionId: "job-1:1:1", extra: "field" },
      {},
      "not json",
    ]) {
      expect((await handleResult(resultRequest(body))).status).toBe(400);
    }
  });

  test("does not answer with a receipt for a different execution", async () => {
    await writeReceipt(published, directory);
    const response = await handleResult(resultRequest({ executionId: "job-9:1:1" }));
    expect(response.status).toBe(404);
  });
});
