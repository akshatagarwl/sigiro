import { tmpdir } from "node:os";
import { claudeCode, createAiSdkMcpServer } from "ai-sdk-provider-claude-code";
import { chatgpt } from "eve/models/openai";
import { Config, Effect, Option, Schema } from "effect";
import { z } from "zod";
import { sandboxForSession } from "./sandbox-binding.ts";
import { CHECKOUT_PATH } from "./workspace.ts";

const selectionConfig = Config.all({
  provider: Config.String("SIGIRO_AGENT_PROVIDER").pipe(Config.withDefault("claude")),
  model: Config.String("SIGIRO_AGENT_MODEL").pipe(Config.option),
});

function selection(): { provider: "claude" | "codex"; model: string } {
  const configured = Effect.runSync(selectionConfig);
  const provider = configured.provider;
  if (provider !== "claude" && provider !== "codex") throw new Error(`Unsupported subscription provider: ${provider}`);
  return { provider, model: Option.getOrElse(configured.model, () => provider === "claude" ? "sonnet" : "gpt-5.6-luna") };
}

const common = {
  tools: [],
  settingSources: [],
  strictMcpConfig: true,
  persistSession: false,
  cwd: tmpdir(),
  // Authentication comes from the customer's Claude login. Never select API billing.
  env: { ANTHROPIC_API_KEY: undefined, CLAUDECODE: undefined },
} as const;

/** Compaction has no tool access and can run before the sandbox binding hook. */
export function summaryModel() {
  const selected = selection();
  if (selected.provider === "codex") return chatgpt(selected.model);
  return claudeCode(selected.model, { ...common, tools: [], settingSources: [], maxTurns: 1 });
}

export class SandboxExecutionError extends Schema.TaggedError<SandboxExecutionError>()("SandboxExecutionError", {
  message: Schema.String,
  exitCode: Schema.Int,
  stdout: Schema.String,
  stderr: Schema.String,
  cancelled: Schema.Boolean,
}) {}

const executeSandboxCommand = Effect.fn("execute_tool mcp__sandbox__run")(function*(sessionId: string, command: string, abortSignal: AbortSignal | undefined) {
  yield* Effect.annotateSpans(Effect.void, {
    "gen_ai.operation.name": "execute_tool",
    "gen_ai.tool.name": "mcp__sandbox__run",
    "agent.model.provider": "claude-code",
    "agent.session.id": sessionId,
    "agent.command.length": command.length,
  });
  const result = yield* Effect.tryPromise({
    try: () => sandboxForSession(sessionId).run({
      command,
      workingDirectory: CHECKOUT_PATH,
      ...(abortSignal === undefined ? {} : { abortSignal }),
    }),
    catch: (cause) => new SandboxExecutionError({
      message: `Sandbox command could not execute: ${String(cause)}`,
      exitCode: -1,
      stdout: "",
      stderr: "",
      cancelled: abortSignal?.aborted === true,
    }),
  });
  yield* Effect.annotateSpans(Effect.void, {
    "process.exit.code": result.exitCode,
    "sigiro.outcome": result.exitCode === 0 ? "succeeded" : "failed",
  });
  if (result.exitCode !== 0) {
    return yield* new SandboxExecutionError({
      message: `Sandbox command failed (${result.exitCode})\n${result.stdout}\n${result.stderr}`,
      exitCode: result.exitCode,
      stdout: result.stdout,
      stderr: result.stderr,
      cancelled: abortSignal?.aborted === true,
    });
  }
  return result;
});

export function subscriptionModel(sessionId: string) {
  const selected = selection();
  // Eve's ChatGPT adapter obtains customer auth through the bundled Codex CLI.
  // Eve owns inference and its default file/shell tools use ctx.getSandbox().
  if (selected.provider === "codex") return chatgpt(selected.model);
  // This is the only Zod adapter: createAiSdkMcpServer's runtime requires a Zod object schema.
  const server = createAiSdkMcpServer("sandbox", {
    run: {
      description: "Run a shell command in the Eve Docker sandbox, in /workspace/repository. Read evidence at /workspace/sigiro-evidence.json before editing. All file and command operations must use this tool.",
      inputSchema: z.object({ command: z.string().min(1).max(32_768) }),
      execute({ command }, options) {
        return Effect.runPromise(executeSandboxCommand(sessionId, command, options?.abortSignal).pipe(
          Effect.catch((error) => Effect.annotateSpans(Effect.fail(error), {
            "sigiro.outcome": error.cancelled ? "cancelled" : "failed",
            "error.type": "sandbox_execution_failed",
          })),
        ));
      },
    },
  });
  return claudeCode(selected.model, {
    ...common,
    tools: [],
    settingSources: [],
    mcpServers: { sandbox: server },
    allowedTools: ["mcp__sandbox__run"],
    maxTurns: 24,
    canUseTool: (name, input) => Promise.resolve(name === "mcp__sandbox__run"
      ? { behavior: "allow", updatedInput: input }
      : { behavior: "deny", message: "Only the Eve sandbox MCP tool is available" }),
  });
}
