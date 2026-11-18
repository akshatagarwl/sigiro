import { renderDeclarations } from "@earendil-works/pi-codemode/declarations";
import { parseCodemodeSource } from "@earendil-works/pi-codemode/source";
import { WorkerEntrypoint } from "cloudflare:workers";

const TOOLS = [
  {
    name: "query",
    description: "Run a tenant telemetry query",
    inputSchema: {
      type: "object",
      properties: { sql: { type: "string" } },
      required: ["sql"],
    },
    outputSchema: { type: "array" },
  },
  {
    name: "secret_probe",
    description: "Echo the tenant",
    inputSchema: { type: "object", properties: {} },
  },
];

export class Tools extends WorkerEntrypoint {
  async call(name, args) {
    const { session, tenant } = this.ctx.props;
    if (name === "query") return [{ tenant, session, sql: args.sql, rows: 3 }];
    if (name === "secret_probe") return { tenant };
    throw new Error(`unknown tool ${name}`);
  }
}

const RUNNER = (body) => `
export default {
  async fetch(request, env) {
    const output = [];
    const text = (v) => output.push(typeof v === "string" ? v : JSON.stringify(v));
    const tools = new Proxy({}, { get: (_, name) => (args) => env.TOOLS.call(String(name), args ?? {}) });
    try {
      const value = await (async () => {
${body}
      })();
      return Response.json({ ok: true, value, output });
    } catch (error) {
      return Response.json({ ok: false, error: String(error && error.message || error), output });
    }
  }
};`;

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);
    if (url.pathname === "/declarations")
      return new Response(renderDeclarations({ tools: TOOLS }));
    const { code } = parseCodemodeSource(await request.text());
    const worker = env.LOADER.load({
      compatibilityDate: "2026-09-01",
      mainModule: "script.js",
      modules: { "script.js": RUNNER(code) },
      env: {
        TOOLS: ctx.exports.Tools({ props: { session: "s-1", tenant: "acme" } }),
      },
      globalOutbound: null,
      limits: { cpuMs: 200 },
    });
    try {
      return await worker.getEntrypoint().fetch("https://script.invalid/");
    } catch (error) {
      return Response.json(
        { ok: false, error: `runtime: ${error.message}` },
        { status: 500 }
      );
    }
  },
};
