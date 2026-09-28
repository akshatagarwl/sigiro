import { defineTool } from "eve/tools";
import { Effect } from "effect";
import { z } from "zod";
import { optimizerRuntime } from "../../optimizer/runtime.ts";
import { provisionWorkspace } from "../../optimizer/workspace.ts";

export default defineTool({
  description: "Prepare this run's explicitly mapped repository in the isolated credential-free workspace. Call before inspecting or editing files.",
  inputSchema: z.object({}),
  async execute(_input, ctx) {
    const runtime = optimizerRuntime();
    const run = await runtime.forSession(ctx.session.id, ctx.session.auth.current);
    const files = await Effect.runPromise(runtime.github.checkout(run.connection, run.targetCommitSha));
    await provisionWorkspace(await ctx.getSandbox(), files);
    return {
      workspace: "/workspace/repo", fileCount: files.length, baseRef: run.baseRef,
      assessorDecision: { yesProbability: run.decisionContext.yes_probability },
      evidenceWindowUs: { start: run.windowStartUs, end: run.windowEndUs },
    };
  },
});
