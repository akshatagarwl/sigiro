import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { checkpointKey, decryptCheckpoint, encryptCheckpoint } from "./checkpoint.ts";

const old = process.env.SIGIRO_OPTIMIZER_SNAPSHOT_KEY;
try {
  delete process.env.SIGIRO_OPTIMIZER_SNAPSHOT_KEY;
  assert.throws(() => checkpointKey(), /encryption key is required/);
  const key = randomBytes(32);
  process.env.SIGIRO_OPTIMIZER_SNAPSHOT_KEY = key.toString("hex");
  assert.deepEqual(checkpointKey(), key);
  const archive = Buffer.from("Private checkout and validation results stay encrypted at rest");
  const sealed = encryptCheckpoint(archive, "session-a", key);
  assert.equal(sealed.includes(archive), false);
  assert.deepEqual(decryptCheckpoint(sealed, "session-a", key), archive);
  assert.notDeepEqual(sealed, encryptCheckpoint(archive, "session-a", key));
  assert.throws(() => decryptCheckpoint(sealed, "session-b", key), /authentication failed/);
  assert.throws(() => decryptCheckpoint(sealed, "session-a", randomBytes(32)), /authentication failed/);
  const tampered = Buffer.from(sealed);
  tampered[tampered.length - 1] ^= 1;
  assert.throws(() => decryptCheckpoint(tampered, "session-a", key), /authentication failed/);
  assert.throws(() => decryptCheckpoint(archive, "session-a", key), /Invalid encrypted/);
  console.log("Snapshot encryption, isolation, tamper rejection and fail-closed key checks passed");
} finally {
  if (old === undefined) delete process.env.SIGIRO_OPTIMIZER_SNAPSHOT_KEY;
  else process.env.SIGIRO_OPTIMIZER_SNAPSHOT_KEY = old;
}
