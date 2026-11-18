import { Config, ConfigProvider, Effect, FileSystem, Redacted } from "effect";
import type { PlatformError } from "effect/PlatformError";

export const secret = Effect.fn("secret")(function* (name: string): Effect.fn.Return<string, Config.ConfigError | PlatformError, FileSystem.FileSystem> {
  const file = yield* Config.string(`${name}_FILE`).pipe(Config.option);
  if (file._tag === "Some") {
    const fs = yield* Effect.service(FileSystem.FileSystem);
    const value = (yield* fs.readFileString(file.value)).trim();
    if (value.length === 0) return yield* Effect.fail(new Config.ConfigError(new ConfigProvider.SourceError({ message: `${name}_FILE is empty` })));
    return value;
  }
  return Redacted.value(yield* Config.redacted(name));
});
