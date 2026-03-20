mod schema;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use duckdb::Connection;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "sigiro", about = "Observability for AI agents")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start gRPC + HTTP servers
    Serve,
    /// Run SQL against local DuckDB
    Query {
        /// SQL statement to execute
        sql: String,
    },
    /// Show services seen, row counts, data size, uptime
    Status,
}

fn data_dir() -> Result<PathBuf> {
    match std::env::var("SIGIRO_DATA_DIR") {
        Ok(dir) => Ok(PathBuf::from(dir)),
        Err(_) => {
            let home = dirs::home_dir().context("cannot determine home directory")?;
            Ok(home.join(".sigiro").join("data"))
        }
    }
}

fn open_db() -> Result<Connection> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create data directory: {}", dir.display()))?;
    let db_path = dir.join("sigiro.duckdb");
    Connection::open(&db_path)
        .with_context(|| format!("failed to open DuckDB at {}", db_path.display()))
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Serve) | None => {
            let conn = open_db()?;
            schema::create_tables(&conn)?;
            let db_path = data_dir()?.join("sigiro.duckdb");
            eprintln!("sigiro ready — {}", db_path.display());
        }
        Some(Command::Query { sql }) => {
            let conn = open_db()?;
            run_query(&conn, &sql)?;
        }
        Some(Command::Status) => {
            let conn = open_db()?;
            schema::create_tables(&conn)?;
            print_status(&conn)?;
        }
    }
    Ok(())
}

fn run_query(conn: &Connection, sql: &str) -> Result<()> {
    let mut stmt = conn.prepare(sql).context("failed to prepare SQL")?;
    let mut rows = stmt.query([]).context("failed to execute query")?;

    let column_count = rows.as_ref().context("no result set")?.column_count();
    let column_names: Vec<String> = (0..column_count)
        .map(|i| {
            rows.as_ref()
                .expect("checked above")
                .column_name(i)
                .expect("valid column index")
                .to_string()
        })
        .collect();
    println!("{}", column_names.join("\t"));

    while let Some(row) = rows.next()? {
        let vals: Vec<String> = (0..column_count)
            .map(|i| {
                row.get::<_, String>(i)
                    .or_else(|_| row.get::<_, i64>(i).map(|v| v.to_string()))
                    .or_else(|_| row.get::<_, f64>(i).map(|v| v.to_string()))
                    .unwrap_or_else(|_| "NULL".to_string())
            })
            .collect();
        println!("{}", vals.join("\t"));
    }
    Ok(())
}

fn print_status(conn: &Connection) -> Result<()> {
    let tables = [
        "sigiro_spans",
        "sigiro_logs",
        "sigiro_metrics_gauge",
        "sigiro_metrics_sum",
        "sigiro_metrics_histogram",
        "sigiro_metrics_exp_histogram",
    ];
    println!("sigiro status");
    println!("─────────────");
    for table in tables {
        let count: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0))
            .unwrap_or(0);
        println!("  {table}: {count} rows");
    }
    Ok(())
}
