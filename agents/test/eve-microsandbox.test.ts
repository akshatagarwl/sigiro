import { readFile } from "node:fs/promises";
import { describe, expect, test } from "bun:test";

describe("Eve microsandbox production wiring", () => {
  test("pins the model and backend without fixture bootstrap code", async () => {
    const agentSource = await readFile(new URL("../agent/agent.ts", import.meta.url), "utf8");
    const sandboxSource = await readFile(
      new URL("../agent/sandbox/sandbox.ts", import.meta.url),
      "utf8",
    );
    const instructions = await readFile(
      new URL("../agent/instructions.md", import.meta.url),
      "utf8",
    );
    expect(agentSource).toContain('model: chatgpt("gpt-5.6-luna")');
    expect(sandboxSource).not.toContain("bootstrap");
    for (const source of [sandboxSource, instructions]) {
      expect(source).not.toContain("/workspace/repo");
      expect(source).not.toContain("bug.check.js");
      expect(source).not.toContain("seed failing fixture");
    }

    const process = Bun.spawn(["bunx", "eve", "info", "--json"], {
      env: {
        ...Bun.env,
        EVE_SANDBOX_PROVIDER: "microsandbox",
        OPENAI_API_KEY: undefined,
      },
      stdout: "pipe",
      stderr: "pipe",
    });
    const [exitCode, stdout, stderr] = await Promise.all([
      process.exited,
      new Response(process.stdout).text(),
      new Response(process.stderr).text(),
    ]);
    expect(stderr).toBe("");
    expect(exitCode).toBe(0);
    const info = JSON.parse(stdout) as {
      status: string;
      model: string;
      artifacts: { compiledManifest: string };
    };
    const manifest = JSON.parse(await readFile(info.artifacts.compiledManifest, "utf8")) as {
      config: { model: { id: string; routing: { provider: string } } };
      sandbox: { backendName: string };
    };
    expect(info.status).toBe("ready");
    expect(info.model).toBe("codex/gpt-5.6-luna");
    expect(manifest.config.model.id).toBe("codex/gpt-5.6-luna");
    expect(manifest.config.model.routing.provider).toBe("codex");
    expect(manifest.sandbox.backendName).toBe("microsandbox");
  });
});
