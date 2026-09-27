import assert from "node:assert/strict";
import test from "node:test";

import { fetchClientMetadataResource } from "../src/d1/cimd-fetch.js";

/**
 * These cover the transport contract `@better-auth/cimd` depends on. The plugin
 * wraps every call in `catch {}` and reports one generic `invalid_client`
 * ("Failed to fetch metadata document"), so ANY mistake in here — a rejected
 * option, a mishandled status, a consumed body — is indistinguishable from a
 * network outage at the API boundary. The OAuth device flow was dead in
 * production for exactly this reason and nothing caught it.
 */

const withStubbedFetch = async (
  stub: (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>,
  run: () => Promise<void>
): Promise<void> => {
  const original = globalThis.fetch;
  globalThis.fetch = stub as typeof globalThis.fetch;
  try {
    await run();
  } finally {
    globalThis.fetch = original;
  }
};

void test("returns a 304 to the caller instead of treating it as a redirect", async () => {
  // The caller sends If-None-Match from its cache entry and joins the 304 back
  // onto the cached document. 304 is inside the 3xx range, so a naive
  // `status >= 300` redirect guard swallows every revalidation — which is the
  // bug that broke the device flow: the FIRST fetch succeeded, and the cached
  // revalidation on every subsequent request failed.
  await withStubbedFetch(
    () => Promise.resolve(new Response(null, { status: 304 })),
    async () => {
      const response = await fetchClientMetadataResource(
        "https://client.example/.well-known/oauth-client.json",
        {
          headers: new Headers({ "If-None-Match": '"abc"' }),
        }
      );
      assert.equal(response.status, 304);
    }
  );
});

void test("refuses a real redirect", async () => {
  await withStubbedFetch(
    () =>
      Promise.resolve(
        new Response(null, {
          status: 302,
          headers: { location: "https://evil.example/" },
        })
      ),
    async () => {
      await assert.rejects(
        async () =>
          void (await fetchClientMetadataResource(
            "https://client.example/.well-known/oauth-client.json"
          )),
        /must not redirect \(got 302\)/
      );
    }
  );
});

void test("never requests redirect:'error', which Workers does not implement", async () => {
  // Workers throws `TypeError: Invalid redirect value` before issuing the
  // request. The caller passes `redirect: "error"` in its own init, so this
  // function must override it rather than forward it.
  let seen: RequestRedirect | undefined;
  await withStubbedFetch(
    (_input, init) => {
      seen = init?.redirect;
      return Promise.resolve(
        new Response("{}", {
          status: 200,
          headers: { "content-type": "application/json" },
        })
      );
    },
    async () => {
      await fetchClientMetadataResource(
        "https://client.example/.well-known/oauth-client.json",
        {
          redirect: "error",
          headers: new Headers({ Accept: "application/json" }),
        }
      );
    }
  );
  assert.equal(seen, "manual");
});

void test("forwards the caller's conditional headers and abort signal", async () => {
  // Dropping If-None-Match turns every revalidation into a full refetch;
  // dropping the signal means the caller's 5s timeout cannot cancel anything.
  const controller = new AbortController();
  let seenEtag: string | null = null;
  let seenSignal: AbortSignal | null | undefined;
  await withStubbedFetch(
    (_input, init) => {
      seenEtag = new Headers(init?.headers).get("If-None-Match");
      seenSignal = init?.signal;
      return Promise.resolve(
        new Response("{}", {
          status: 200,
          headers: { "content-type": "application/json" },
        })
      );
    },
    async () => {
      await fetchClientMetadataResource(
        "https://client.example/.well-known/oauth-client.json",
        {
          headers: new Headers({ "If-None-Match": '"etag-value"' }),
          signal: controller.signal,
        }
      );
    }
  );
  assert.equal(seenEtag, '"etag-value"');
  assert.equal(seenSignal, controller.signal);
});

void test("leaves the body unread for the caller to stream", async () => {
  // The caller enforces its own byte limit with a reader. Consuming the body
  // here and re-wrapping it copied a stale content-length onto a decoded body.
  await withStubbedFetch(
    () =>
      Promise.resolve(
        new Response(
          '{"client_id":"https://sigiro.com/.well-known/oauth-client.json"}',
          {
            status: 200,
            headers: { "content-type": "application/json" },
          }
        )
      ),
    async () => {
      const response = await fetchClientMetadataResource(
        "https://client.example/.well-known/oauth-client.json"
      );
      assert.equal(response.bodyUsed, false);
      const body = (await response.json()) as { client_id: string };
      assert.equal(
        body.client_id,
        "https://sigiro.com/.well-known/oauth-client.json"
      );
    }
  );
});

void test("rejects a non-https client_id", async () => {
  await assert.rejects(
    async () =>
      void (await fetchClientMetadataResource(
        "http://sigiro.com/.well-known/oauth-client.json"
      )),
    /https/
  );
});

void test("serves the first-party client_id from memory without a network fetch", async () => {
  // Cloudflare refuses a Worker fetching its own zone with 403 error code 1014
  // ("CNAME Cross-User Banned"), which broke the OAuth device flow in
  // production. The first-party document must be answered in-isolate.
  let fetched = false;
  await withStubbedFetch(
    () => {
      fetched = true;
      return Promise.resolve(new Response(null, { status: 403 }));
    },
    async () => {
      const response = await fetchClientMetadataResource(
        "https://sigiro.com/.well-known/oauth-client.json"
      );
      assert.equal(response.status, 200);
      const body = (await response.json()) as {
        client_id: string;
        scope: string;
      };
      assert.equal(
        body.client_id,
        "https://sigiro.com/.well-known/oauth-client.json"
      );
      assert.match(body.scope, /telemetry:ingest/);
    }
  );
  assert.equal(
    fetched,
    false,
    "the first-party document must not be fetched over the network"
  );
});

void test("a different URL on the same origin still goes over the network", async () => {
  // The shortcut is an exact match, not an origin match: a third-party client
  // registering some other path must not be served our document.
  let requested: string | undefined;
  await withStubbedFetch(
    (input) => {
      requested = String(input);
      return Promise.resolve(
        new Response('{"client_id":"https://sigiro.com/other.json"}', {
          status: 200,
          headers: { "content-type": "application/json" },
        })
      );
    },
    async () => {
      await fetchClientMetadataResource("https://sigiro.com/other.json");
    }
  );
  assert.equal(requested, "https://sigiro.com/other.json");
});

void test("rejects a private or reserved host", async () => {
  await assert.rejects(
    async () =>
      void (await fetchClientMetadataResource(
        "https://127.0.0.1/.well-known/oauth-client.json"
      )),
    /not permitted/
  );
  await assert.rejects(
    async () =>
      void (await fetchClientMetadataResource(
        "https://localhost/.well-known/oauth-client.json"
      )),
    /not permitted/
  );
});
