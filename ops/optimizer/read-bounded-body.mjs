// OTLP HTTP exporters may send Transfer-Encoding: chunked instead of Content-Length.
// Enforce the byte limit while reading regardless of which framing they choose.
export class InvalidBody extends Error {
  constructor(status) {
    super(`Invalid or oversized request body (${status})`);
    this.status = status;
  }
}

export async function readBoundedBody(req, limit) {
  const header = req.headers["content-length"];
  let announced;
  if (header !== undefined) {
    if (typeof header !== "string" || !/^(0|[1-9][0-9]*)$/.test(header)) {
      throw new InvalidBody(400);
    }
    announced = Number(header);
    if (!Number.isSafeInteger(announced) || announced > limit)
      throw new InvalidBody(413);
  }
  const parts = [];
  let size = 0;
  for await (const part of req) {
    size += part.length;
    if (size > limit) throw new InvalidBody(413);
    parts.push(part);
  }
  if (announced !== undefined && size !== announced) throw new InvalidBody(400);
  return Buffer.concat(parts, size);
}
