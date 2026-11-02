import type { NextConfig } from "next";

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
};

export default nextConfig;
