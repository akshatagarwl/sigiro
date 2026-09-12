import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtemp, open, readFile, realpath, rm, stat } from "node:fs/promises";
import { isAbsolute, join, relative } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { GitHubApiError } from "eve/channels/github";
import type { GitHubHandle } from "eve/channels/github";
import type { SandboxSession } from "eve/sandbox";
import { quote } from "shell-quote";
import type { CelldJob } from "./celld-dispatch.ts";

const MAX_EVIDENCE_BYTES = 1024 * 1024;
const MAX_BUNDLE_BYTES = 64 * 1024 * 1024;
export const CHECKOUT_PATH = "/workspace/repository";

export class EvidenceDigestMismatchError extends Error {
  constructor() {
    super("Sigiro evidence SHA-256 mismatch");
    this.name = "EvidenceDigestMismatchError";
  }
}

/** Trusted supervisor commands only: never pass model-provided commands here. */
async function command(binary: string, args: string[], input?: string): Promise<{ code: number; stdout: string; stderr: string }> {
  return await new Promise((resolve, reject) => {
    let inputError: Error | undefined;
    const child = execFile(binary, args, {
      encoding: "utf8", timeout: 120_000, killSignal: "SIGKILL", maxBuffer: 4 * 1024 * 1024,
      env: { ...process.env, GIT_TERMINAL_PROMPT: "0", GH_PROMPT_DISABLED: "1" },
    }, (error, stdout, stderr) => {
      if (inputError !== undefined) reject(error === null ? inputError : new AggregateError([inputError, error], `${binary} input failed`, { cause: inputError }));
      else if (error === null) resolve({ code: 0, stdout, stderr });
      else if (typeof error.code === "number" && !error.killed) resolve({ code: error.code, stdout, stderr });
      else reject(error);
    });
    // execFile owns output limits, timeout, termination, and close ordering.
    child.stdin!.on("error", (error: Error) => { inputError = error; child.kill("SIGKILL"); });
    child.stdin!.end(input);
  });
}

async function checked(binary: string, args: string[]): Promise<string> {
  const result = await command(binary, args);
  if (result.code !== 0) throw new Error(`${binary} failed (${result.code}): ${result.stderr.trim()}`);
  return result.stdout;
}

export function assertRepository(repository: string): void {
  const configured = process.env.SIGIRO_REPOSITORY;
  if (!configured || repository !== configured) throw new Error("Job repository does not match SIGIRO_REPOSITORY");
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) throw new Error("Invalid repository");
}

/** Evidence is a bounded local artifact created by the trusted Sigiro collector. */
export async function verifiedEvidence(job: Pick<CelldJob, "repository" | "evidenceUri" | "evidenceSha256">): Promise<Buffer> {
  assertRepository(job.repository);
  const configured = process.env.SIGIRO_EVIDENCE_DIR;
  if (!configured) throw new Error("SIGIRO_EVIDENCE_DIR is required");
  const uri = new URL(job.evidenceUri);
  if (uri.protocol !== "file:" || uri.hostname !== "") throw new Error("Evidence must use a local file URI");
  const directory = await realpath(configured);
  const path = await realpath(fileURLToPath(uri));
  const child = relative(directory, path);
  if (!child || child === ".." || child.startsWith("../") || isAbsolute(child)) throw new Error("Evidence is outside SIGIRO_EVIDENCE_DIR");
  await using handle = await open(path, "r");
  const info = await handle.stat();
  if (!info.isFile() || info.size > MAX_EVIDENCE_BYTES) throw new Error("Evidence must be a regular file of at most 1 MiB");
  const buffer = Buffer.alloc(MAX_EVIDENCE_BYTES + 1);
  let size = 0;
  while (size < buffer.length) {
    const read = await handle.read(buffer, size, buffer.length - size, null);
    if (read.bytesRead === 0) break;
    size += read.bytesRead;
  }
  if (size > MAX_EVIDENCE_BYTES) throw new Error("Evidence exceeded 1 MiB while reading");
  const bytes = buffer.subarray(0, size);
  if (createHash("sha256").update(bytes).digest("hex") !== job.evidenceSha256) throw new EvidenceDigestMismatchError();
  const evidence: unknown = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  if (typeof evidence !== "object" || evidence === null || !("service" in evidence) || !("from_ts" in evidence) || !("to_ts" in evidence) || !("diagnosis" in evidence)) {
    throw new Error("Sigiro evidence requires service, from_ts, to_ts, and diagnosis");
  }
  return bytes;
}

