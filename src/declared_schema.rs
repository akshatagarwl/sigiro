//! Declared storage: each table is one `CREATE TABLE` statement plus its
//! indexes, written with `{table}` in place of the table name. Opening a
//! database reconciles it to the declaration in one transaction: create
//! missing tables, rebuild tables whose declaration changed (keeping the
//! columns both shapes share), and drop tables this code once declared but no
//! longer does. The `schema_declaration` table records what each table was
//! built from, so the reconciler only touches tables it created and works the
//! same on SQLite and PostgreSQL. There are no migration steps.

use anyhow::{Context, Result};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub(crate) struct DeclaredTable {
    pub name: &'static str,
    pub create: &'static str,
    pub indexes: &'static [&'static str],
}

impl DeclaredTable {
    fn declaration(&self) -> String {
        std::iter::once(self.create)
            .chain(self.indexes.iter().copied())
            .collect::<Vec<_>>()
            .join(";\n")
    }
}

pub(crate) trait SchemaStore {
    fn execute(&mut self, sql: &str) -> Result<()>;
    fn qualify(&self, table: &str) -> String;
    fn columns(&mut self, table: &str) -> Result<Vec<String>>;
    fn recorded(&mut self) -> Result<BTreeMap<String, String>>;
    fn record(&mut self, table: &str, declaration: &str) -> Result<()>;
    fn forget(&mut self, table: &str) -> Result<()>;
    fn continue_sequences(&mut self, table: &str) -> Result<()>;
}

pub(crate) fn reconcile(store: &mut dyn SchemaStore, tables: &[DeclaredTable]) -> Result<()> {
    let recorded = store.recorded()?;
    for name in recorded.keys() {
        if !tables.iter().any(|table| table.name == name) {
            let qualified = store.qualify(name);
            store.execute(&format!("DROP TABLE IF EXISTS {qualified}"))?;
            store.forget(name)?;
        }
    }
    for table in tables {
        let declaration = table.declaration();
        if recorded.get(table.name) == Some(&declaration) {
            continue;
        }
        let existing = store.columns(table.name)?;
        if existing.is_empty() {
            create(store, table, table.name)?;
        } else {
            rebuild(store, table, &existing).with_context(|| {
                format!("cannot reconcile table {} to its declaration", table.name)
            })?;
        }
        store.record(table.name, &declaration)?;
    }
    Ok(())
}

fn create(store: &mut dyn SchemaStore, table: &DeclaredTable, name: &str) -> Result<()> {
    let qualified = store.qualify(name);
    store.execute(&table.create.replace("{table}", &qualified))?;
    if name == table.name {
        for index in table.indexes {
            store.execute(&index.replace("{table}", &qualified))?;
        }
    }
    Ok(())
}

fn rebuild(store: &mut dyn SchemaStore, table: &DeclaredTable, existing: &[String]) -> Result<()> {
    let staging = format!("{}_reconciling", table.name);
    create(store, table, &staging)?;
    let shared: Vec<String> = store
        .columns(&staging)?
        .into_iter()
        .filter(|column| existing.contains(column))
        .map(|column| format!("\"{column}\""))
        .collect();
    let (current, staged) = (store.qualify(table.name), store.qualify(&staging));
    if !shared.is_empty() {
        let list = shared.join(", ");
        store.execute(&format!(
            "INSERT INTO {staged} ({list}) SELECT {list} FROM {current}"
        ))?;
    }
    store.execute(&format!("DROP TABLE {current}"))?;
    store.execute(&format!("ALTER TABLE {staged} RENAME TO {}", table.name))?;
    for index in table.indexes {
        store.execute(&index.replace("{table}", &current))?;
    }
    store.continue_sequences(table.name)
}

pub(crate) struct SqliteSchema<'a>(pub &'a rusqlite::Transaction<'a>);

