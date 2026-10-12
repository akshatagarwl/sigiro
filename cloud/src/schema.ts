export type Schema = Record<string, string>;

export type SchemaStore = {
  all(sql: string, ...params: unknown[]): Record<string, unknown>[];
  run(sql: string, ...params: unknown[]): void;
  transaction(fn: () => void): void;
};

const applicationTable = /^[a-z][a-z0-9_]*$/u;

export function reconcile(store: SchemaStore, schema: Schema): void {
  for (const name of Object.keys(schema)) {
    if (!applicationTable.test(name) || name.startsWith("sqlite_")) {
      throw new Error(`declared table ${name} must start with a lowercase letter; underscore and sqlite_ names belong to the runtime`);
    }
  }
  const actual = new Map(
    store
      .all("SELECT name, sql FROM sqlite_master WHERE type = 'table'")
      .map((row) => [String(row.name), String(row.sql)] as const)
      .filter(([name]) => applicationTable.test(name) && !name.startsWith("sqlite_")),
  );
  store.transaction(() => {
    for (const [name] of actual) {
      if (!(name in schema)) {
        store.run(`DROP TABLE "${name}"`);
      }
    }
    for (const [name, declared] of Object.entries(schema)) {
      const current = actual.get(name);
      if (current === undefined) {
        store.run(declared);
      } else if (normalize(current) !== normalize(declared)) {
        rebuild(store, name, declared);
      }
    }
  });
}

function rebuild(store: SchemaStore, name: string, declared: string): void {
  const staging = `${name}_reconciling`;
  const before = columns(store, name);
  store.run(declared.replace(new RegExp(`^\\s*CREATE TABLE\\s+"?${name}"?`, "iu"), `CREATE TABLE "${staging}"`));
  const shared = columns(store, staging).filter((column) => before.includes(column));
  const list = shared.map((column) => `"${column}"`).join(", ");
  try {
    if (shared.length > 0) {
      store.run(`INSERT INTO "${staging}" (${list}) SELECT ${list} FROM "${name}"`);
    }
  } catch (error) {
    throw new Error(`cannot reconcile table ${name} to its declaration: ${error instanceof Error ? error.message : String(error)}`);
  }
  store.run(`DROP TABLE "${name}"`);
  store.run(`ALTER TABLE "${staging}" RENAME TO "${name}"`);
}

function columns(store: SchemaStore, table: string): string[] {
  return store.all(`PRAGMA table_info("${table}")`).map((row) => String(row.name));
}

function normalize(sql: string): string {
  return sql.replace(/\s+/gu, " ").replace(/\s*([(),])\s*/gu, "$1").replace(/"/gu, "").trim().toLowerCase();
}
