import { expect, test } from "bun:test";
import { dispatchWorldJob, messageForJob, worldJobSchema } from "../agent/lib/celld-dispatch.ts";

const job = { executionId: "exec-1", incidentId: "incident-1", evidenceUri: "s3://evidence", evidenceSha256: "a".repeat(64), repository: "acme/demo", baseRef: "main", baseSha: "b".repeat(40), prompt: "Fix the failing test.", tests: [{ name: "unit", command: "bun test" }], allowedPaths: ["src", "test"] };

test("validates exact repository, base SHA, evidence, containment, and policy bounds", () => {
  expect(worldJobSchema.safeParse(job).success).toBe(true);
  expect(worldJobSchema.safeParse({ ...job, baseSha: "B".repeat(40) }).success).toBe(false);
  expect(worldJobSchema.safeParse({ ...job, evidenceSha256: "bad" }).success).toBe(false);
  expect(worldJobSchema.safeParse({ ...job, allowedPaths: [] }).success).toBe(false);
  const message = messageForJob(job);
  expect(message).toContain(`git rev-parse HEAD equals exactly ${job.baseSha}`);
  expect(message).toContain("Leave all changes uncommitted");
  expect(message).toContain("/workspace/sigiro-evidence.json");
});

test("uses one stable execution address and sends no local worker credential", async () => {
  const calls: Array<{ address: string; options: unknown }> = [];
  const result = await dispatchWorldJob(job, ((address, ) => ({ send: async (_message: string, options: unknown) => { calls.push({ address, options }); return { id: "session-1" }; } })) as never);
  expect(result).toEqual({ sessionId: "session-1" });
  expect(calls[0]?.address).toBe("exec-1");
  expect(calls[0]?.options).not.toHaveProperty("workerId");
  expect(calls[0]?.options).not.toHaveProperty("leaseToken");
});