impl SchemaStore for SqliteSchema<'_> {
    fn execute(&mut self, sql: &str) -> Result<()> {
        self.0
            .execute_batch(sql)
            .with_context(|| format!("failed to run: {sql}"))
    }

    fn qualify(&self, table: &str) -> String {
        format!("\"{table}\"")
    }

    fn columns(&mut self, table: &str) -> Result<Vec<String>> {
        let mut statement = self.0.prepare("SELECT name FROM pragma_table_info(?1)")?;
        let names = statement
            .query_map([table], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(names)
    }

    fn recorded(&mut self) -> Result<BTreeMap<String, String>> {
        self.execute("CREATE TABLE IF NOT EXISTS schema_declaration (table_name TEXT PRIMARY KEY, declaration TEXT NOT NULL)")?;
        let mut statement = self
            .0
            .prepare("SELECT table_name, declaration FROM schema_declaration")?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    fn record(&mut self, table: &str, declaration: &str) -> Result<()> {
        self.0.execute(
            "INSERT INTO schema_declaration (table_name, declaration) VALUES (?1, ?2)
             ON CONFLICT (table_name) DO UPDATE SET declaration = excluded.declaration",
            [table, declaration],
        )?;
        Ok(())
    }

    fn forget(&mut self, table: &str) -> Result<()> {
        self.0.execute(
            "DELETE FROM schema_declaration WHERE table_name = ?1",
            [table],
        )?;
        Ok(())
    }

    fn continue_sequences(&mut self, _table: &str) -> Result<()> {
        Ok(())
    }
}

pub(crate) struct PostgresSchema<'a, 'b> {
    pub transaction: &'a mut postgres::Transaction<'b>,
    pub schema: &'static str,
}

impl SchemaStore for PostgresSchema<'_, '_> {
    fn execute(&mut self, sql: &str) -> Result<()> {
        self.transaction
            .batch_execute(sql)
            .with_context(|| format!("failed to run: {sql}"))
    }

    fn qualify(&self, table: &str) -> String {
        format!("{}.\"{table}\"", self.schema)
    }

    fn columns(&mut self, table: &str) -> Result<Vec<String>> {
        let rows = self.transaction.query(
            "SELECT column_name FROM information_schema.columns WHERE table_schema = $1 AND table_name = $2 ORDER BY ordinal_position",
            &[&self.schema, &table],
        )?;
        Ok(rows.into_iter().map(|row| row.get(0)).collect())
    }

    fn recorded(&mut self) -> Result<BTreeMap<String, String>> {
        let schema = self.schema;
        self.execute(&format!(
            "CREATE SCHEMA IF NOT EXISTS {schema};
             CREATE TABLE IF NOT EXISTS {schema}.schema_declaration (table_name TEXT PRIMARY KEY, declaration TEXT NOT NULL)"
        ))?;
        let rows = self.transaction.query(
            &format!("SELECT table_name, declaration FROM {schema}.schema_declaration"),
            &[],
        )?;
        Ok(rows
            .into_iter()
            .map(|row| (row.get(0), row.get(1)))
            .collect())
    }

    fn record(&mut self, table: &str, declaration: &str) -> Result<()> {
        let schema = self.schema;
        self.transaction.execute(
            &format!(
                "INSERT INTO {schema}.schema_declaration (table_name, declaration) VALUES ($1, $2)
                 ON CONFLICT (table_name) DO UPDATE SET declaration = excluded.declaration"
            ),
            &[&table, &declaration],
        )?;
        Ok(())
    }

    fn forget(&mut self, table: &str) -> Result<()> {
        let schema = self.schema;
        self.transaction.execute(
            &format!("DELETE FROM {schema}.schema_declaration WHERE table_name = $1"),
            &[&table],
        )?;
        Ok(())
    }

    fn continue_sequences(&mut self, table: &str) -> Result<()> {
        let qualified = self.qualify(table);
        for column in self.columns(table)? {
            let sequence: Option<String> = self
                .transaction
                .query_one(
                    "SELECT pg_get_serial_sequence($1, $2)",
                    &[&qualified, &column],
                )?
                .get(0);
            if let Some(sequence) = sequence {
                self.execute(&format!(
                    "SELECT setval('{sequence}', COALESCE((SELECT max(\"{column}\") FROM {qualified}), 0) + 1, false)"
                ))?;
            }
        }
        Ok(())
    }
}
