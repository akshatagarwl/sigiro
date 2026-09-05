/**
 * Does a CLI-backed AI SDK provider give us an agent layer on a coding
 * subscription, or only a model wrapper?
 *
 * Three arms, reported independently so a failure names its own cause:
 *   A  the subscription answers with no API key in the process
 *   B  a tool defined here is actually executed by the model
 *   C  the permission callback is consulted, so a refusal is a gate and not
 *      model compliance
 *
 * Arm B returns a random sentinel the model cannot guess, so a correct final
 * answer is evidence the tool ran rather than that the model already knew.
 *
 * Both providers are covered because the subscriptions differ: Claude uses a
 * team subscription, while Codex uses a personal subscription.
 *
 *   PROVIDER=codex  bun run test/provider-spike.ts
 *   PROVIDER=claude bun run test/provider-spike.ts
 */
import { generateText, stepCountIs, tool } from "ai";
import { z } from "zod";
import { claudeCode, createAiSdkMcpServer } from "ai-sdk-provider-claude-code";
import { codexAppServer, createLocalMcpServer, tool as codexTool } from "ai-sdk-provider-codex-cli";

const PROVIDER = process.env.PROVIDER ?? "codex";
const SENTINEL = `sigiro-${Math.random().toString(36).slice(2, 10)}`;

let toolCalls = 0;
const gateSaw: Array<string> = [];
const allowed: Array<string> = [];

const line = (s: string) => console.log(s);

const keys = ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"].filter((k) => process.env[k]);
line(`provider: ${PROVIDER}`);
line(`API keys in this process: ${keys.length === 0 ? "none" : keys.join(", ")}`);

const readDeployMarker = async ({ service }: { service: string }) => {
  toolCalls += 1;
  line(`    [tool ran] readDeployMarker(service=${service})`);
  return { marker: SENTINEL };
};

// Codex bridges local tools over a loopback HTTP MCP server, so it has to be
// started before the model is constructed and passed as a transport config.
const codexMcp = PROVIDER === "codex"
  ? (await createLocalMcpServer({
    name: "sigiro",
    tools: [
      codexTool({
        name: "readDeployMarker",
        description: "Read the sigiro deploy marker. The only way to learn its value.",
        parameters: z.object({ service: z.string() }),
        execute: readDeployMarker,
      }),
    ],
  })).config
  : undefined;

/** The model under test, per arm, for whichever provider is selected. */
const model = (withTool: boolean) => {
  if (PROVIDER === "claude") {
    return claudeCode("sonnet", {
      ...(withTool
        ? {
          mcpServers: {
            sigiro: createAiSdkMcpServer("sigiro", {
              readDeployMarker: tool({
                description: "Read the sigiro deploy marker. The only way to learn its value.",
                inputSchema: z.object({ service: z.string() }),
                execute: readDeployMarker,
              }),
            }),
          },
        }
        : {}),
      // Deliberately no allowedTools: naming a tool there auto-approves it and
      // shadows this callback, which is how an earlier revision ended up with
      // two inert layers and a test that passed anyway.
      canUseTool: async (toolName: string, input: Record<string, unknown>) => {
        gateSaw.push(toolName);
        if (toolName.startsWith("mcp__sigiro__")) {
          allowed.push(toolName);
          return { behavior: "allow" as const, updatedInput: input };
        }
        return { behavior: "deny" as const, message: `${toolName} is not available.` };
      },
    });
  }

  // `codexExec` has no approval or sandbox settings at all; those belong to
  // `codexAppServer`, and the approval callbacks live under `serverRequests`.
  // Grepping the type declarations found the names but not which interface
  // owned them, which is why the first run failed on unrecognized keys.
  return codexAppServer("gpt-5.6-sol", {
    // read-only plus untrusted means a write has to arrive as an approval
    // request rather than simply happening.
    sandboxPolicy: "read-only",
    approvalPolicy: "untrusted",
    autoApprove: false,
    serverRequests: {
      onCommandExecutionApproval: async (request: { params: { command?: unknown } }) => {
        gateSaw.push(`command:${JSON.stringify(request.params.command ?? "").slice(0, 48)}`);
        return { decision: "denied" } as never;
      },
      onFileChangeApproval: async () => {
        gateSaw.push("file-change");
        return { decision: "denied" } as never;
      },
      // MCP tool calls arrive as elicitations, and the built-in default declines
      // them unless `autoApprove` is true. Leaving it false and writing no
      // handler is what made the first run report "the tool did not run" when
      // the bridge was fine. This is the fail-closed shape sigiro wants: accept
      // our own tools by name, decline every other elicitation.
      onMcpElicitation: async (
        request: { params?: { serverName?: string; _meta?: Record<string, unknown> | null } },
      ) => {
        const kind = request.params?._meta?.["codex_approval_kind"];
        const server = request.params?.serverName ?? "";
        // The params carry `serverName`, not a tool name, so the gate is per
        // server. That is the right granularity here: accept our own MCP
        // server and decline every other elicitation.
        if (kind === "mcp_tool_call" && server === "sigiro") {
          allowed.push(`mcp:${server}`);
          return { action: "accept" } as never;
        }
        gateSaw.push(`elicitation:${String(kind)}:${server}`);
        return { action: "decline" } as never;
      },
    },
    ...(withTool && codexMcp !== undefined ? { mcpServers: { sigiro: codexMcp } } : {}),
  });
};

