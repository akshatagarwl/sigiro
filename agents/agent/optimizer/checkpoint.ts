import { createCipheriv, createDecipheriv, randomBytes } from "node:crypto";

const MAGIC = Buffer.from("SIGIRO-CF-1\0");
const IV_BYTES = 12;
const TAG_BYTES = 16;

export function checkpointKey(): Buffer {
  const hex = process.env.SIGIRO_OPTIMIZER_SNAPSHOT_KEY;
  if (!hex || !/^[0-9a-f]{64}$/i.test(hex)) throw new Error("A 32-byte optimizer snapshot encryption key is required");
  return Buffer.from(hex, "hex");
}

function associatedData(sessionKey: string): Buffer {
  return Buffer.concat([MAGIC, Buffer.from(sessionKey, "utf8")]);
}

export function encryptCheckpoint(archive: Buffer, sessionKey: string, key: Buffer): Buffer {
  if (key.length !== 32) throw new Error("Invalid optimizer snapshot key");
  const iv = randomBytes(IV_BYTES);
  const cipher = createCipheriv("aes-256-gcm", key, iv);
  cipher.setAAD(associatedData(sessionKey));
  return Buffer.concat([MAGIC, iv, cipher.update(archive), cipher.final(), cipher.getAuthTag()]);
}

export function decryptCheckpoint(snapshot: Buffer, sessionKey: string, key: Buffer): Buffer {
  if (key.length !== 32 || snapshot.length < MAGIC.length + IV_BYTES + TAG_BYTES ||
    !snapshot.subarray(0, MAGIC.length).equals(MAGIC)) throw new Error("Invalid encrypted optimizer checkpoint");
  const iv = snapshot.subarray(MAGIC.length, MAGIC.length + IV_BYTES);
  const decipher = createDecipheriv("aes-256-gcm", key, iv);
  decipher.setAAD(associatedData(sessionKey));
  decipher.setAuthTag(snapshot.subarray(-TAG_BYTES));
  try { return Buffer.concat([decipher.update(snapshot.subarray(MAGIC.length + IV_BYTES, -TAG_BYTES)), decipher.final()]); }
  catch { throw new Error("Optimizer checkpoint authentication failed"); }
}
