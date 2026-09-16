import { BunRuntime, BunServices } from "@effect/platform-bun";
import { Config, Console, Effect, FileSystem, Layer, Path, Schema } from "effect";
import { Command, Flag } from "effect/unstable/cli";
import { FetchHttpClient } from "effect/unstable/http";
import { secret } from "../src/config.ts";
import { bridgeTelemetry } from "../executor/telemetry.ts";
import { bridgeOnce } from "./bridge.ts";
import { makeBridgeClient } from "./client.ts";
import { BridgeError } from "./errors.ts";
import { BridgePolicy } from "./types.ts";
import { encodeBridgeJson } from "./store.ts";

const settings = Config.all({
  sigiroUrl: Config.NonEmptyString("SIGIRO_URL"),
  coordinatorUrl: Config.NonEmptyString("COORDINATOR_URL"),
  repository: Config.NonEmptyString("SIGIRO_REPOSITORY"),
  evidenceDir: Config.NonEmptyString("SIGIRO_EVIDENCE_DIR"),
  stateDir: Config.NonEmptyString("BRIDGE_STATE_DIR"),
  pollSeconds: Config.Number("BRIDGE_POLL_SECONDS").pipe(Config.withDefault(300)),
});

Command.make("bridge", {
  config: Flag.String("config").pipe(Flag.withDescription("Trusted service/repository policy JSON file")),
  watch: Flag.Boolean("watch").pipe(Flag.withDefault(false), Flag.withDescription("Repeat the bridge pass; default is one pass")),
}, Effect.fn(function* ({ config: path, watch }) {
  const runtime = yield* settings;
  if (!Number.isFinite(runtime.pollSeconds) || runtime.pollSeconds <= 0) {
    return yield* new BridgeError({ operation: "configure bridge", detail: "BRIDGE_POLL_SECONDS must be positive and finite", retryable: false });
  }
  for (const value of [runtime.sigiroUrl, runtime.coordinatorUrl]) {
    const url = yield* Effect.try({
      try: () => new URL(value),
      catch: () => new BridgeError({ operation: "configure bridge", detail: "endpoint must be an absolute HTTP URL", retryable: false }),
    });
    if (!["http:", "https:"].includes(url.protocol) || url.username || url.password || url.search || url.hash) {
      return yield* new BridgeError({ operation: "configure bridge", detail: "endpoint must be HTTP(S) without credentials, query, or fragment", retryable: false });
    }
  }
  const fs = yield* FileSystem.FileSystem;
  const policy = yield* Schema.decodeEffect(Schema.fromJsonString(BridgePolicy))(yield* fs.readFileString(path)).pipe(
    Effect.mapError(() => new BridgeError({ operation: "configure bridge", detail: "policy file does not match the required schema", retryable: false })),
  );
  const pathService = yield* Path.Path;
  if (policy.repository !== runtime.repository) {
    return yield* new BridgeError({ operation: "configure bridge", detail: "policy repository must match Eve's SIGIRO_REPOSITORY", retryable: false });
  }
  const client = yield* makeBridgeClient({
    sigiroUrl: runtime.sigiroUrl, coordinatorUrl: runtime.coordinatorUrl,
    sigiroAccessToken: yield* secret("SIGIRO_ACCESS_TOKEN"), workerToken: yield* secret("EVE_WORKER_TOKEN"),
  });
  const pass = bridgeOnce({ settings: policy, evidenceDir: pathService.resolve(runtime.evidenceDir), stateDir: pathService.resolve(runtime.stateDir) }, client).pipe(
    Effect.flatMap((result) => Effect.flatMap(encodeBridgeJson(result), (encoded) => Console.log(encoded))),
  );
  if (!watch) return yield* pass;
  while (true) {
    yield* pass.pipe(Effect.catchTag("BridgeError", (error) => error.retryable
      ? Effect.logError("bridge pass failed", error)
      : Effect.fail(error)));
    yield* Effect.sleep(`${runtime.pollSeconds} seconds`);
  }
})).pipe(
  Command.withDescription("Bridge Sigiro anomalies into evidence-backed celld jobs"),
  Command.run({ version: "0.0.0" }),
  Effect.provide(Layer.mergeAll(FetchHttpClient.layer, BunServices.layer, bridgeTelemetry)),
  BunRuntime.runMain,
);
