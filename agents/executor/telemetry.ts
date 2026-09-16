import { Config, ConfigProvider, Effect, Layer } from "effect";
import { FetchHttpClient, HttpClient, HttpTraceContext } from "effect/unstable/http";
import { Otlp, OtlpSerialization } from "effect/unstable/observability";

export const currentTraceHeaders = Effect.currentSpan.pipe(
  Effect.map(HttpTraceContext.toHeaders),
  Effect.catchTag("NoSuchElementError", () => Effect.succeed({})),
);

// Export only to the customer's local Sigiro receiver.
// Only opaque correlation hashes, operation names and error codes leave this process.
const attributeKeys = new Set(["execution.hash", "job.hash", "error.type"]);
const messages = new Set(["Sigiro agent worker started", "job claimed", "job reported", "job execution failed", "Eve cancellation failed", "worker iteration failed", "pending report failed", "bridge job submitted", "bridge pass failed"]);

const serialization = Layer.effect(OtlpSerialization.OtlpSerialization, Effect.gen(function* () {
  const json = yield* OtlpSerialization.OtlpSerialization;
  return OtlpSerialization.OtlpSerialization.of({
    metrics: json.metrics,
    traces: (data) => json.traces({ resourceSpans: data.resourceSpans.map((group) => ({
      ...group, scopeSpans: group.scopeSpans.map((scope) => ({
        ...scope, spans: scope.spans.map((span) => ({
          ...span,
          attributes: span.attributes.filter((attribute) => attributeKeys.has(attribute.key)),
          events: [], links: [], status: { code: span.status.code },
        })),
      })),
    })) }),
    logs: (data) => json.logs({ resourceLogs: data.resourceLogs.map((group) => ({
      ...group, scopeLogs: group.scopeLogs.map((scope) => ({
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
  return client.pipe(HttpClient.filterStatusOk, HttpClient.tapError((error) =>
    Effect.logError("Sigiro telemetry export failed", error.reason._tag),
  ));
})).pipe(Layer.provide(FetchHttpClient.layer));

const telemetryFor = (serviceName: string) => Layer.unwrap(Effect.gen(function* () {
  const endpoint = yield* Config.NonEmptyString("OTEL_EXPORTER_OTLP_ENDPOINT").pipe(Config.option);
  if (endpoint._tag === "None") {
    yield* Effect.logInfo("Sigiro telemetry disabled: OTEL_EXPORTER_OTLP_ENDPOINT is not set");
    return Layer.empty;
  }
  const url = endpoint.value.replace(/\/$/, "");
  if (url !== "http://127.0.0.1:14318" && url !== "http://sigiro:4318") {
    return yield* Effect.fail(new Config.ConfigError(new ConfigProvider.SourceError({ message: "Agent telemetry requires the local Sigiro receiver: http://127.0.0.1:14318 or http://sigiro:4318" })));
  }
  return Otlp.layer({
    baseUrl: url,
    resource: { serviceName },
    loggerExportInterval: "1 second",
    metricsExportInterval: "1 second",
    tracerExportInterval: "1 second",
    loggerMergeWithExisting: true,
    shutdownTimeout: "5 seconds",
  }).pipe(Layer.provide(serialization), Layer.provide(transport));
}));

export const telemetry = telemetryFor("sigiro-agent-worker");
export const bridgeTelemetry = telemetryFor("sigiro-agent-bridge");