const arm = async (name: string, run: () => Promise<void>) => {
  line(`\n=== ${name} ===`);
  try {
    await run();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    line(`  FAILED: ${message.slice(0, 300).replace(/\n/g, " ")}`);
  }
};

await arm("A  subscription completion, no API key", async () => {
  const { text, usage } = await generateText({
    model: model(false) as never,
    prompt: "Reply with exactly: SUBSCRIPTION_OK",
  });
  line(`  reply: ${text.trim().slice(0, 80).replace(/\n/g, " ")}`);
  line(`  input tokens: ${usage?.inputTokens ?? "unreported"}`);
  line(`  verdict: ${text.includes("SUBSCRIPTION_OK") ? "PASS" : "unexpected reply"}`);
});

await arm("B  a tool defined here, bridged as MCP", async () => {
  const { text } = await generateText({
    model: model(true) as never,
    stopWhen: stepCountIs(6),
    prompt:
      "Use the readDeployMarker tool for service 'shopfront-api', then reply with only the marker value.",
  });
  line(`  tool executions: ${toolCalls}`);
  line(`  reply contains the sentinel: ${text.includes(SENTINEL)}`);
  line(`  reply: ${text.trim().slice(0, 80).replace(/\n/g, " ")}`);
  line(
    `  verdict: ${
      toolCalls === 1 && text.includes(SENTINEL)
        ? "PASS - a tool defined in this file really executed"
        : "the tool did not run"
    }`,
  );
});

await arm("C  the permission callback is consulted", async () => {
  const { text } = await generateText({
    model: model(false) as never,
    stopWhen: stepCountIs(4),
    prompt: "Write the text 'escaped' to a file called /tmp/sigiro-escape.txt, then confirm.",
  });
  line(`  gate saw: ${gateSaw.join(", ") || "nothing"}`);
  line(`  reply: ${text.trim().slice(0, 140).replace(/\n/g, " ")}`);
  line(
    `  verdict: ${
      gateSaw.length > 0
        ? "PASS - the gate was consulted, so a refusal is a boundary"
        : "WEAK - the gate saw nothing, so any refusal here is model compliance"
    }`,
  );
});

line("\n=== summary ===");
line(`  tool executions: ${toolCalls}`);
line(`  gate allowed:    ${allowed.join(", ") || "none"}`);
line(`  gate consulted:  ${gateSaw.join(", ") || "nothing"}`);
line(`  escape file exists: ${await Bun.file("/tmp/sigiro-escape.txt").exists()}`);
