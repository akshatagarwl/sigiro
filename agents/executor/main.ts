import { BunRuntime, BunServices } from "@effect/platform-bun";
import { Config, Effect, Layer } from "effect";
import { secret } from "../src/config.ts";
import { makeCoordinatorClient } from "./coordinator.ts";
import { makeEveExecution } from "./eve.ts";
import { workerProgram } from "./worker.ts";
import { makeFileJobStore } from "./services.ts";
import { telemetry } from "./telemetry.ts";

const settings = Config.all({
  coordinatorUrl: Config.string("COORDINATOR_URL"), eveUrl: Config.string("EVE_URL"), workerId: Config.nonEmptyString("WORKER_ID"),
  pollSeconds: Config.number("WORKER_POLL_SECONDS").pipe(Config.withDefault(2)),
  stateDir: Config.string("EXECUTOR_STATE_DIR").pipe(Config.withDefault("/var/lib/sigiro-agent/jobs")),
});

const configured = Effect.gen(function* () {
  const config = yield* settings;
  if (config.pollSeconds <= 0) return yield* Effect.fail(new Error("WORKER_POLL_SECONDS must be positive"));
  // Eve host authenticates the executor with this same bearer; there is one worker token.
  const workerToken = yield* secret("EVE_WORKER_TOKEN");
  const client = makeCoordinatorClient({ coordinatorUrl: config.coordinatorUrl, workerId: config.workerId, workerToken });
  const store = makeFileJobStore(config.stateDir);
  const eve = makeEveExecution({ url: config.eveUrl, workerToken, workerId: config.workerId }).pipe(Layer.provide(store));
  const services = Layer.mergeAll(store, eve, client);
  yield* Effect.logInfo("Sigiro agent worker started");
  yield* Effect.provide(workerProgram(config.pollSeconds), services);
}).pipe(Effect.provide(BunServices.layer), Effect.provide(telemetry));

BunRuntime.runMain(configured);
