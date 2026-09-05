import { Effect, Encoding } from "effect";

const encoder = new TextEncoder();

export const sha256 = Effect.fn("sha256")(function*(value: string) {
  const digest = yield* Effect.promise(() =>
    crypto.subtle.digest("SHA-256", encoder.encode(value))
  );
  return Encoding.encodeHex(new Uint8Array(digest));
});
