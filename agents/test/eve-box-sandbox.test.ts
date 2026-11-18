import { describe, expect, test } from "bun:test";
import { boxSandbox } from "../agent/lib/box-sandbox.ts";

const BOX_ID = "bx_test_1";
const BASE_URL = "https://box.test/api/box/v1";
const BASE_PATH = new URL(BASE_URL).pathname;

interface Call {
  readonly method: string;
  readonly path: string;
  readonly headers: Record<string, string>;
  readonly body: string | null;
}

/**
 * Answers the Box API v1 envelopes the adapter and the wrapper actually send.
 * Driving the real `BoxHttpClient` through this keeps the assertions about HTTP
 * shape rather than about a hand-written stand-in client.
 */
function fakeBox() {
  const calls: Call[] = [];
  const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
  const box = { id: BOX_ID, state: "ready" };

  const fetchImpl: typeof fetch = async (input, init) => {
    const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url);
    const method = init?.method ?? "GET";
    const headers: Record<string, string> = {};
    new Headers(init?.headers).forEach((value, key) => { headers[key.toLowerCase()] = value; });
    // Assert on the route below the API base so a base-URL change cannot pass silently.
    if (!url.pathname.startsWith(BASE_PATH)) throw new Error(`Box call escaped the configured API base: ${url.pathname}`);
    const path = url.pathname.slice(BASE_PATH.length);
    calls.push({ method, path, headers, body: typeof init?.body === "string" ? init.body : null });

    if (path === "/boxes" && method === "POST") return json({ ok: true, box });
    if (path === `/boxes/${BOX_ID}` && method === "PATCH") return json({ ok: true, box });
    if (path === `/boxes/${BOX_ID}` && method === "GET") return json({ ok: true, box });
    if (path === `/boxes/${BOX_ID}/commands`) return json({ ok: true, result: { exitCode: 0, stdout: "", stderr: "" } });
    if (path === `/boxes/${BOX_ID}/stop`) return json({ ok: true, box });
    if (path === `/boxes/${BOX_ID}/resume`) return json({ ok: true, box });
    if (path === `/boxes/${BOX_ID}` && method === "DELETE") return json({ ok: true, status: "accepted" });
    return new Response(JSON.stringify({ ok: false, code: "not_found" }), { status: 404 });
  };

  return { calls, fetchImpl };
}

async function createHandle(fetchImpl: typeof fetch) {
  const backend = boxSandbox({ apiKey: "test-key", baseUrl: BASE_URL, fetchImpl });
  return await backend.create({
    templateKey: null,
    sessionKey: "session-1",
    runtimeContext: { appRoot: process.cwd() },
  });
}

const lifecycleCalls = (calls: readonly Call[]) =>
  calls.filter((call) => call.path.endsWith("/stop") || call.path.endsWith("/resume") || call.method === "DELETE")
    .map((call) => `${call.method} ${call.path}`);

describe("Box sandbox backend on Eve 0.51", () => {
  /**
   * The published adapter targets Eve 0.11 and ships only `captureState` and
   * `dispose`. Eve 0.51 calls these three by name, so their absence is a runtime
   * failure that the package's own type declarations hide.
   */
  test("supplies the lifecycle methods Eve 0.51 requires", async () => {
    const { fetchImpl } = fakeBox();
    const handle = await createHandle(fetchImpl);
    for (const method of ["captureState", "delete", "stop", "shutdown"] as const) {
      expect(typeof handle[method]).toBe("function");
    }
    expect(handle.session).toBeDefined();
    expect(typeof handle.useSessionFn).toBe("function");
  });

  test("withholds the operator's Box account from every box it creates", async () => {
    const { calls, fetchImpl } = fakeBox();
    await createHandle(fetchImpl);
    const created = calls.find((call) => call.path === "/boxes" && call.method === "POST");
    expect(created).toBeDefined();
    expect(JSON.parse(created?.body ?? "{}")).toMatchObject({ noEnv: true });
  });

  test("authenticates as a bearer token", async () => {
    const { calls, fetchImpl } = fakeBox();
    await createHandle(fetchImpl);
    expect(calls.every((call) => call.headers["authorization"] === "Bearer test-key")).toBe(true);
  });

  test("stop archives the box and does not delete it", async () => {
    const { calls, fetchImpl } = fakeBox();
    const handle = await createHandle(fetchImpl);
    const before = calls.length;
    await handle.stop();
    expect(lifecycleCalls(calls.slice(before))).toEqual([`POST /boxes/${BOX_ID}/stop`]);
  });

  /** Shutdown must leave nothing running yet stay reattachable, which is a stop. */
  test("shutdown archives the box and does not delete it", async () => {
    const { calls, fetchImpl } = fakeBox();
    const handle = await createHandle(fetchImpl);
    const before = calls.length;
    await handle.shutdown();
    expect(lifecycleCalls(calls.slice(before))).toEqual([`POST /boxes/${BOX_ID}/stop`]);
  });

  /** Box refuses a delete that does not echo the box id back as confirmation. */
  test("delete sends the confirmation header Box requires", async () => {
    const { calls, fetchImpl } = fakeBox();
    const handle = await createHandle(fetchImpl);
    const before = calls.length;
    await handle.delete();
    const deleted = calls.slice(before).find((call) => call.method === "DELETE");
    expect(deleted?.path).toBe(`/boxes/${BOX_ID}`);
    expect(deleted?.headers["x-ascii-confirm-delete"]).toBe(BOX_ID);
  });

  test("raises a Box delete failure instead of reporting success", async () => {
    const { fetchImpl } = fakeBox();
    const failing: typeof fetch = async (input, init) => {
      if ((init?.method ?? "GET") === "DELETE") return new Response("{}", { status: 409 });
      return fetchImpl(input, init);
    };
    const handle = await createHandle(failing);
    await expect(handle.delete()).rejects.toThrow("Box delete failed");
  });

  test("refuses to start without a Box API key", () => {
    const configured = process.env.BOX_API_KEY;
    delete process.env.BOX_API_KEY;
    expect(() => boxSandbox()).toThrow("BOX_API_KEY is required");
    if (configured === undefined) delete process.env.BOX_API_KEY;
    else process.env.BOX_API_KEY = configured;
  });
});
