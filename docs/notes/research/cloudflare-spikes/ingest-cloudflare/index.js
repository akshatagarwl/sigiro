import { DurableObject } from "cloudflare:workers";

export class Cell extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    ctx.storage.sql.exec(
      "CREATE TABLE IF NOT EXISTS wal (id INTEGER PRIMARY KEY, body BLOB)"
    );
    ctx.storage.sql.exec(
      "CREATE TABLE IF NOT EXISTS segment (key TEXT PRIMARY KEY, state TEXT)"
    );
  }
  rows(cursor) {
    cursor.toArray();
    return { read: cursor.rowsRead, written: cursor.rowsWritten };
  }
  async measure(kb) {
    const body = new Uint8Array(kb * 1024).map((_, i) => i % 251);
    const sql = this.ctx.storage.sql;
    const insert = this.rows(
      sql.exec("INSERT INTO wal (body) VALUES (?)", body)
    );
    const del = this.rows(sql.exec("DELETE FROM wal"));
    const key = `seg-${crypto.randomUUID()}`;
    const register = this.rows(
      sql.exec("INSERT INTO segment VALUES (?, 'pending')", key)
    );
    const r2 = [];
    for (const mb of [1, 4, 16]) {
      const t = performance.now();
      await this.env.SEGMENTS.put(
        `${key}-${mb}`,
        new Uint8Array(mb * 1024 * 1024)
      );
      r2.push({ mb, ms: Math.round(performance.now() - t) });
    }
    const commit = this.rows(
      sql.exec("UPDATE segment SET state = 'live' WHERE key = ?", key)
    );
    const t = performance.now();
    await this.ctx.storage.sync?.();
    const syncMs = Math.round(performance.now() - t);
    return {
      walInsert: insert,
      walDelete: del,
      register,
      commit,
      r2PutFromCell: r2,
      syncMs,
    };
  }
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const kb = Number(url.searchParams.get("kb") ?? 32);
    const stub = env.CELL.get(
      env.CELL.idFromName(url.searchParams.get("cell") ?? "a")
    );
    const t = performance.now();
    const result = await stub.measure(kb);
    return Response.json({
      ...result,
      totalMs: Math.round(performance.now() - t),
    });
  },
};
