import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { once } from "node:events";
import { createServer } from "node:http";
import { mkdir, mkdtemp, readFile, readdir, symlink, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

// Real E2E only. Provision isolated Sigiro, celld, Eve, and its worker first.
// The tracked fixtures/bridge checkout exposes app.mjs's real HTTP server and
// x-e2e-event-time header.
// Required: SIGIRO_E2E_CONFIG, SIGIRO_E2E_FIXTURE_DIR, SIGIRO_E2E_OUTPUT_DIR,
// SIGIRO_URL, SIGIRO_E2E_OTLP_URL, COORDINATOR_URL, BRIDGE_STATE_DIR,
// SIGIRO_EVIDENCE_DIR, and EVE_WORKER_TOKEN (or EVE_WORKER_TOKEN_FILE).
// Run `bun test/bridge-e2e.ts traffic`, `verify`, then `negative`.
// No detector, diagnosis, coordinator, model, or GitHub API is mocked.
const agents = dirname(dirname(fileURLToPath(import.meta.url)));
const required = (name: string): string => {
  const value = process.env[name];
  assert.ok(value, `${name} is required`);
  return value;
};
const local = (name: string): string => {
  const url = new URL(required(name));
  assert.ok(["127.0.0.1", "localhost", "[::1]"].includes(url.hostname), `${name} must target isolated loopback services`);
  return url.origin;
};
const configPath = resolve(required("SIGIRO_E2E_CONFIG"));
const fixture = resolve(required("SIGIRO_E2E_FIXTURE_DIR"));
const output = resolve(required("SIGIRO_E2E_OUTPUT_DIR"));
const state = resolve(required("BRIDGE_STATE_DIR"));
const sigiro = local("SIGIRO_URL");
const coordinator = local("COORDINATOR_URL");
const config = JSON.parse(await readFile(configPath, "utf8"));
const sha256 = (bytes: string | Uint8Array) => createHash("sha256").update(bytes).digest("hex");
await mkdir(output, { recursive: true, mode: 0o700 });
for (const name of ["SIGIRO_API_KEY", "EVE_WORKER_TOKEN"]) {
  const path = process.env[`${name}_FILE`];
  if (path !== undefined) process.env[name] = (await readFile(path, "utf8")).trim();
  assert.ok(process.env[name], `${name} must not be empty`);
}

async function run(args: string[], cwd: string, name: string, env: Record<string, string> = {}) {
  const child = Bun.spawn(args, { cwd, env: { ...process.env, ...env }, stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  await writeFile(join(output, `${name}.log`), `${stdout}\n${stderr}`, { mode: 0o600 });
  return { stdout, stderr, code };
}

async function request(path: string, init: RequestInit = {}, origin = sigiro): Promise<any> {
  const response = await fetch(`${origin}${path}`, {
    ...init, redirect: "error", signal: AbortSignal.timeout(60_000),
    headers: { authorization: `Bearer ${origin === coordinator ? process.env.EVE_WORKER_TOKEN : process.env.SIGIRO_API_KEY ?? ""}`, ...init.headers },
  });
  assert.ok(response.ok, `${path} returned HTTP ${response.status}: ${(await response.clone().text()).slice(0, 400)}`);
  return response.json();
}

async function records(): Promise<any[]> {
  const root = join(state, "records");
  const names = await readdir(root);
  return Promise.all(names.filter(name => name.endsWith(".json")).map(async name => JSON.parse(await readFile(join(root, name), "utf8"))));
}

if (process.argv[2] === "traffic") {
  const base = await run(["git", "rev-parse", "HEAD"], fixture, "fixture-base");
  assert.equal(base.code, 0);
  const baseline = await run(["npm", "test"], fixture, "baseline-http-e2e");
  assert.notEqual(baseline.code, 0, "the unpatched fixture must fail its actual HTTP E2E");
  const script = `
    import assert from 'node:assert/strict';
    const { url, close } = await import(${JSON.stringify(pathToFileURL(join(fixture, "app.mjs")).href)});
    const bucket = 300000, end = Math.floor(Date.now() / bucket) * bucket;
    let successful = 0, errors = 0;
    try {
      for (let i = 0; i < 60; i++) for (let n = 0; n < 5; n++) {
        const response = await fetch(url + '/checkout' + (i < 40 ? '?coupon=SAVE10' : ''), {
          headers: { 'x-e2e-event-time': String(end - (60 - i) * bucket + 1000 + n * 100) },
          signal: AbortSignal.timeout(5000),
        });
        await response.text();
        assert.equal(response.status, i < 40 ? 200 : 500);
        if (response.ok) successful++; else errors++;
      }
    } finally { await close(); }
    console.log(JSON.stringify({ successful, errors, buckets: 60, from_ts: (end - 60 * bucket) * 1000, to_ts: end * 1000 }));
  `;
  const traffic = await run(["node", "--input-type=module", "--eval", script], fixture, "traffic", {
    PORT: "0", GIT_SHA: base.stdout.trim(), OTEL_EXPORTER_OTLP_ENDPOINT: local("SIGIRO_E2E_OTLP_URL"),
    OTEL_SDK_DISABLED: "false",
  });
  assert.equal(traffic.code, 0, `real fixture traffic failed; see ${join(output, "traffic.log")}`);
  console.log("Exported 300 real checkout HTTP spans over 60 compressed historical buckets; waiting for Sigiro's scheduled pass is required.");
} else if (process.argv[2] === "verify") {
  const deadline = Date.now() + 660_000;
  let anomalySnapshot;
  while (Date.now() < deadline) {
    anomalySnapshot = await request(`/v1/anomalies?service=${encodeURIComponent(config.service)}`);
    assert.equal(anomalySnapshot.truncated, false, "use current Sigiro source; incomplete anomaly snapshots must not be tested as complete");
    if (anomalySnapshot.anomalies.some((row: any) => row.signal === "error_rate")) break;
    console.log("Waiting for the real scheduled anomaly pass.");
    await Bun.sleep(30_000);
  }
  assert.ok(anomalySnapshot?.anomalies.some((row: any) => row.signal === "error_rate"), "real scheduled detector did not find the error-rate shift");
  await writeFile(join(output, "anomalies.json"), `${JSON.stringify(anomalySnapshot, null, 2)}\n`);

  // Fault injection at the transport boundary: forward every request to REAL
  // celld, then lose its first successful create response. All retry bodies
  // must be identical, including evidence digest and pinned repository SHA.
  const submittedBodies: string[] = [];
  let droppedResponse = false;
  const proxy = createServer(async (incoming, outgoing) => {
    try {
      const chunks = [];
      for await (const chunk of incoming) chunks.push(Buffer.from(chunk));
      const body = Buffer.concat(chunks);
      const headers = new Headers();
      for (const [name, value] of Object.entries(incoming.headers)) {
        if (value !== undefined && !["host", "connection", "content-length"].includes(name)) headers.set(name, Array.isArray(value) ? value.join(", ") : value);
      }
      const response = await fetch(`${coordinator}${incoming.url}`, {
        method: incoming.method, headers, redirect: "error", signal: AbortSignal.timeout(30_000),
        ...(body.length === 0 ? {} : { body }),
      });
      const bytes = Buffer.from(await response.arrayBuffer());
      if (incoming.method === "POST" && incoming.url === "/v1/jobs") {
        submittedBodies.push(body.toString("utf8"));
        if (response.ok && !droppedResponse) { droppedResponse = true; outgoing.destroy(); return; }
      }
      outgoing.writeHead(response.status, { "content-type": response.headers.get("content-type") ?? "application/json" });
      outgoing.end(bytes);
    } catch (error) {
      console.error("E2E transport failed", error);
      outgoing.destroy(error instanceof Error ? error : undefined);
    }
  });
  proxy.listen(0, "127.0.0.1");
  await once(proxy, "listening");
  const address = proxy.address();
  assert.ok(address && typeof address !== "string");
  try {
    const first = await Promise.all([1, 2].map(index => run(
      ["bun", "run", "bridge", "--config", configPath], agents, `bridge-first-${index}`,
      { COORDINATOR_URL: `http://127.0.0.1:${address.port}` },
    )));
    assert.ok(first.some(result => result.code === 0), `both bridge processes failed; see ${output}/bridge-first-*.log`);
  } finally {
    const closed = new Promise<void>((resolve, reject) => proxy.close(error => error ? reject(error) : resolve()));
    proxy.closeAllConnections();
    await closed;
  }
  assert.ok(droppedResponse, "fault injection must lose a real successful celld response");
  assert.ok(submittedBodies.length >= 2, "bridge must retry the interrupted submission");
  assert.equal(new Set(submittedBodies.map(sha256)).size, 1, "submission retries changed immutable input");
  const stored = await records();
  assert.equal(stored.length, 1, "one incident must produce one durable candidate");
  const record = stored[0];
  assert.equal(record.status, "submitted");
  const frozen = JSON.stringify(record.job);
  const evidenceBytes = await readFile(new URL(record.job.evidenceUri));
  assert.equal(sha256(evidenceBytes), record.job.evidenceSha256);
  const evidence = JSON.parse(evidenceBytes.toString("utf8"));
  assert.equal(evidence.service, config.service);
  assert.equal(evidence.diagnosis.service, config.service);
  assert.equal(evidence.diagnosis.window.from_ts, record.fromTs);
  assert.equal(evidence.diagnosis.window.to_ts, record.toTs);
  assert.ok(evidence.diagnosis.error_traces_total > 0, "real diagnosis must contain failed checkout requests");
  const repeats = await Promise.all([1, 2].map(index => run(["bun", "run", "bridge", "--config", configPath], agents, `bridge-concurrent-${index}`)));
  assert.ok(repeats.some(result => result.code === 0));
  assert.equal((await records()).length, 1);
  assert.equal(JSON.stringify((await records())[0].job), frozen);

  let job;
  const executionDeadline = Date.now() + config.policy.deadlineSeconds * 1000 + 90_000;
  while (Date.now() < executionDeadline) {
    job = await request(`/v1/jobs/${encodeURIComponent(record.job.jobId)}`, {}, coordinator);
    if (["succeeded", "failed", "no_fix", "needs_review", "cancelled"].includes(job.status)) break;
    console.log(`Waiting for real worker/Eve: ${job.status}, attempt ${job.attempt}.`);
    await Bun.sleep(15_000);
  }
  assert.equal(job?.status, "succeeded", `worker did not publish: ${JSON.stringify(job?.result ?? job?.lastError)}`);
  const prNumber = Number(new URL(job.result.pullRequestUrl).pathname.split("/").at(-1));
  const github = await run(["gh", "api", `repos/${config.repository}/pulls/${prNumber}`], agents, "github-pr");
  assert.equal(github.code, 0);
  const pr = JSON.parse(github.stdout);
  assert.equal(pr.draft, true);
  assert.equal(pr.state, "open");
  assert.equal(pr.base.sha, record.job.baseSha);
  const filesResult = await run(["gh", "api", `repos/${config.repository}/pulls/${prNumber}/files`], agents, "github-files");
  assert.equal(filesResult.code, 0);
  assert.deepEqual(JSON.parse(filesResult.stdout).map((file: any) => file.filename), ["app.mjs"]);
  const verification = await mkdtemp(join(output, "published-checkout-"));
  const clone = await run(["gh", "repo", "clone", config.repository, verification], agents, "published-clone");
  assert.equal(clone.code, 0);
  const checkout = await run(["git", "checkout", "--detach", pr.head.sha], verification, "published-checkout");
  assert.equal(checkout.code, 0);
  await symlink(join(fixture, "node_modules"), join(verification, "node_modules"), "dir");
  const fixed = await run(["npm", "test"], verification, "published-http-e2e");
  assert.equal(fixed.code, 0, "actual HTTP E2E must pass from the published commit");
  const held = await run(["bun", "run", "bridge", "--config", configPath], agents, "bridge-open-pr");
  assert.equal(held.code, 0);
  assert.ok(held.stdout.includes("draft_pr_open"), "open draft PR must hold the repository reservation");
  assert.equal((await records()).length, 1);
  const proof = { status: "passed", jobId: record.job.jobId, baseSha: record.job.baseSha, commitSha: pr.head.sha,
    pullRequestUrl: pr.html_url, evidenceSha256: record.job.evidenceSha256,
    checks: ["real HTTP traffic and OTLP ingestion", "real scheduled anomaly detection", "real diagnosis snapshot", "lost celld response replay with identical job input", "concurrent replay deduplication", "real worker and Eve draft PR", "published commit passes actual HTTP E2E", "open PR blocks another repair"] };
  await writeFile(join(output, "verification.json"), `${JSON.stringify(proof, null, 2)}\n`);
  console.log(JSON.stringify(proof, null, 2));
} else if (process.argv[2] === "negative") {
  const [primary] = await records();
  assert.equal(primary?.status, "submitted", "complete the primary E2E first");
  const originalEvidence = await readFile(new URL(primary.job.evidenceUri));
  const checks: string[] = [];
  const rejectedEvidence = await mkdtemp(join(output, "rejected-evidence-"));
  const copyReady = async (name: string, evidenceUri = primary.job.evidenceUri) => {
    const root = await mkdtemp(join(output, `${name}-`));
    await mkdir(join(root, "records"));
    const record = { ...primary, status: "ready", job: { ...primary.job, evidenceUri } };
    const path = join(root, "records", `${record.candidateId}.json`);
    await writeFile(path, `${JSON.stringify(record)}\n`, { mode: 0o600 });
    return { root, path, record };
  };
  for (const [name, bytes] of [
    ["tampered-evidence", Buffer.from("{}\n")],
    ["oversized-evidence", Buffer.alloc(1024 * 1024 + 1, 120)],
    ["missing-evidence", undefined],
  ] as const) {
    const evidencePath = join(rejectedEvidence, `${name}.json`);
    if (bytes !== undefined) await writeFile(evidencePath, bytes, { mode: 0o600 });
    const copy = await copyReady(name, pathToFileURL(evidencePath).href);
    const result = await run(["bun", "run", "bridge", "--config", configPath], agents, name, {
      BRIDGE_STATE_DIR: copy.root, SIGIRO_EVIDENCE_DIR: rejectedEvidence,
    });
    assert.notEqual(result.code, 0, `${name} must fail loudly`);
    const after = JSON.parse(await readFile(copy.path, "utf8"));
    assert.notEqual(after.status, "submitted");
    assert.deepEqual(after.job, copy.record.job);
    assert.ok(after.lastError.includes("evidence"));
    checks.push(`${name} rejected before submission`);
  }
  const unavailable = await copyReady("coordinator-unavailable");
  const refused = await run(["bun", "run", "bridge", "--config", configPath], agents, "coordinator-unavailable", {
    BRIDGE_STATE_DIR: unavailable.root, COORDINATOR_URL: "http://127.0.0.1:1",
  });
  assert.notEqual(refused.code, 0);
  const pending = JSON.parse(await readFile(unavailable.path, "utf8"));
  assert.equal(pending.status, "ready");
  assert.deepEqual(pending.job, primary.job);
  checks.push("unavailable coordinator preserves the frozen ready job");

  // Kill a real bridge process after its persisted ready job reaches POST but
  // before any bytes reach celld. Resume with the ORIGINAL already-created job
  // identity, so this recovery test cannot create another repair/PR.
  const crash = await copyReady("crash-before-submit");
  let reached!: () => void;
  const attempted = new Promise<void>(resolve => { reached = resolve; });
  let observedBody = "";
  const gate = createServer(async (incoming, outgoing) => {
    const chunks = [];
    for await (const chunk of incoming) chunks.push(Buffer.from(chunk));
    observedBody = Buffer.concat(chunks).toString("utf8");
    reached();
    outgoing.on("error", error => console.error("E2E stopped request", error.message));
  });
  gate.listen(0, "127.0.0.1");
  await once(gate, "listening");
  const address = gate.address();
  assert.ok(address && typeof address !== "string");
  const child = Bun.spawn(["bun", "bridge/main.ts", "--config", configPath], {
    cwd: agents, env: { ...process.env, BRIDGE_STATE_DIR: crash.root, COORDINATOR_URL: `http://127.0.0.1:${address.port}` },
    stdout: "pipe", stderr: "pipe",
  });
  const captured = Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text()]);
  try {
    await Promise.race([attempted, Bun.sleep(30_000).then(() => { throw new Error("bridge did not reach the pre-submit crash boundary"); })]);
    assert.deepEqual(JSON.parse(observedBody), primary.job);
  } finally {
    child.kill("SIGKILL");
    await child.exited;
    const closed = new Promise<void>((resolve, reject) => gate.close(error => error ? reject(error) : resolve()));
    gate.closeAllConnections();
    await closed;
    await writeFile(join(output, "crash-before-submit.log"), (await captured).join("\n"), { mode: 0o600 });
  }
  assert.equal(JSON.parse(await readFile(crash.path, "utf8")).status, "ready");
  const resumed = await run(["bun", "run", "bridge", "--config", configPath], agents, "crash-resume", { BRIDGE_STATE_DIR: crash.root });
  assert.equal(resumed.code, 0);
  const afterCrash = JSON.parse(await readFile(crash.path, "utf8"));
  assert.equal(afterCrash.status, "submitted");
  assert.deepEqual(afterCrash.job, primary.job);
  checks.push("SIGKILL releases ownership and restart submits the exact frozen job");

  for (const [path, init] of [
    ["/v1/jobs", { method: "POST", headers: { "content-type": "application/json" }, body: "{}" }],
    [`/v1/jobs/${encodeURIComponent(primary.job.jobId)}`, {}],
  ] as const) {
    const response = await fetch(`${coordinator}${path}`, { ...init, signal: AbortSignal.timeout(5000) });
    await response.text();
    assert.equal(response.status, 401);
  }
  checks.push("real coordinator rejects unauthenticated creation and status reads");
  assert.deepEqual(await readFile(new URL(primary.job.evidenceUri)), originalEvidence);
  assert.deepEqual((await records())[0].job, primary.job);
  const job = await request(`/v1/jobs/${encodeURIComponent(primary.job.jobId)}`, {}, coordinator);
  assert.equal(job.status, "succeeded");
  assert.equal(job.attempt, 1);
  const telemetry = await request("/v1/query", { method: "POST", headers: { "content-type": "text/plain" },
    body: "SELECT service_name, count(*) AS spans FROM sigiro_spans WHERE service_name IN ('sigiro-agent-bridge', 'sigiro-agent-worker', 'sigiro-agent-eve') GROUP BY service_name" });
  for (const service of ["sigiro-agent-bridge", "sigiro-agent-worker", "sigiro-agent-eve"]) {
    assert.ok(telemetry.some((row: any) => row.service_name === service && row.spans > 0), `${service} must be observed in real Sigiro telemetry`);
  }
  checks.push("Sigiro observed bridge, worker, and Eve spans");
  const proof = { status: "passed", checks, telemetry };
  await writeFile(join(output, "negative-verification.json"), `${JSON.stringify(proof, null, 2)}\n`);
  console.log(JSON.stringify(proof, null, 2));
} else {
  throw new Error("Usage: bun test/bridge-e2e.ts traffic|verify|negative");
}
