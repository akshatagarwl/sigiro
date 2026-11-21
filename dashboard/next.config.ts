import type { NextConfig } from "next";
import { PHASE_DEVELOPMENT_SERVER } from "next/constants";

/**
 * A static export served at sigiro.com/dashboard by the `Dashboard` Worker in
 * www/alchemy.run.ts. Everything runs in the browser against same-origin
 * `/v1/query` and `/api/auth`, so there is no Next server.
 */
const nextConfig: NextConfig = {
  output: "export",
  basePath: "/dashboard",
  trailingSlash: true,
  images: { unoptimized: true },
  /**
   * Next picks a random build ID per build, which alone made two builds of one
   * commit differ. `SIGIRO_DASHBOARD_BUILD_ID` pins it: the Nix build passes
   * the hash of the dashboard source (nix/dashboard.nix). It must change when
   * the content does, because `_next/static/<id>/` is served immutable
   * (src/dashboard.rs). Unset, Next keeps its random ID (`null`).
   */
  generateBuildId: async () => process.env.SIGIRO_DASHBOARD_BUILD_ID ?? null,
};

export default function config(phase: string): NextConfig {
  if (phase !== PHASE_DEVELOPMENT_SERVER) return nextConfig;
  // Keep API requests same-origin in development too. This is a local backend,
  // not a proxy to production OAuth or production customer telemetry.
  const backend = process.env.SIGIRO_DEV_BACKEND ?? "http://127.0.0.1:9999";
  const url = new URL(backend);
  if (
    !["http:", "https:"].includes(url.protocol) ||
    url.username ||
    url.password ||
    url.search ||
    url.hash ||
    url.pathname !== "/"
  )
    throw new Error(
      "SIGIRO_DEV_BACKEND must be an HTTP(S) origin without credentials or a path"
    );
  return {
    ...nextConfig,
    output: undefined,
    skipTrailingSlashRedirect: true,
    async rewrites() {
      return [
        {
          source: "/v1/:path*",
          destination: `${url.origin}/v1/:path*`,
          basePath: false,
        },
      ];
    },
  };
}
