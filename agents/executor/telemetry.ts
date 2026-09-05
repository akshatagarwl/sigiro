import { Config, ConfigProvider, Effect, Layer } from "effect";
import { FetchHttpClient, HttpClient, HttpTraceContext } from "effect/unstable/http";
import { OtlpLogger, OtlpSerialization, OtlpTracer } from "effect/unstable/observability";

export const currentTraceHeaders = Effect.currentSpan.pipe(
  Effect.map(HttpTraceContext.toHeaders),
  Effect.catchTag("NoSuchElementError", () => Effect.succeed({})),
);

// The user requested dogfooding against the existing local Sigiro receiver.
// Only opaque correlation hashes, operation names and error codes leave this process.
const resource = { attributes: [{ key: "service.name", value: { stringValue: "sigiro-agent-worker" } }], droppedAttributesCount: 0 };
const attributeKeys = new Set(["execution.hash", "job.hash", "error.type"]);
const messages = new Set(["Sigiro agent worker started", "job claimed", "job reported", "job execution failed", "Eve cancellation failed", "worker iteration failed", "pending report failed"]);

const serialization = Layer.effect(OtlpSerialization.OtlpSerialization, Effect.gen(function* () {
  const json = yield* OtlpSerialization.OtlpSerialization;
  return OtlpSerialization.OtlpSerialization.of({
    metrics: json.metrics,
    traces: (data) => json.traces({ resourceSpans: data.resourceSpans.map((group) => ({
      ...group, resource, scopeSpans: group.scopeSpans.map((scope) => ({
        ...scope, spans: scope.spans.map((span) => ({
          ...span,
          attributes: span.attributes.filter((attribute) => attributeKeys.has(attribute.key)),
          events: [], links: [], status: { code: span.status.code },
        })),
      })),
    })) }),
    logs: (data) => json.logs({ resourceLogs: data.resourceLogs.map((group) => ({
      ...group, resource, scopeLogs: group.scopeLogs.map((scope) => ({
        ...scope, logRecords: (scope.logRecords ?? []).map((record) => {
          const first = record.body?.stringValue ?? record.body?.arrayValue?.values?.[0]?.stringValue;
          return {
            ...record,
            attributes: record.attributes.filter((attribute) => attributeKeys.has(attribute.key)),
            body: { stringValue: typeof first === "string" && messages.has(first) ? first : "agent worker diagnostic" },
          };
        }),
      })),
    })) }),
  });
})).pipe(Layer.provide(OtlpSerialization.layerJson));

// Do not log HTTP request bodies when the telemetry receiver is unavailable.
const transport = Layer.effect(HttpClient.HttpClient, Effect.gen(function* () {
  const client = yield* HttpClient.HttpClient;
  return client.pipe(HttpClient.filterStatusOk, HttpClient.tapError((error) => Effect.sync(() => {
    console.error("Sigiro telemetry export failed", {
      reason: error.reason._tag,
      ...(error.reason._tag === "StatusCodeError" ? { status: error.reason.response.status } : {}),
    });
  })));
})).pipe(Layer.provide(FetchHttpClient.layer));

export const telemetry = Layer.unwrap(Effect.gen(function* () {
  const endpoint = yield* Config.nonEmptyString("OTEL_EXPORTER_OTLP_ENDPOINT").pipe(Config.option);
  if (endpoint._tag === "None") {
    yield* Effect.logInfo("Sigiro telemetry disabled: OTEL_EXPORTER_OTLP_ENDPOINT is not set");
    return Layer.empty;
  }
  if (endpoint.value.replace(/\/$/, "") !== "http://127.0.0.1:14318") {
    return yield* Effect.fail(new Config.ConfigError(new ConfigProvider.SourceError({ message: "This rehearsal exports only to the existing local Sigiro receiver: http://127.0.0.1:14318" })));
  }
  const options = { resource: { serviceName: "sigiro-agent-worker" }, exportInterval: "1 second", shutdownTimeout: "5 seconds" } as const;
  return Layer.merge(
    OtlpLogger.layer({ ...options, url: "http://127.0.0.1:14318/v1/logs", mergeWithExisting: true }),
    OtlpTracer.layer({ ...options, url: "http://127.0.0.1:14318/v1/traces" }),
  ).pipe(Layer.provide(serialization), Layer.provide(transport));
}));
