import { defineHook } from "eve/hooks";
import { bindSandbox, releaseSandbox } from "../lib/sandbox-binding.ts";

export default defineHook({
  events: {
    "step.started": async (_event, ctx) => { bindSandbox(ctx.session.id, await ctx.getSandbox()); },
    "turn.completed": (_event, ctx) => releaseSandbox(ctx.session.id),
    "turn.failed": (_event, ctx) => releaseSandbox(ctx.session.id),
    "turn.cancelled": (_event, ctx) => releaseSandbox(ctx.session.id),
    "session.failed": (_event, ctx) => releaseSandbox(ctx.session.id),
  },
});
