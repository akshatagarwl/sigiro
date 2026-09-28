import { defineTool } from "eve/tools";
import { z } from "zod";
import { optimizerRuntime } from "../../optimizer/runtime.ts";

export default defineTool({
  description: "Read tenant-scoped Sigiro telemetry through the authorized optimizer run. Use sigiro_logs (timestamp TIMESTAMP, service_name, severity_text, body); filter epoch_us(timestamp) by the authorized microsecond window. Generic logs/otel_logs/telemetry tables do not exist. Only read-only, allowlisted SQL is accepted. Never copy raw identifiers or logs into the PR body.",
  inputSchema: z.object({ sql: z.string().min(1).max(4096) }),
  async execute({ sql }, ctx) {
    const runtime = optimizerRuntime();
    const run = await runtime.forSession(ctx.session.id, ctx.session.auth.current);
    return runtime.telemetry(run, sql);
  },
});
