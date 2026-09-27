/**
 * Announce this site's URLs to IndexNow (Bing, Yandex, Seznam, Naver).
 *
 * Why this is a script and not an Astro integration. `astro-indexnow` exists and
 * submits "at build time", which is the wrong trigger here: `aube run build` and
 * `aube run deploy` are separate steps, so a build-time submission announces URLs
 * that are not live yet, and announces them again for every build that is never
 * deployed. IndexNow rate-limits and de-prioritises noisy hosts, so submitting a
 * URL before it changed is not free. This runs after a deploy instead.
 *
 * Why the URL list comes from the live sitemap rather than from `dist/`. The
 * point of the call is "these URLs changed on the origin". Reading the origin's
 * own sitemap means we can only ever announce URLs that are actually being
 * served — if the deploy half-failed, the sitemap says so and we announce less,
 * rather than lying to Bing about pages that 404.
 *
 * The key file at `public/<key>.txt` is a plain static asset. It must stay
 * reachable at the site root: IndexNow fetches `https://sigiro.com/<key>.txt`
 * and refuses the submission unless its body is exactly the key. `public/` files
 * land in `dist/client/` and `wrangler.jsonc`'s `run_worker_first` covers only
 * `/` and `/docs*`, so the key is served by the asset layer and never touches
 * the Worker. This is the same reason `robots.txt` is a static asset here.
 */

const HOST = "sigiro.com";
const KEY = "1fcd9653aa5a4e26d24e76ecb4e1d3c1";
const SITEMAP = `https://${HOST}/sitemap-0.xml`;
const ENDPOINT = "https://api.indexnow.org/IndexNow";

async function readSitemapUrls(): Promise<string[]> {
  const res = await fetch(SITEMAP);
  if (!res.ok) throw new Error(`sitemap ${SITEMAP} returned ${res.status}`);
  const xml = await res.text();
  const urls = [...xml.matchAll(/<loc>([^<]+)<\/loc>/g)].map((m) => m[1]!);
  if (urls.length === 0) throw new Error("sitemap parsed to zero URLs");
  return urls;
}

/**
 * The key file is fetched by IndexNow itself, not by us — but a submission with
 * an unreachable key fails with a 403 that does not say which half was wrong.
 * Checking it here turns that into a legible error before we send anything.
 */
async function assertKeyIsLive(): Promise<void> {
  const url = `https://${HOST}/${KEY}.txt`;
  const res = await fetch(url);
  if (!res.ok)
    throw new Error(
      `key file ${url} returned ${res.status} — deploy it before submitting`
    );
  const body = (await res.text()).trim();
  if (body !== KEY)
    throw new Error(
      `key file body is ${JSON.stringify(body)}, expected ${JSON.stringify(KEY)}`
    );
}

/**
 * Drop any URL the origin does not actually serve.
 *
 * Cloudflare's own Crawler Hints does this — its docs say a response of 4xx or
 * worse is never reported to IndexNow — and the reason generalises: announcing a
 * dead URL spends crawl budget to have a crawler discover a 404, and repeatedly
 * announcing dead URLs is how a host gets de-prioritised. The sitemap is
 * generated at build time, so it describes what the build *intended* to publish;
 * this checks what the origin is serving right now.
 *
 * Bad URLs are dropped rather than fatal, so one retired page cannot block the
 * announcement of fourteen good ones. They are printed, because a non-200 in our
 * own sitemap is a defect worth seeing rather than swallowing. HEAD, because the
 * status is the whole question and the bodies here run to 87 KB.
 */
async function keepLiveUrls(urls: string[]): Promise<string[]> {
  const checked = await Promise.all(
    urls.map(async (url) => {
      try {
        const res = await fetch(url, { method: "HEAD", redirect: "manual" });
        return {
          url,
          status: res.status,
          ok: res.status >= 200 && res.status < 400,
        };
      } catch (cause) {
        const message = cause instanceof Error ? cause.message : String(cause);
        return { url, status: `unreachable (${message})`, ok: false };
      }
    })
  );

  for (const c of checked)
    if (!c.ok) console.warn(`  skipped ${c.url} -> ${c.status}`);
  return checked.filter((c) => c.ok).map((c) => c.url);
}

async function main(): Promise<void> {
  const cliUrls = process.argv.slice(2);
  await assertKeyIsLive();
  const candidates = cliUrls.length > 0 ? cliUrls : await readSitemapUrls();

  const urlList = await keepLiveUrls(candidates);
  const skipped = candidates.length - urlList.length;
  if (urlList.length === 0)
    throw new Error(
      `no live URLs to submit (all ${candidates.length} failed their status check)`
    );
  if (skipped > 0)
    console.warn(
      `warning: ${skipped} of ${candidates.length} URLs are not serving and were not submitted`
    );

  const res = await fetch(ENDPOINT, {
    method: "POST",
    headers: { "content-type": "application/json; charset=utf-8" },
    body: JSON.stringify({
      host: HOST,
      key: KEY,
      keyLocation: `https://${HOST}/${KEY}.txt`,
      urlList,
    }),
  });

  const text = await res.text();
  console.log(
    `submitted ${urlList.length} URLs -> ${res.status} ${res.statusText}`
  );
  if (text) console.log(text);

  // 200 accepted, 202 accepted-pending-key-validation. Anything else is a real
  // failure and must fail the deploy step rather than pass silently.
  if (res.status !== 200 && res.status !== 202) process.exit(1);
}

await main();
