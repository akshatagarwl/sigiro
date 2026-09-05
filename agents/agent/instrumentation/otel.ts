import { otel } from "eve/instrumentation/otel";

export default otel({
  resource: { "service.name": "sigiro-agent-eve" },
  traceChannelRequests: true,
  tracePolicy: () => ({ emit: true, recordInputs: false, recordOutputs: false }),
});
