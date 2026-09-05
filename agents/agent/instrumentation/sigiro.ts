import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-proto";
import { otelIntegration } from "eve/instrumentation/otel";

export default otelIntegration({
  // Only the customer's local demo Sigiro collector receives these spans.
  // Eve removes input/output content, exception events, and status messages.
  traceExporter: new OTLPTraceExporter({ url: "http://127.0.0.1:14318/v1/traces" }),
  recordInputs: false,
  recordOutputs: false,
  runtimeContext: ({ session }) => {
    const auth = session.auth.current;
    if (auth?.authenticator !== "celld-worker-token") return undefined;
    const { executionId, incidentId } = auth.attributes ?? {};
    return {
      ...(typeof executionId === "string" ? { "execution.hash": createHash("sha256").update(executionId).digest("hex") } : {}),
      ...(typeof incidentId === "string" ? { "incident.hash": createHash("sha256").update(incidentId).digest("hex") } : {}),
    };
  },
});
import { createHash } from "node:crypto";
