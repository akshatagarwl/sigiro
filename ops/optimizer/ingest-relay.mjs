import { timingSafeEqual } from "node:crypto";
import { createServer } from "node:http";

import { InvalidBody, readBoundedBody } from "./read-bounded-body.mjs";
import {
  relayTarget,
  retryProfileAdmission,
  validPyroscopeContentType,
} from "./relay-target.mjs";

const clientId = process.env.SIGIRO_INGEST_CLIENT_ID;
const clientSecret = process.env.SIGIRO_INGEST_CLIENT_SECRET;
const relayKey = process.env.SIGIRO_INGEST_RELAY_KEY;
if (!clientId || !clientSecret || !/^[0-9a-f]{64}$/.test(relayKey ?? "")) {
  throw new Error("ingest relay credentials are missing or invalid");
}
const limit = 8 * 1024 * 1024;
const forwarded = { traces: 0, logs: 0, metrics: 0, profiles: 0 };
const rejected = { unauthenticated: 0, upstream: 0 };
let cached;
let inflight;

async function accessToken() {
  if (cached && cached.expires > Date.now()) return cached.value;
  inflight ??= (async () => {
    const response = await fetch("https://sigiro.com/api/auth/oauth2/token", {
      method: "POST",
      headers: {
        authorization: `Basic ${Buffer.from(`${clientId}:${clientSecret}`).toString("base64")}`,
        "content-type": "application/x-www-form-urlencoded",
      },
      body: new URLSearchParams({
        grant_type: "client_credentials",
        scope: "telemetry:ingest",
        resource: "https://sigiro.com",
      }),
      redirect: "error",
      signal: AbortSignal.timeout(10000),
    });
    if (!response.ok)
      throw new Error(`OAuth token request returned ${response.status}`);
    const issued = await response.json();
    if (
      typeof issued.access_token !== "string" ||
      !Number.isSafeInteger(issued.expires_in) ||
      issued.expires_in < 90
    ) {
      throw new Error("OAuth response lacks a valid short-lived access token");
    }
    cached = {
      value: issued.access_token,
      expires: Date.now() + (issued.expires_in - 60) * 1000,
    };
    return cached.value;
  })();
  try {
    return await inflight;
  } finally {
    inflight = undefined;
  }
}

function authorized(header) {
  const submitted = Buffer.from(header ?? "");
  const expected = Buffer.from(`Bearer ${relayKey}`);
  return (
    submitted.length === expected.length && timingSafeEqual(submitted, expected)
  );
}

async function forward(path, body, headers) {
  const token = await accessToken();
  const outbound = {
    authorization: `Bearer ${token}`,
    "content-type": headers["content-type"] ?? "application/x-protobuf",
  };
  if (headers["content-encoding"])
    outbound["content-encoding"] = headers["content-encoding"];
  const request = () =>
    fetch(`http://127.0.0.1:9999${path}`, {
      method: "POST",
      headers: outbound,
      body,
      redirect: "error",
      signal: AbortSignal.timeout(18000),
    });
  let result = await request();
  if (result.status === 401) {
    cached = undefined;
    outbound.authorization = `Bearer ${await accessToken()}`;
    result = await request();
  }
  result = await retryProfileAdmission(path, result, request);
  const reply = Buffer.from(await result.arrayBuffer());
  if (reply.length > 1024 * 1024)
    throw new Error("OTLP upstream response exceeds limit");
  return {
    status: result.status,
    body: reply,
    contentType: result.headers.get("content-type"),
  };
}

createServer(async (req, res) => {
  if (req.method === "GET" && req.url === "/health") {
    res
      .writeHead(200, { "content-type": "application/json" })
      .end(JSON.stringify({ forwarded, rejected }));
    return;
  }
  const target = req.method === "POST" ? relayTarget(req.url) : null;
  if (!target) {
    res.writeHead(404).end();
    return;
  }
  if (!authorized(req.headers.authorization)) {
    rejected.unauthenticated++;
    res.writeHead(401).end();
    return;
  }
  try {
    if (
      target.counter === "profiles" &&
      target.path.startsWith("/ingest?") &&
      !validPyroscopeContentType(req.headers["content-type"])
    ) {
      res.writeHead(415).end();
      return;
    }
    const body = await readBoundedBody(req, limit);
    const upstream = await forward(target.path, body, req.headers);
    if (upstream.status === 200) forwarded[target.counter]++;
    else rejected.upstream++;
    res
      .writeHead(
        upstream.status,
        upstream.contentType ? { "content-type": upstream.contentType } : {}
      )
      .end(upstream.body);
  } catch (error) {
    if (error instanceof InvalidBody) {
      res.writeHead(error.status).end();
      return;
    }
    console.error(
      "ingest relay request failed",
      error instanceof Error ? error.name : "unknown"
    );
    if (!res.headersSent) res.writeHead(503).end();
  }
}).listen(14318, "127.0.0.1", () =>
  console.info("tenant ingest relay bound to loopback")
);
