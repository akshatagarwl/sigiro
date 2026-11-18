// Restrict the loopback relay to known, bounded telemetry endpoints. A
// Pyroscope name may include SDK-generated labels; the OAuth token is scoped
// to the one tenant, while these checks keep requests from becoming an open
// path/query proxy.
export function validPyroscopeContentType(value) {
  return (
    typeof value === "string" &&
    /^multipart\/form-data;\s*boundary="?[a-zA-Z0-9'()+_,.\/:=?-]{1,70}"?$/i.test(
      value
    )
  );
}

// Sigiro's Transform admission is intentionally non-queuing (HTTP 429).
// Pyroscope does not retry failed uploads; the relay retries only this
// authenticated profile path, twice with a small, fixed, bounded delay.
export async function retryProfileAdmission(
  path,
  result,
  request,
  sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
) {
  if (!path.startsWith("/ingest?")) return result;
  for (const delay of [250, 750]) {
    if (result.status !== 429) break;
    await result.body?.cancel();
    await sleep(delay);
    result = await request();
  }
  return result;
}

export function relayTarget(raw) {
  if (
    typeof raw !== "string" ||
    !raw.startsWith("/") ||
    raw.startsWith("//") ||
    raw.length > 1024
  )
    return null;
  if (/^\/v1\/(traces|logs|metrics|profiles)$/.test(raw)) {
    return { path: raw, counter: raw.slice(4) };
  }
  let url;
  try {
    url = new URL(raw, "http://localhost");
  } catch {
    return null;
  }
  if (
    url.origin !== "http://localhost" ||
    url.pathname !== "/ingest" ||
    url.hash
  )
    return null;
  const query = url.searchParams;
  const allowed = new Set(["from", "until", "name", "spyName", "sampleRate"]);
  if ([...query.keys()].some((key) => !allowed.has(key))) return null;
  if (
    ["from", "until", "name", "spyName"].some(
      (key) => query.getAll(key).length !== 1
    )
  )
    return null;
  if (query.getAll("sampleRate").length > 1) return null;
  if (
    !/^[0-9]{1,16}$/.test(query.get("from")) ||
    !/^[0-9]{1,16}$/.test(query.get("until"))
  )
    return null;
  const name = query.get("name");
  if (!name || name.length > 256 || /[\u0000-\u001f\u007f]/.test(name))
    return null;
  if (!/^[a-z0-9_-]{1,32}$/.test(query.get("spyName"))) return null;
  if (query.has("sampleRate") && !/^[0-9]{1,8}$/.test(query.get("sampleRate")))
    return null;
  return { path: `/ingest${url.search}`, counter: "profiles" };
}