/** Clone on the trusted host; only a credential-free git bundle enters Docker. */
export async function prepareWorkspace(job: CelldJob, sandbox: SandboxSession): Promise<void> {
  if (typeof AsyncDisposableStack === "undefined") throw new Error("Sigiro's Eve runtime requires Node.js 24+ or Bun 1.3.14+ for resource disposal");
  const evidence = await verifiedEvidence(job);
  await checked("git", ["check-ref-format", "--branch", job.baseRef]);
  await using cleanup = new AsyncDisposableStack();
  const temporary = await mkdtemp(join(tmpdir(), "sigiro-eve-checkout-"));
  cleanup.defer(() => rm(temporary, { recursive: true, force: true }));
  const repository = join(temporary, "repository.git");
  await checked("git", ["-c", "credential.helper=", "-c", "credential.helper=!gh auth git-credential", "clone", "--bare", "--no-tags", "--single-branch", "--branch", job.baseRef, `https://github.com/${job.repository}.git`, repository]);
  await checked("git", ["-C", repository, "cat-file", "-e", `${job.baseSha}^{commit}`]);
  await checked("git", ["-C", repository, "update-ref", "refs/heads/sigiro-job", job.baseSha]);
  const bundle = join(temporary, "repository.bundle");
  await checked("git", ["-C", repository, "bundle", "create", bundle, "refs/heads/sigiro-job"]);
  if ((await stat(bundle)).size > MAX_BUNDLE_BYTES) throw new Error("Repository bundle exceeds 64 MiB");
  await sandbox.writeBinaryFile({ path: "/workspace/repository.bundle", content: await readFile(bundle) });
  await sandbox.writeBinaryFile({ path: "/workspace/sigiro-evidence.json", content: evidence });
  const result = await sandbox.run({ command: `test ! -e ${quote([CHECKOUT_PATH])} && git clone --no-checkout /workspace/repository.bundle ${quote([CHECKOUT_PATH])} && git -C ${quote([CHECKOUT_PATH])} checkout --detach ${quote([job.baseSha])} && git -C ${quote([CHECKOUT_PATH])} remote remove origin && test "$(git -C ${quote([CHECKOUT_PATH])} rev-parse HEAD)" = ${quote([job.baseSha])} && rm /workspace/repository.bundle` });
  if (result.exitCode !== 0) throw new Error(`Sandbox checkout failed (${result.exitCode}): ${result.stderr}`);
}

/** Existing publisher transport; the host's gh credential never reaches the agent. */
export function githubClient(repository: string): Pick<GitHubHandle, "request" | "repository"> {
  assertRepository(repository);
  const [owner, name] = repository.split("/") as [string, string];
  return {
    repository: { owner, name, fullName: repository, id: 0, private: true },
    async request<T>({ method, path, body }: Parameters<GitHubHandle["request"]>[0]) {
      const prefix = `/repos/${repository}/`;
      if (!path.startsWith(prefix)) throw new Error("GitHub publisher request escaped its configured repository");
      const result = await command("gh", ["api", "--hostname", "github.com", "--include", "--method", method, path, ...(body === undefined ? [] : ["--input", "-"])], body === undefined ? undefined : JSON.stringify(body));
      const match = /^HTTP\/\S+ (\d+) [^\n]*\r?\n[\s\S]*?\r?\n\r?\n([\s\S]*)$/.exec(result.stdout);
      if (!match) throw new Error(`gh api returned an invalid HTTP response (${result.code}): ${result.stderr.trim()}`);
      const status = Number(match[1]);
      const parsed: unknown = match[2]?.trim() ? JSON.parse(match[2]) : null;
      if (status < 200 || status >= 300) throw new GitHubApiError({ body: parsed, method, path, status });
      if (result.code !== 0) throw new Error(`gh api failed (${result.code}): ${result.stderr.trim()}`);
      return { body: parsed as T, ok: true, status };
    },
  };
}
