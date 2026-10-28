export type Schema = Record<string, string>;

export type SchemaStore = {
  all(sql: string, ...params: unknown[]): Record<string, unknown>[];
  run(sql: string, ...params: unknown[]): void;
  transaction(fn: () => void): void;
};

export function reconcile(store: SchemaStore, schema: Schema): void {
  store.run(
    "CREATE TABLE IF NOT EXISTS schema_declaration (table_name TEXT PRIMARY KEY, declaration TEXT NOT NULL)"
  );
  const recorded = new Map(
    store
      .all("SELECT table_name, declaration FROM schema_declaration")
      .map((row) => [String(row.table_name), String(row.declaration)] as const)
  );
  store.transaction(() => {
    for (const name of recorded.keys()) {
      if (!(name in schema)) {
        store.run(`DROP TABLE IF EXISTS "${name}"`);
        store.run("DELETE FROM schema_declaration WHERE table_name = ?", name);
      }
    }
    for (const [name, declaration] of Object.entries(schema)) {
      if (recorded.get(name) === declaration) {
        continue;
      }
      const existing = columns(store, name);
      if (existing.length === 0) {
        store.run(declaration.replaceAll("{table}", `"${name}"`));
      } else {
        rebuild(store, name, declaration, existing);
      }
      store.run(
        "INSERT INTO schema_declaration (table_name, declaration) VALUES (?, ?) ON CONFLICT (table_name) DO UPDATE SET declaration = excluded.declaration",
        name,
        declaration
      );
    }
  });
}

function rebuild(
  store: SchemaStore,
  name: string,
  declaration: string,
  existing: string[]
): void {
  const staging = `${name}_reconciling`;
  store.run(declaration.replaceAll("{table}", `"${staging}"`));
  const list = columns(store, staging)
    .filter((column) => existing.includes(column))
    .map((column) => `"${column}"`)
    .join(", ");
  try {
    if (list !== "") {
      store.run(
        `INSERT INTO "${staging}" (${list}) SELECT ${list} FROM "${name}"`
      );
    }
  } catch (error) {
    throw new Error(
      `cannot reconcile table ${name} to its declaration: ${error instanceof Error ? error.message : String(error)}`
    );
  }
  store.run(`DROP TABLE "${name}"`);
  store.run(`ALTER TABLE "${staging}" RENAME TO "${name}"`);
}

function columns(store: SchemaStore, table: string): string[] {
  return store
    .all(`PRAGMA table_info("${table}")`)
    .map((row) => String(row.name));
}
