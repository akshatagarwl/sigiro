import type { RuntimeSandboxSession } from "eve/sandbox";

/**
 * DI bridge required by Eve 0.51: model resolvers expose a session id but no
 * sandbox. A step.started hook supplies the public sandbox handle before the
 * model runs. Shared symbol prevents Eve's separate entry bundles from making
 * separate registries. Handles are refreshed every step, never persisted.
 */
const key = Symbol.for("sigiro.eve.sandbox-bindings");
const shared = globalThis as typeof globalThis & { [key]?: Map<string, RuntimeSandboxSession> };
const bindings = shared[key] ??= new Map<string, RuntimeSandboxSession>();

export function bindSandbox(sessionId: string, sandbox: RuntimeSandboxSession): void {
  bindings.set(sessionId, sandbox);
}

export function sandboxForSession(sessionId: string): RuntimeSandboxSession {
  const sandbox = bindings.get(sessionId);
  if (sandbox === undefined) throw new Error(`Eve did not bind a sandbox for session ${sessionId}`);
  return sandbox;
}

export async function releaseSandbox(sessionId: string): Promise<void> {
  const sandbox = bindings.get(sessionId);
  bindings.delete(sessionId);
  // Eve preserves the durable workspace; a later turn resumes compute lazily.
  if (sandbox !== undefined) await sandbox.stop();
}
