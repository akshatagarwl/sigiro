import { readFile } from "node:fs/promises";
import { expect, test } from "bun:test";

test("Eve production wiring uses world-celld and isolated sandbox instructions", async () => {
  const agentSource = await readFile(new URL("../agent/agent.ts", import.meta.url), "utf8");
  const sandboxSource = await readFile(new URL("../agent/sandbox/sandbox.ts", import.meta.url), "utf8");
  const instructions = await readFile(new URL("../agent/instructions.md", import.meta.url), "utf8");
  expect(agentSource).toContain("model: defineDynamic");
  expect(agentSource).toContain("@ewhauser/world-celld");
  expect(sandboxSource).toContain("boxSandbox");
  // The Docker backend is gone. Its deny-all policy went with it, so nothing
  // here may imply an egress guarantee Box cannot make. See docs/threat-model.md.
  expect(sandboxSource).not.toContain("eve/sandbox/docker");
  expect(sandboxSource).not.toContain("deny-all");
  expect(sandboxSource).not.toContain("bootstrap");
  for (const source of [sandboxSource, instructions]) {
    expect(source).not.toContain("bug.check.js");
    expect(source).not.toContain("seed failing fixture");
  }
});
