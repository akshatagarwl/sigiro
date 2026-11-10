import type { APIRoute } from "astro";
import { env } from "cloudflare:workers";

/**
 * Release artifacts, streamed out of R2.
 *
 * The bucket is private and there is no public bucket URL. Everything a reader
 * downloads comes through here, which is what lets the artifacts live on
 * sigiro.com without a second hostname — this deploy path has no DNS scope, so a
 * `dl.sigiro.com` custom domain is not available to it.
 *
 * Keys are `LATEST`, `<version>/SHA256SUMS` and
 * `<version>/sigiro-<version>-<target>.tar.gz`. The installer at /install builds
 * all three. `nightly/` has the same shape with `nightly` as the version, so
 * `SIGIRO_VERSION=nightly` installs the last build of main. sigiro-infra is the
 * only writer.
 */

/**
 * The one route on this site that is not prerendered, and the only reason an
 * adapter is configured at all. Everything else is a static file.
 */
export const prerender = false;

/**
 * The key charset, enforced before R2 sees it.
 *
 * R2 does not resolve `..` — a key is a literal string, not a path — so this is
 * not traversal defence. It is there so a malformed request fails as a 400 at the
 * edge of the system rather than as a confusing 404 from storage, and so the set of
 * things this route can ever ask for stays small enough to read.
 */
const KEY = /^[A-Za-z0-9][A-Za-z0-9._/-]{0,127}$/;

/** No `..` anywhere, whatever else the charset allows. */
const isSafe = (key: string) => KEY.test(key) && !key.includes("..");

export const GET: APIRoute = async ({ params }) => {
  const key = params.key ?? "";

  if (!isSafe(key)) return new Response("bad request\n", { status: 400 });

  // `cloudflare:workers` and not `Astro.locals.runtime.env`, which was tried first
  // and answers with a 500 that says so: the adapter removed that accessor in Astro
  // v6 and points at this import instead. It is the same import the Next route
  // handler used, and it types against `worker-configuration.d.ts`.
  const object = await env.DOWNLOADS.get(key);
  if (!object) return new Response("not found\n", { status: 404 });

  const headers = new Headers();
  object.writeHttpMetadata(headers);
  headers.set("etag", object.httpEtag);
  headers.set("content-length", String(object.size));

  /**
   * Two things are mutable: `LATEST`, which names the current version, and
   * `nightly/`, which the nightly build overwrites. A cached copy of either is
   * a reader pinned to an old build. Everything else is addressed by a released
   * version and can never change, so it gets a year and `immutable`.
   */
  headers.set(
    "cache-control",
    key === "LATEST" || key.startsWith("nightly/")
      ? "public, max-age=0, s-maxage=60, stale-while-revalidate=300"
      : "public, max-age=31536000, immutable"
  );

  return new Response(object.body, { headers });
};
