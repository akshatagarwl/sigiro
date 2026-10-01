import * as ducklings from "@ducklings/workers-ducklake";
import {
  AccessMode,
  DuckDB,
  init,
  type Connection,
} from "@ducklings/workers-ducklake";
import wasmModule from "@ducklings/workers-ducklake/wasm/duckdb-workers.wasm";

import { installR2S3, R2_S3_SECRET_SQL } from "./r2-s3";

/** Emscripten MEMFS surface the lake needs to publish and restore its catalog. */
export interface LakeFileSystem {
  readFile(path: string): Uint8Array;
  writeFile(path: string, data: Uint8Array): void;
}

/** The custom runtime build exports `fileSystem()`; the published package does not. */
export function fileSystem(): LakeFileSystem {
  const exported = (ducklings as { fileSystem?: () => LakeFileSystem })
    .fileSystem;
  if (typeof exported !== "function")
    throw new Error(
      "DuckDB-WASM runtime exposes no fileSystem(); the custom ducklings build is required"
    );
  return exported();
}

let connection: Promise<Connection> | undefined;

/** A FATAL DuckDB error invalidates the database; drop it so the next caller gets a fresh one. */
export function discardIfFatal(error: unknown): void {
  if (String(error).includes("FATAL Error")) connection = undefined;
}

/** One DuckDB per isolate, bounded by the query budget so overruns are DuckDB errors, not isolate kills. */
export function lakeConnection(bucket: R2Bucket): Promise<Connection> {
  connection ??= (async () => {
    installR2S3(bucket);
    await init({ wasmModule });
    const db = new DuckDB({
      accessMode: AccessMode.READ_WRITE,
      lockConfiguration: false,
      customConfig: {
        memory_limit: "64MB",
        preserve_insertion_order: "false",
        threads: "1",
      },
    });
    const c = db.connect();
    // httpfs defaults to 800 GB / 10,000 parts = 76.5 MiB parts, which cannot fit; use 5 MiB parts.
    for (const sql of [
      R2_S3_SECRET_SQL,
      "SET temp_directory=''",
      "SET s3_uploader_max_filesize='10GB'",
      "SET s3_uploader_max_parts_per_file=2000",
    ])
      await c.execute(sql);
    return c;
  })();
  return connection;
}
