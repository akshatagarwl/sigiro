import { defineTool } from "eve/tools";
import { Effect } from "effect";
import { z } from "zod";
import { optimizerRuntime } from "../../optimizer/runtime.ts";
import { collectChanges } from "../../optimizer/workspace.ts";

const quoteSql = (value: string) => value.replaceAll("'", "''");

export default defineTool({
  description: "Run native validation in the isolated workspace, then have the trusted host open one draft GitHub PR for the authorized run. Failures are disclosed, never silently passed.",
  inputSchema: z.object({
    hypothesis: z.string().min(1).max(400),
    changeSummary: z.string().min(1).max(400),
    risk: z.enum(["regression", "performance trade-off", "behavioral change", "unknown"]),
    validationCommands: z.array(z.string().min(1).max(300)).max(5),
  }),
  async execute({ hypothesis, changeSummary, risk, validationCommands }, ctx) {
    const runtime = optimizerRuntime();
    const run = await runtime.forSession(ctx.session.id, ctx.session.auth.current);
    const sandbox = await ctx.getSandbox();
    const validation: Array<{ number: number; status: "passed" | "failed" | "not-run"; exitCode?: number }> = [];
    for (const [index, command] of validationCommands.entries()) {
      try {
        const result = await sandbox.run({ command: `cd /workspace/repo && ${command}`, abortSignal: AbortSignal.timeout(120_000) });
        validation.push({ number: index + 1, status: result.exitCode === 0 ? "passed" : "failed", exitCode: result.exitCode });
      } catch {
        validation.push({ number: index + 1, status: "not-run" });
      }
    }
    const changes = await collectChanges(sandbox);
    if (run.windowStartUs === null || run.windowEndUs === null ||
      !Number.isSafeInteger(run.windowStartUs) || !Number.isSafeInteger(run.windowEndUs))
      throw new Error("A bounded live evidence window is required before publication");
    const timestamp = (micros: number) => new Date(micros / 1000).toISOString().replace("T", " ").replace("Z", "");
    const evidence = await Promise.all(["sigiro_spans", "sigiro_logs"].map((table) =>
      runtime.telemetry(run, `SELECT COUNT(*) AS events FROM ${table}
        WHERE service_name='${quoteSql(run.service)}'
        AND timestamp>=TIMESTAMP '${timestamp(run.windowStartUs!)}'
        AND timestamp<TIMESTAMP '${timestamp(run.windowEndUs!)}'
        AND (resource_attributes IS NULL OR resource_attributes NOT LIKE '%production-journald-backfill%')`)));
    const liveEvents = evidence.reduce((total, result) => total + z.object({ events: z.number().int().nonnegative() }).parse(result.rows[0]).events, 0);
    if (liveEvents === 0) throw new Error("No naturally emitted production telemetry supports this change");
    const summary = validation.length === 0 ? "NOT RUN: no validation commands provided" : validation
      .map((item) => `Command ${item.number}: ${item.status.toUpperCase()}${item.exitCode === undefined ? "" : ` (exit ${item.exitCode})`}`)
      .join("\n");
    const body = [
      "## Why", "Assessor recommended investigating recent live Sigiro telemetry; its probability is a routing signal, not proof of the proposed fix.",
      `At least ${liveEvents} naturally emitted events were verified for the mapped service in the bounded evidence window.`,
      "## Change", `${changes.length} changed paths. Inspect the diff before merging.`,
      "## Validation", summary,
      "Command output is withheld from this public-facing PR to avoid disclosing repository or telemetry secrets.",
      "## Safety", `Risk category: ${risk}. Hypothesis and change summary are withheld because model text may contain sensitive telemetry.`,
      "Draft only; a human reviews and merges.",
    ].join("\n\n");
    const saved = await runtime.pool.query(`UPDATE sigiro_admin.optimizer_run
      SET hypothesis=$5,change_summary=$6,validation_status=$7
      WHERE tenant_id=$1 AND id=$2 AND lease_owner=$3 AND attempts=$4 AND status='running'
      AND lease_until_ms >= (extract(epoch FROM clock_timestamp())*1000)::bigint`,
      [run.tenantId,run.id,run.leaseOwner,run.attempts,hypothesis,changeSummary,
       validation.some((item) => item.status === "failed") ? "failed" :
       validation.length === 0 || validation.some((item) => item.status === "not-run") ? "not-run" : "passed"]);
    if (saved.rowCount !== 1) throw new Error("Optimizer lease lost before PR publication");
    const published = await Effect.runPromise(runtime.github.publish({
      connection: run.connection, runId: run.id, baseRef: run.baseRef,
      targetCommitSha: run.targetCommitSha, changes,
      title: "optimizer: telemetry-backed performance change", body,
    }));
    return { pullRequestId: published.pullRequestId, branch: published.branch, validation };
  },
});
