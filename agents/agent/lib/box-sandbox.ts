import { asciiBox, BoxHttpClient } from "@asciidev/eve-box";
import type { SandboxBackend, SandboxBackendHandle, SandboxDeleteOptions } from "eve/sandbox";

/**
 * Adapts `@asciidev/eve-box` to Eve 0.51.
 *
 * The published adapter targets Eve 0.11, whose backend handle exposed a single
 * `dispose()`. Eve 0.51 requires `delete()`, `stop()`, and `shutdown()` instead.
 * Its type declarations still assert conformance, so the gap only appears at run
 * time when the runtime calls a method the shipped class never defined.
 */

const DEFAULT_BASE_URL = "https://ascii.dev/api/box/v1";

export interface BoxSandboxOptions {
  /** Box API key. Defaults to `BOX_API_KEY`. */
  readonly apiKey?: string;
  readonly baseUrl?: string;
  /** Per-box environment. The only way to pass secrets while `noEnv` holds. */
  readonly env?: Record<string, string>;
  /** Auto-stop TTL in seconds. `null` disables it. */
  readonly ttlSeconds?: number | null;
  /** Injected for tests. Every Box call routes through it. */
  readonly fetchImpl?: typeof fetch;
}

/** Box archives asynchronously, so a delete is accepted rather than completed. */
async function deleteBox(
  fetchImpl: typeof fetch,
  baseUrl: string,
  apiKey: string,
  boxId: string,
  options?: SandboxDeleteOptions,
): Promise<void> {
  const request: RequestInit = {
    method: "DELETE",
    headers: { Authorization: `Bearer ${apiKey}`, "X-Ascii-Confirm-Delete": boxId },
  };
  if (options?.abortSignal !== undefined) request.signal = options.abortSignal;
  const response = await fetchImpl(`${baseUrl}/boxes/${encodeURIComponent(boxId)}`, request);
  if (!response.ok) throw new Error(`Box delete failed for ${boxId}: HTTP ${response.status}`);
}

/** `captureState` is the adapter's only public route to the box id. */
async function boxIdOf(handle: SandboxBackendHandle): Promise<string> {
  const state = await handle.captureState();
  const boxId = state.metadata["boxId"];
  if (typeof boxId !== "string" || boxId.length === 0) throw new Error("Box sandbox handle carries no box id");
  return boxId;
}

/**
 * Runs Eve sandboxes on Ascii Box.
 *
 * `noEnv` is fixed on: the operator's Box account secrets, secret files, GitHub
 * credentials, SSH identity, and selected repositories must never reach a box
 * running model-authored code. Pass what a job legitimately needs through `env`.
 *
 * Box cannot enforce Eve network policies, so sandbox code reaches the internet.
 * The host GitHub credential stays on the host: `prepareWorkspace` clones there
 * and transfers a credential-free git bundle.
 */
export function boxSandbox(options: BoxSandboxOptions = {}): SandboxBackend {
  const apiKey = options.apiKey ?? process.env.BOX_API_KEY;
  if (!apiKey) throw new Error("BOX_API_KEY is required to run agent sandboxes on Ascii Box");
  const baseUrl = (options.baseUrl ?? DEFAULT_BASE_URL).replace(/\/$/, "");
  const fetchImpl = options.fetchImpl ?? fetch;

  const client = new BoxHttpClient({ apiKey, baseUrl, fetchImpl });
  const inner = asciiBox({
    client,
    apiKey,
    baseUrl,
    noEnv: true,
    ...(options.env === undefined ? {} : { env: options.env }),
    ...(options.ttlSeconds === undefined ? {} : { ttlSeconds: options.ttlSeconds }),
  }) as SandboxBackend;

  return {
    name: inner.name,
    prewarm: (input) => inner.prewarm(input),
    async create(input) {
      const handle = await inner.create(input);
      return {
        session: handle.session,
        useSessionFn: handle.useSessionFn,
        captureState: () => handle.captureState(),
        // Stop snapshots the disk and pauses billing. The adapter's `create`
        // resumes from the persisted box id, so the session stays reattachable.
        stop: async () => { await client.stop(await boxIdOf(handle)); },
        shutdown: async () => { await client.stop(await boxIdOf(handle)); },
        delete: async (deleteOptions?: SandboxDeleteOptions) => {
          await deleteBox(fetchImpl, baseUrl, apiKey, await boxIdOf(handle), deleteOptions);
        },
      };
    },
  };
}
