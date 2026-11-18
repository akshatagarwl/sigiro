import { Schema, SchemaTransformation } from "effect";

/**
 * The pieces both coordinator entrypoints need. `index.ts` and `job.ts`
 * previously carried their own copies of all of this, differing only in an
 * error message.
 *
 * Reading the body is not here: `HttpServerRequest.fromWeb(request).text` is
 * already an Effect, so the wrapper this module used to export existed only to
 * repeat it.
 *
 * A body that is too large is a contract violation like any other, so the size
 * bound sits on the schema rather than in a hand-written length check. The
 * `content-length` header is only a hint; the decoded text is what is bounded.
 */

const MAX_BODY_BYTES = 256 * 1024;

export const json = (body: unknown, status = 200): Response =>
  Response.json(body, { status, headers: { "cache-control": "no-store" } });

/** The size bound applies to the raw text, before any JSON parsing is attempted. */
const BoundedText = Schema.String.check(Schema.isMaxLength(MAX_BODY_BYTES));

export const BoundedJson = <S extends Schema.Constraint>(schema: S) =>
  BoundedText.pipe(Schema.decodeTo(schema, SchemaTransformation.fromJsonString()));
