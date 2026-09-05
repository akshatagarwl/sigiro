import assert from "node:assert/strict";
import { once } from "node:events";
import { createServer } from "node:http";
import { diag, DiagConsoleLogger, DiagLogLevel, SpanKind, SpanStatusCode, trace } from "@opentelemetry/api";
import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-proto";
import { defaultResource, resourceFromAttributes } from "@opentelemetry/resources";
import { NodeSDK } from "@opentelemetry/sdk-node";

assert.match(process.env.GIT_SHA ?? "", /^[a-f0-9]{40,64}$/, "GIT_SHA must identify the deployed commit");
assert.ok(process.env.OTEL_SDK_DISABLED === "true" || process.env.OTEL_EXPORTER_OTLP_ENDPOINT,
  "OTEL_EXPORTER_OTLP_ENDPOINT is required unless OTEL_SDK_DISABLED=true");
diag.setLogger(new DiagConsoleLogger(), DiagLogLevel.ERROR);
const sdk = new NodeSDK({
  autoDetectResources: false,
  resource: defaultResource().merge(resourceFromAttributes({
    "service.name": "sigiro-bridge-e2e-checkout",
    "service.version": process.env.GIT_SHA,
  })),
  traceExporter: new OTLPTraceExporter(),
  logRecordProcessors: [],
});
sdk.start();
const tracer = trace.getTracer("sigiro-bridge-e2e-checkout");

const server = createServer((request, response) => {
  const path = new URL(request.url, "http://localhost");
  if (request.method !== "GET" || path.pathname !== "/checkout") {
    response.writeHead(404).end();
    return;
  }
  // The fixture can compress historical event time while exercising real HTTP requests.
  const eventStart = request.headers["x-e2e-event-time"] === undefined ? Date.now() : Number(request.headers["x-e2e-event-time"]);
  assert.ok(Number.isFinite(eventStart), "invalid fixture event timestamp");
  tracer.startActiveSpan("GET /checkout", {
    startTime: new Date(eventStart),
    kind: SpanKind.SERVER,
    attributes: { "http.request.method": "GET", "http.route": "/checkout", "url.path": "/checkout" },
  }, (span) => {
    let status = 200;
    let body;
    try {
      const coupon = path.searchParams.get("coupon").trim().toUpperCase();
      const discountCents = coupon === "SAVE10" ? 1000 : 0;
      body = { totalCents: 10000 - discountCents, discountCents, currency: "USD" };
    } catch (error) {
      status = 500;
      span.recordException(error);
      span.setStatus({ code: SpanStatusCode.ERROR, message: error.message });
      span.setAttribute("error.type", error.name);
      console.error(error);
      body = { error: "Checkout failed" };
    } finally {
      span.setAttribute("http.response.status_code", status);
      response.writeHead(status, { "content-type": "application/json", "x-trace-id": span.spanContext().traceId });
      response.end(JSON.stringify(body));
      span.end(new Date(eventStart + 10));
    }
  });
});

const port = Number(process.env.PORT ?? 0);
assert.ok(Number.isInteger(port) && port >= 0 && port <= 65535, "PORT must be between 0 and 65535");
server.listen(port, "127.0.0.1");
await once(server, "listening");
export const url = `http://127.0.0.1:${server.address().port}`;
console.log(JSON.stringify({ url, service: "sigiro-bridge-e2e-checkout", version: process.env.GIT_SHA }));

export async function close() {
  await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  await sdk.shutdown();
}

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.once(signal, () => close().catch((error) => {
    console.error(error);
    process.exitCode = 1;
  }));
}
