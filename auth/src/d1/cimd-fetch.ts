import type { ClientMetadataResourceFetch } from "@better-auth/oauth-provider";

/**
 * Workers-native CIMD (client metadata document) fetch.
 *
 * Replaces `@better-auth/cimd/node`, which cannot run here: it resolves the
 * target with node:dns, checks the resolved addresses against private ranges,
 * and then pins the connection to the address it validated. Workers has no
 * node:dns and no socket-level control, so that transport fails at import.
 *
 * SECURITY NOTE \u2014 this is a genuine reduction, not a like-for-like port. The
 * Node transport's defence was DNS-pinning: validate the address, then connect
 * to *that address*, closing the DNS-rebinding window between check and
 * connect. Here the platform owns DNS resolution and the check can only be
 * applied to the literal URL. Cloudflare's runtime does block requests to
 * private address space from Workers, so the SSRF surface is narrowed by the
 * platform rather than by this code \u2014 but a rebinding attack that resolves
 * public-then-private is not defended by the checks below.
 *
 * The syntactic checks are still worth keeping: they reject the obvious cases
 * cheaply and make intent explicit.
 */

const BLOCKED_HOSTNAMES = new Set(["localhost", "metadata.google.internal"]);

/** Literal addresses that must never be fetched, checked before any request. */
const isBlockedHost = (hostname: string): boolean => {
  const host = hostname.toLowerCase().replace(/^\[|\]$/g, "");
  if (BLOCKED_HOSTNAMES.has(host)) return true;
  if (host.endsWith(".localhost") || host.endsWith(".internal")) return true;
  // IPv4 literals in private / loopback / link-local / CGNAT ranges.
  if (/^127\./.test(host) || /^10\./.test(host) || /^192\.168\./.test(host)) return true;
  if (/^172\.(1[6-9]|2\d|3[01])\./.test(host)) return true;
  if (/^169\.254\./.test(host) || /^100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\./.test(host)) return true;
  if (/^0\./.test(host)) return true;
  // IPv6 loopback, link-local, and unique-local.
  if (host === "::1" || host === "::" ) return true;
  if (host.startsWith("fe80:") || host.startsWith("fc") || host.startsWith("fd")) return true;
  return false;
};

const MAX_BYTES = 256 * 1024;

export const fetchClientMetadataResource: ClientMetadataResourceFetch = async (url) => {
  const target = new URL(typeof url === "string" ? url : url.toString());
  if (target.protocol !== "https:") throw new Error("client metadata must be served over https");
  if (isBlockedHost(target.hostname)) throw new Error("client metadata host is not permitted");

  const response = await fetch(target, {
    method: "GET",
    headers: { accept: "application/json" },
    redirect: "error", // a followed redirect is an unchecked second destination
    signal: AbortSignal.timeout(10_000),
  });
  if (!response.ok) throw new Error(`client metadata fetch failed: ${response.status}`);

  // Bound the body. An unbounded read of an attacker-influenced URL is a memory
  // exhaustion vector, and the Node transport capped this too.
  const body = await response.text();
  if (body.length > MAX_BYTES) throw new Error("client metadata document is too large");
  return JSON.parse(body) as never;
};
