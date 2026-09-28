import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-proto";
import { registerOTel } from "@vercel/otel";
import { defineInstrumentation } from "eve/instrumentation";

export default defineInstrumentation({
  recordInputs: false,
  recordOutputs: false,
  setup: ({ agentName }) => {
    const url = process.env.SIGIRO_OTLP_TRACES_URL;
    const key = process.env.SIGIRO_API_KEY;
    if (!url || !key) {
      throw new Error("SIGIRO_OTLP_TRACES_URL and SIGIRO_API_KEY are required");
    }
    registerOTel({
      serviceName: agentName,
      instrumentations: [],
      traceExporter: new OTLPTraceExporter({
        url,
        headers: { Authorization: `Bearer ${key}` },
      }),
    });
  },
});
