import { Sandbox as BaseSandbox, getSandbox } from "@cloudflare/sandbox";
import { bridge, WarmPool as BaseWarmPool } from "@cloudflare/sandbox/bridge";

export class Sandbox extends BaseSandbox {
  enableInternet = false;
  sleepAfter = "1h";
}

export class WarmPool extends BaseWarmPool {
  async assignedSandboxIds(): Promise<string[]> {
    const stored: unknown = await this.ctx.storage.get("assignments");
    const entries = stored instanceof Map ? [...stored.keys()] : Array.isArray(stored) ? stored.map((item: unknown) => Array.isArray(item) ? item[0] : null) : [];
    if (!entries.every((id) => typeof id === "string" && /^[a-z2-7]{1,128}$/.test(id))) throw new Error("Invalid saved sandbox assignment");
    return entries as string[];
  }
}

const handler = bridge({});
type BridgeEnvironment = { SANDBOX_API_KEY?: string; Sandbox: DurableObjectNamespace<Sandbox>; WarmPool: DurableObjectNamespace<WarmPool> };
export default {
  async fetch(request: Request<unknown, IncomingRequestCfProperties>, env: BridgeEnvironment, ctx: ExecutionContext): Promise<Response> {
    if (!env.SANDBOX_API_KEY || env.SANDBOX_API_KEY.length < 32) {
      return new Response("Sandbox bridge is not configured", { status: 503 });
    }
    const pathname = new URL(request.url).pathname;
    if (pathname.startsWith("/v1/openapi")) {
      return new Response("Not found", { status: 404 });
    }
    const match = /^\/v1\/sandbox\/([a-z2-7]{1,128})$/.exec(pathname);
    if (pathname === "/v1/operator/assignments" || (request.method === "DELETE" && match)) {
      if (request.headers.get("authorization") !== `Bearer ${env.SANDBOX_API_KEY}`) {
        return new Response("Unauthorized", { status: 401 });
      }
      const pool = env.WarmPool.get(env.WarmPool.idFromName("global-pool"));
      if (pathname === "/v1/operator/assignments" && request.method === "GET") {
        return Response.json({ ids: await pool.assignedSandboxIds() });
      }
      if (!match) return new Response("Method not allowed", { status: 405 });
      const containerId = await pool.lookupContainer(match[1]!);
      if (containerId) {
        await getSandbox(env.Sandbox, containerId).destroy();
        await pool.reportStopped(containerId);
      }
      return new Response(null, { status: 204 });
    }
    return handler.fetch!(request, env, ctx);
  },
};
