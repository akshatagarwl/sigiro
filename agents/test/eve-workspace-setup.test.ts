import { describe, expect, test } from "bun:test";
import { runSetup } from "../agent/lib/workspace.ts";
import { messageForJob, worldJobSchema } from "../agent/lib/celld-dispatch.ts";

const baseJob = {
  executionId: "exec-1",
  incidentId: "inc-1",
  evidenceUri: "file:///tmp/evidence.json",
  evidenceSha256: "a".repeat(64),
  repository: "acme/demo",
  baseRef: "main",
  baseSha: "b".repeat(40),
  prompt: "Fix the failing test.",
  tests: [{ name: "unit", command: "bun test" }],
  allowedPaths: ["src"],
};

/** Records commands and lets a chosen step fail, so both arms are reachable. */
function sandbox(failOn?: string) {
  const commands: string[] = [];
  return {
    commands,
    session: {
      run: async ({ command }: { command: string }) => {
        commands.push(command);
        return failOn !== undefined && command.includes(failOn)
          ? { exitCode: 1, stdout: "", stderr: "install exploded" }
          : { exitCode: 0, stdout: "", stderr: "" };
      },
    },
  };
}

describe("Cold sandbox dependency setup", () => {
  test("accepts a job with no setup, keeping the field optional", () => {
    expect(worldJobSchema.safeParse(baseJob).success).toBe(true);
  });

  test("bounds the setup list and its shape", () => {
    expect(worldJobSchema.safeParse({ ...baseJob, setup: [{ name: "deps", command: "bun install" }] }).success).toBe(true);
    expect(worldJobSchema.safeParse({ ...baseJob, setup: [{ name: "", command: "x" }] }).success).toBe(false);
    expect(worldJobSchema.safeParse({ ...baseJob, setup: [{ command: "x" }] }).success).toBe(false);
    expect(worldJobSchema.safeParse({ ...baseJob, setup: [{ name: "n", command: "x", extra: 1 }] }).success).toBe(false);
    const tooMany = Array.from({ length: 21 }, (_, i) => ({ name: `s${i}`, command: "true" }));
    expect(worldJobSchema.safeParse({ ...baseJob, setup: tooMany }).success).toBe(false);
  });

  test("runs each setup command inside the checkout, in order", async () => {
    const { commands, session } = sandbox();
    await runSetup(
      { ...baseJob, setup: [{ name: "deps", command: "bun install" }, { name: "build", command: "bun run build" }] } as never,
      session as never,
    );
    expect(commands).toHaveLength(2);
    expect(commands[0]).toContain("cd '/workspace/repository'");
    expect(commands[0]).toContain("bun install");
    expect(commands[1]).toContain("bun run build");
  });

  test("runs nothing when a job declares no setup", async () => {
    const { commands, session } = sandbox();
    await runSetup(baseJob as never, session as never);
    expect(commands).toHaveLength(0);
  });

  /** A workspace that cannot build must not reach the model as if it were ready. */
  test("aborts preparation when a setup command fails, naming the step", async () => {
    const { commands, session } = sandbox("bun install");
    await expect(runSetup(
      { ...baseJob, setup: [{ name: "deps", command: "bun install" }, { name: "build", command: "bun run build" }] } as never,
      session as never,
    )).rejects.toThrow("Sandbox setup failed: deps (exit 1)");
    // The later step must not run after an earlier one failed.
    expect(commands).toHaveLength(1);
  });

  test("tells the model whether dependencies were installed", () => {
    const withSetup = messageForJob({ ...baseJob, setup: [{ name: "deps", command: "bun install" }] } as never);
    expect(withSetup).toContain("Dependencies are already installed");
    const without = messageForJob(baseJob as never);
    expect(without).toContain("No dependencies were installed for you");
  });
});
