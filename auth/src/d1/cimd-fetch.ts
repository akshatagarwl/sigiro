import type { ClientMetadataResourceFetch } from "@better-auth/oauth-provider";

import {
  FIRST_PARTY_CLIENT_ID,
  FIRST_PARTY_CLIENT_METADATA_JSON,
} from "../client-metadata.js";
import { OPTIMIZER_CLIENT_ID, OPTIMIZER_CLIENT_METADATA_JSON } from "../optimizer-client.js";

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
  if (/^127\./.test(host) || /^10\./.test(host) || /^192\.168\./.test(host))
    return true;
  if (/^172\.(1[6-9]|2\d|3[01])\./.test(host)) return true;
  if (
    /^169\.254\./.test(host) ||
    /^100\.(6[4-9]|[7-9]\d|1[01]\d|12[0-7])\./.test(host)
  )
    return true;
  if (/^0\./.test(host)) return true;
  // IPv6 loopback, link-local, and unique-local.
  if (host === "::1" || host === "::") return true;
  if (
    host.startsWith("fe80:") ||
    host.startsWith("fc") ||
    host.startsWith("fd")
  )
    return true;
  return false;
};

const MAX_BYTES = 256 * 1024;

/**
 * The caller (`@better-auth/cimd`) passes its own `RequestInit` as a second
 * argument, carrying conditional-revalidation headers (`If-None-Match`,
 * `If-Modified-Since`), an abort signal for its 5s budget, and
 * `redirect: "error"`.
 *
 * Two of those must be honoured and one must not:
 *
 *   - The headers and signal MUST be forwarded. Dropping the headers turns
 *     every revalidation into a full refetch; dropping the signal means the
 *     library's timeout cannot cancel the request.
 *   - `redirect: "error"` MUST be overridden. Workers does not implement it and
 *     throws `TypeError: Invalid redirect value` before the request is issued.
 *     The library wraps the call in `catch {}` and reports the generic
 *     `invalid_client` / "Failed to fetch metadata document", so the real error
 *     never surfaces — which is exactly how this stayed invisible.
 *
 * `manual` preserves the intent: a redirect comes back as an opaque 3xx that we
 * reject, instead of being followed to an unchecked second destination.
 */
export const fetchClientMetadataResource: ClientMetadataResourceFetch = async (
  url,
  init
) => {
  try {
    // A Worker cannot fetch a hostname served by another Worker on the same
    // zone over the public internet: Cloudflare refuses the loopback with
    // `403 error code: 1014` ("CNAME Cross-User Banned"). The CLI's client_id
    // is our own first-party document, so the request never needs to leave the
    // isolate. Serve it from the shared module instead — this is both the fix
    // and strictly less work than a public round-trip.
    //
    // Deliberately an exact match on the canonical first-party client_id. Any
    // other URL, including another path on the same origin, still goes through
    // the full validated network path below.
    if (String(url) === FIRST_PARTY_CLIENT_ID || String(url) === OPTIMIZER_CLIENT_ID) {
      return new Response(String(url) === FIRST_PARTY_CLIENT_ID
        ? FIRST_PARTY_CLIENT_METADATA_JSON : OPTIMIZER_CLIENT_METADATA_JSON, {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    }
    return await fetchClientMetadata(url, init);
  } catch (error) {
    // `@better-auth/cimd` catches everything this throws and reports one
    // generic `invalid_client` / "Failed to fetch metadata document (network
    // error or redirect blocked)". That collapses a rejected fetch option, a
    // refused redirect, a blocked host and a genuine outage into the same
    // opaque string, which is why a 304 handled in the wrong order took four
    // attempts to find. Log the real cause before it is swallowed.
    console.error("cimd: client metadata fetch failed", {
      url: String(url),
      error:
        error instanceof Error
          ? `${error.name}: ${error.message}`
          : String(error),
    });
    throw error;
  }
};

const fetchClientMetadata: ClientMetadataResourceFetch = async (url, init) => {
  const target = new URL(typeof url === "string" ? url : url.toString());
  if (target.protocol !== "https:")
    throw new Error("client metadata must be served over https");
  if (isBlockedHost(target.hostname))
    throw new Error("client metadata host is not permitted");

  const headers = new Headers(init?.headers);
  if (!headers.has("accept")) headers.set("accept", "application/json");

  const response = await fetch(target, {
    method: "GET",
    headers,
    redirect: "manual",
    signal: init?.signal ?? AbortSignal.timeout(10_000),
  });
  // 304 MUST be checked before the redirect range. It is a conditional
  // response, not a redirect: the caller sends `If-None-Match` from its cache
  // entry and joins the 304 back onto it. Checking `>= 300` first swallowed
  // every revalidation and reported it as a blocked redirect, which is the
  // generic `invalid_client` the plugin surfaces.
  if (response.status === 304) return response;
  // A real redirect (301/302/307/308), or an opaque one on runtimes that
  // report `manual` that way, is refused: following it would fetch client
  // metadata from an unchecked second destination.
  if (
    response.type === "opaqueredirect" ||
    (response.status >= 300 && response.status < 400)
  ) {
    throw new Error(
      `client metadata must not redirect (got ${response.status})`
    );
  }
  if (!response.ok) {
    // Include a snippet of the body: a 403 from an edge/WAF layer names itself
    // there, and without it the status alone is indistinguishable from an
    // origin refusal.
    const detail = (await response.text().catch(() => ""))
      .slice(0, 200)
      .replace(/\s+/g, " ")
      .trim();
    throw new Error(
      `client metadata fetch failed: ${response.status}${detail ? ` — ${detail}` : ""}`
    );
  }

  // Do NOT read the body here. The caller streams it itself with its own byte
  // limit (`readBodyWithLimit`), and an earlier version of this function read
  // the text and returned `new Response(body, { headers: response.headers })`.
  // That copies `content-length` (and any `content-encoding`) from the
  // compressed upstream response onto a body that has already been decoded, so
  // the caller's reader disagreed with the declared length and threw — which
  // the plugin reports as the same generic "failed to fetch" as a network
  // error. Enforce the size limit from the declared length and hand back the
  // untouched response.
  const declared = response.headers.get("content-length");
  if (declared !== null && Number.parseInt(declared, 10) > MAX_BYTES) {
    await response.body?.cancel();
    throw new Error("client metadata document is too large");
  }
  return response;
};
