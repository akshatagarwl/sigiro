/**
 * Emits the D1 schema as SQL.
 *
 * better-auth's own CLI cannot do this: "Cloudflare D1 can only be queried
 * through a Cloudflare Worker, so the CLI cannot access it directly", and
 * `getMigrations` only supports the built-in Kysely adapter, not Drizzle.
 * So the checked-in Drizzle schema is the source of truth and this renders it.
 *
 * Usage:
 *   aube run schema:sql > schema.sql
 *   wrangler d1 execute sigiro-auth --file=schema.sql [--local|--remote]
 */
import { getTableConfig } from "drizzle-orm/sqlite-core";
import { is } from "drizzle-orm";
import { SQLiteTable } from "drizzle-orm/sqlite-core";

import * as schema from "../src/schema/auth-schema.js";
import { D1_SECONDARY_STORAGE_DDL } from "../src/d1/secondary-storage.js";

const quote = (identifier: string) => `"${identifier.replace(/"/g, '""')}"`;

const statements: string[] = [];

for (const value of Object.values(schema)) {
  if (!is(value, SQLiteTable)) continue;
  const table = getTableConfig(value);
  const columns = table.columns.map((column) => {
    const parts = [quote(column.name), column.getSQLType()];
    if (column.primary) parts.push("PRIMARY KEY");
    if (column.notNull && !column.primary) parts.push("NOT NULL");
    if (column.isUnique) parts.push("UNIQUE");
    return "  " + parts.join(" ");
  });

  for (const fk of table.foreignKeys) {
    const reference = fk.reference();
    const from = reference.columns.map((column) => quote(column.name)).join(", ");
    const to = reference.foreignColumns.map((column) => quote(column.name)).join(", ");
    const target = getTableConfig(reference.foreignTable).name;
    const onDelete = fk.onDelete ? ` ON DELETE ${fk.onDelete.toUpperCase()}` : "";
    columns.push(`  FOREIGN KEY (${from}) REFERENCES ${quote(target)} (${to})${onDelete}`);
  }

  statements.push(`CREATE TABLE IF NOT EXISTS ${quote(table.name)} (\n${columns.join(",\n")}\n);`);

  for (const index of table.indexes) {
    const config = index.config;
    const indexColumns = (config.columns as { name?: string }[])
      .map((column) => (column.name ? quote(column.name) : null))
      .filter((name): name is string => name !== null)
      .join(", ");
    if (!indexColumns) continue;
    statements.push(
      `CREATE ${config.unique ? "UNIQUE " : ""}INDEX IF NOT EXISTS ${quote(config.name)} ON ${quote(table.name)} (${indexColumns});`,
    );
  }
}

for (const ddl of D1_SECONDARY_STORAGE_DDL) statements.push(`${ddl};`);

console.log(statements.join("\n\n"));
