import { tmpdir } from "node:os";
import { SpanStatusCode, trace } from "@opentelemetry/api";
import { claudeCode, createAiSdkMcpServer } from "ai-sdk-provider-claude-code";
import { chatgpt } from "eve/models/openai";
import { z } from "zod";
import { sandboxForSession } from "./sandbox-binding.ts";
import { CHECKOUT_PATH } from "./workspace.ts";

function selection(): { provider: "claude" | "codex"; model: string } {
  const provider = process.env.SIGIRO_AGENT_PROVIDER ?? "claude";
  if (provider !== "claude" && provider !== "codex") throw new Error(`Unsupported subscription provider: ${provider}`);
  return { provider, model: process.env.SIGIRO_AGENT_MODEL ?? (provider === "claude" ? "sonnet" : "gpt-5.6-luna") };
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

export function subscriptionModel(sessionId: string) {
  const selected = selection();
  // Eve's ChatGPT adapter obtains customer auth through the bundled Codex CLI.
  // Eve owns inference and its default file/shell tools use ctx.getSandbox().
  if (selected.provider === "codex") return chatgpt(selected.model);
  // Resolve the binding at tool-call time: Eve selects the model before firing
  // step.started hooks, but starts its model stream only after those hooks.
  const server = createAiSdkMcpServer("sandbox", {
    run: {
      description: "Run a shell command in the Eve Docker sandbox, in /workspace/repository. Read evidence at /workspace/sigiro-evidence.json before editing. All file and command operations must use this tool.",
      inputSchema: z.object({ command: z.string().min(1).max(32_768) }),
      async execute({ command }: { command: string }, options) {
        // Claude executes MCP tools inside its SDK, outside AI SDK's tool hooks.
        // Reuse Eve's tracer and export operation metadata only.
        return await trace.getTracer("sigiro.agent").startActiveSpan("execute_tool mcp__sandbox__run", {
          attributes: {
            "gen_ai.operation.name": "execute_tool",
            "gen_ai.tool.name": "mcp__sandbox__run",
            "agent.model.provider": "claude-code",
            "agent.session.id": sessionId,
          },
        }, async (span) => {
          try {
            const result = await sandboxForSession(sessionId).run({
              command,
              workingDirectory: CHECKOUT_PATH,
              ...(options?.abortSignal === undefined ? {} : { abortSignal: options.abortSignal }),
            });
            span.setAttribute("process.exit.code", result.exitCode);
            if (result.exitCode !== 0) throw new Error(`Sandbox command failed (${result.exitCode})\n${result.stdout}\n${result.stderr}`);
            span.setAttribute("sigiro.outcome", "succeeded");
            span.setStatus({ code: SpanStatusCode.OK });
            return result;
          } catch (cause) {
            span.setAttribute("sigiro.outcome", options?.abortSignal?.aborted === true ? "cancelled" : "failed");
            span.setAttribute("error.type", "sandbox_execution_failed");
            span.setStatus({ code: SpanStatusCode.ERROR });
            throw cause;
          } finally {
            span.end();
          }
        });
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
    canUseTool: async (name, input) => name === "mcp__sandbox__run"
      ? { behavior: "allow", updatedInput: input }
      : { behavior: "deny", message: "Only the Eve sandbox MCP tool is available" },
  });
}
