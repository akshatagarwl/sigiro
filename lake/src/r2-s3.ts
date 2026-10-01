// R2 S3 adapter: DuckDB's httpfs speaks S3 to https://r2.internal/<bucket>/<key> and this
// module answers from the R2 binding inside the isolate — no credentials, no network hop.
// DI flag: installed by replacing globalThis.fetch, because httpfs exposes no fetch
// injection point. It must stay the only global patch and is installed once per isolate.
const HOST = "r2.internal";
const xml = (
  body: string,
  status = 200,
  headers: Record<string, string> = {}
) =>
  new Response(`<?xml version="1.0" encoding="UTF-8"?>${body}`, {
    status,
    headers: { "content-type": "application/xml", ...headers },
  });
const esc = (s: string) =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

let installed: R2Bucket | undefined;
export function installR2S3(bucket: R2Bucket) {
  if (installed === bucket) return;
  if (installed)
    throw new Error(
      "R2 S3 adapter is already installed for a different bucket"
    );
  installed = bucket;
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
    const req = new Request(input as RequestInfo, init);
    const url = new URL(req.url);
    if (url.hostname !== HOST) return realFetch(input as RequestInfo, init);
    return r2S3Handle(req, bucket);
  };
}

export async function r2S3Handle(
  req: Request,
  bucket: R2Bucket
): Promise<Response> {
  const url = new URL(req.url);
  const [, , ...rest] = url.pathname.split("/"); // "", bucket, key...
  const key = decodeURIComponent(rest.join("/"));
  const q = url.searchParams;
  const op = `${req.method}${q.has("uploads") ? "?uploads" : q.has("uploadId") ? "?uploadId" : q.has("list-type") ? "?list" : ""}`;
  try {
    if (req.method === "GET" && q.has("list-type")) {
      const prefix = q.get("prefix") ?? "";
      const listed = await bucket.list({
        prefix,
        cursor: q.get("continuation-token") ?? undefined,
        limit: 1000,
      });
      const contents = listed.objects
        .map(
          (o) =>
            `<Contents><Key>${esc(o.key)}</Key><LastModified>${o.uploaded.toISOString()}</LastModified><ETag>"${o.etag}"</ETag><Size>${o.size}</Size><StorageClass>STANDARD</StorageClass></Contents>`
        )
        .join("");
      const trunc = listed.truncated
        ? `<IsTruncated>true</IsTruncated><NextContinuationToken>${esc(listed.cursor)}</NextContinuationToken>`
        : "<IsTruncated>false</IsTruncated>";
      return xml(
        `<ListBucketResult><Name>lake</Name><Prefix>${esc(prefix)}</Prefix><KeyCount>${listed.objects.length}</KeyCount>${trunc}${contents}</ListBucketResult>`
      );
    }
    if (req.method === "HEAD") {
      const h = await bucket.head(key);
      if (!h) return new Response(null, { status: 404 });
      return new Response(null, {
        status: 200,
        headers: {
          "content-length": String(h.size),
          etag: `"${h.etag}"`,
          "last-modified": h.uploaded.toUTCString(),
          "accept-ranges": "bytes",
          "content-type": "application/octet-stream",
        },
      });
    }
    if (req.method === "GET") {
      const range = req.headers.get("range");
      let obj: R2ObjectBody | null;
      if (range) {
        const m = /bytes=(\d+)-(\d*)/.exec(range);
        if (!m) return new Response("bad range", { status: 400 });
        const offset = Number(m[1]);
        const end = m[2] ? Number(m[2]) : undefined;
        obj = await bucket.get(key, {
          range:
            end === undefined
              ? { offset }
              : { offset, length: end - offset + 1 },
        });
        if (!obj) return xml("<Error><Code>NoSuchKey</Code></Error>", 404);
        const buf = await obj.arrayBuffer();
        return new Response(buf, {
          status: 206,
          headers: {
            "content-length": String(buf.byteLength),
            "content-range": `bytes ${offset}-${offset + buf.byteLength - 1}/${obj.size}`,
            etag: `"${obj.etag}"`,
            "last-modified": obj.uploaded.toUTCString(),
          },
        });
      }
      obj = await bucket.get(key);
      if (!obj) return xml("<Error><Code>NoSuchKey</Code></Error>", 404);
      const buf = await obj.arrayBuffer();
      return new Response(buf, {
        status: 200,
        headers: {
          "content-length": String(buf.byteLength),
          etag: `"${obj.etag}"`,
          "last-modified": obj.uploaded.toUTCString(),
        },
      });
    }
    if (req.method === "POST" && q.has("uploads")) {
      const mpu = await bucket.createMultipartUpload(key);
      return xml(
        `<InitiateMultipartUploadResult><Bucket>lake</Bucket><Key>${esc(key)}</Key><UploadId>${esc(mpu.uploadId)}</UploadId></InitiateMultipartUploadResult>`
      );
    }
    if (req.method === "PUT" && q.has("uploadId")) {
      const mpu = bucket.resumeMultipartUpload(key, q.get("uploadId")!);
      const body = await req.arrayBuffer();
      const part = await mpu.uploadPart(Number(q.get("partNumber")), body);
      return new Response(null, {
        status: 200,
        headers: { etag: `"${part.etag}"` },
      });
    }
    if (req.method === "POST" && q.has("uploadId")) {
      const mpu = bucket.resumeMultipartUpload(key, q.get("uploadId")!);
      const body = await req.text();
      const parts = [...body.matchAll(/<Part>([\s\S]*?)<\/Part>/g)].map((m) => {
        const n = /<PartNumber>(\d+)<\/PartNumber>/.exec(m[1]);
        const t = /<ETag>(?:"|&quot;)?([^<"&]+)(?:"|&quot;)?<\/ETag>/.exec(
          m[1]
        );
        if (!n || !t) throw new Error(`malformed part: ${m[1]}`);
        return { partNumber: Number(n[1]), etag: t[1] };
      });
      if (parts.length === 0)
        throw new Error(`no parts in complete body: ${body.slice(0, 300)}`);
      const done = await mpu.complete(parts);
      return xml(
        `<CompleteMultipartUploadResult><Bucket>lake</Bucket><Key>${esc(key)}</Key><ETag>"${done.etag}"</ETag></CompleteMultipartUploadResult>`
      );
    }
    if (req.method === "DELETE" && q.has("uploadId")) {
      await bucket.resumeMultipartUpload(key, q.get("uploadId")!).abort();
      return new Response(null, { status: 204 });
    }
    if (req.method === "PUT") {
      const body = await req.arrayBuffer();
      const put = await bucket.put(key, body);
      if (!put) throw new Error(`R2 put of ${key} returned no object`);
      return new Response(null, {
        status: 200,
        headers: { etag: `"${put.etag}"` },
      });
    }
    if (req.method === "DELETE") {
      await bucket.delete(key);
      return new Response(null, { status: 204 });
    }
    if (req.method === "POST" && q.has("delete")) {
      const body = await req.text();
      const keys = [...body.matchAll(/<Key>([^<]+)<\/Key>/g)].map((m) => m[1]);
      await bucket.delete(keys);
      return xml(
        `<DeleteResult>${keys.map((k) => `<Deleted><Key>${esc(k)}</Key></Deleted>`).join("")}</DeleteResult>`
      );
    }
    return new Response(`unsupported ${req.method} ${url.search}`, {
      status: 501,
    });
  } catch (e) {
    console.error("r2-s3", op, key, e);
    return xml(
      `<Error><Code>InternalError</Code><Message>${esc(String(e))}</Message></Error>`,
      500
    );
  }
}

export const R2_S3_SECRET_SQL = `CREATE OR REPLACE SECRET lake (TYPE S3, KEY_ID 'shim', SECRET 'shim', REGION 'auto', ENDPOINT '${HOST}', URL_STYLE 'path', USE_SSL true)`;
