//! Per-project DuckLake maintenance state for `/health` and metrics.
//!
//! The writer threads record every `CHECKPOINT lake` outcome here and refresh
//! the project's current data-file and inlined-row counts. Nothing here gates
//! ingest: a failing checkpoint is retried with backoff and reported, while
//! the rows keep committing (#228).

use dashmap::DashMap;
use duckdb::Connection;
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, ObservableGauge};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use utoipa::ToSchema;

/// One project's maintenance state.
#[derive(Clone, Debug, Default, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
pub struct ProjectStorage {
    /// When the last `CHECKPOINT` attempt finished (RFC 3339, UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checkpoint_at: Option<String>,
    /// Whether that attempt succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checkpoint_ok: Option<bool>,
    /// Failed attempts since the last success. Ingest continues regardless.
    pub consecutive_checkpoint_failures: u32,
    /// The last failure, while `consecutive_checkpoint_failures > 0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checkpoint_error: Option<String>,
    /// Parquet data files in the current snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_files: Option<u64>,
    /// Rows stored inline in the catalog, waiting for the next checkpoint to
    /// write them to Parquet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inlined_rows: Option<u64>,
}

/// Every project's maintenance state, shared by the writers, `/health` and
/// the metric callbacks.
pub struct StorageHealth {
    projects: Arc<DashMap<String, ProjectStorage>>,
    checkpoints: Counter<u64>,
    _gauges: Vec<ObservableGauge<u64>>,
}

impl StorageHealth {
    pub fn new() -> Arc<Self> {
        let projects: Arc<DashMap<String, ProjectStorage>> = Arc::new(DashMap::new());
        let meter = opentelemetry::global::meter("sigiro.storage");
        let gauge = |name: &'static str,
                     description: &'static str,
                     read: fn(&ProjectStorage) -> Option<u64>| {
            let projects = Arc::clone(&projects);
            meter
                .u64_observable_gauge(name)
                .with_description(description)
                .with_callback(move |observer| {
                    for entry in projects.iter() {
                        if let Some(value) = read(entry.value()) {
                            observer
                                .observe(value, &[KeyValue::new("project", entry.key().clone())]);
                        }
                    }
                })
                .build()
        };
        let gauges = vec![
            gauge(
                "sigiro.storage.data_files",
                "Parquet data files in the project's current DuckLake snapshot",
                |p| p.data_files,
            ),
            gauge(
                "sigiro.storage.inlined_rows",
                "Rows inlined in the DuckLake catalog, waiting for a checkpoint",
                |p| p.inlined_rows,
            ),
            gauge(
                "sigiro.storage.checkpoint_failures",
                "Consecutive failed DuckLake CHECKPOINTs",
                |p| Some(u64::from(p.consecutive_checkpoint_failures)),
            ),
        ];
        Arc::new(Self {
            projects,
            checkpoints: meter
                .u64_counter("sigiro.storage.checkpoints")
                .with_description("DuckLake CHECKPOINT attempts by outcome")
                .build(),
            _gauges: gauges,
        })
    }

    /// Record one `CHECKPOINT` attempt.
    pub(crate) fn record_checkpoint(&self, project_id: &str, error: Option<&str>) {
        let outcome = if error.is_some() { "failed" } else { "ok" };
        self.checkpoints
            .add(1, &[KeyValue::new("outcome", outcome)]);
        let mut entry = self.projects.entry(project_id.to_string()).or_default();
        entry.last_checkpoint_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .ok();
        entry.last_checkpoint_ok = Some(error.is_none());
        match error {
            None => {
                entry.consecutive_checkpoint_failures = 0;
                entry.last_checkpoint_error = None;
            }
            Some(error) => {
                entry.consecutive_checkpoint_failures += 1;
                entry.last_checkpoint_error = Some(error.to_string());
            }
        }
    }

    /// Re-read the project's data-file and inlined-row counts from its catalog.
    pub(crate) fn refresh(&self, project_id: &str, conn: &Connection) {
        match catalog_counts(conn) {
            Ok((files, inlined)) => {
                let mut entry = self.projects.entry(project_id.to_string()).or_default();
                entry.data_files = Some(files);
                entry.inlined_rows = Some(inlined);
            }
            Err(error) => {
                tracing::debug!(project_id, "could not read DuckLake file counts: {error:#}");
            }
        }
    }

    /// Consecutive checkpoint failures for a project (0 when healthy).
    pub fn consecutive_failures(&self, project_id: &str) -> u32 {
        self.projects
            .get(project_id)
            .map_or(0, |p| p.consecutive_checkpoint_failures)
    }

    /// Every project's state, by project id.
    pub fn snapshot(&self) -> BTreeMap<String, ProjectStorage> {
        self.projects
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }
}

/// Data files in the current snapshot of `lake`, and rows inlined in its
/// catalog. Inlined rows live in one catalog table per table and schema
/// version, listed in `ducklake_inlined_data_tables`; a row is current while
/// its `_ducklake_end_snapshot` is NULL.
pub(crate) fn catalog_counts(conn: &Connection) -> anyhow::Result<(u64, u64)> {
    let files = data_file_count(conn)?;
    let mut inlined: i64 = 0;
    let metadata = "__ducklake_metadata_lake";
    let tables: Vec<String> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT table_name FROM \"{metadata}\".ducklake_inlined_data_tables"
        ))?;
        stmt.query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<_, _>>()?
    };
    for table in tables {
        let quoted = table.replace('"', "\"\"");
        let rows: i64 = conn.query_row(
            &format!("SELECT count(*) FROM \"{metadata}\".\"{quoted}\" WHERE _ducklake_end_snapshot IS NULL"),
            [],
            |row| row.get(0),
        )?;
        inlined += rows;
    }
    Ok((files, u64::try_from(inlined).unwrap_or_default()))
}

/// Data files in the current snapshot of `lake`.
pub(crate) fn data_file_count(conn: &Connection) -> anyhow::Result<u64> {
    let files: i64 = conn.query_row(
        "SELECT coalesce(sum(file_count), 0)::BIGINT FROM ducklake_table_info('lake')",
        [],
        |row| row.get(0),
    )?;
    Ok(u64::try_from(files).unwrap_or_default())
}

/// The soft open-files limit, read once. sigiro never raises it; maintenance
/// sizes its work to fit inside it.
pub(crate) fn open_files_limit() -> Option<u64> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit writes one `rlimit` through a valid pointer.
    let status = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) };
    (status == 0 && limit.rlim_cur != libc::RLIM_INFINITY).then(|| {
        #[allow(clippy::useless_conversion)]
        u64::from(limit.rlim_cur)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_counts_until_the_next_success() {
        let health = StorageHealth::new();
        health.record_checkpoint("p", Some("Too many open files"));
        health.record_checkpoint("p", Some("Too many open files"));
        let p = &health.snapshot()["p"];
        assert_eq!(p.consecutive_checkpoint_failures, 2);
        assert_eq!(p.last_checkpoint_ok, Some(false));
        assert_eq!(
            p.last_checkpoint_error.as_deref(),
            Some("Too many open files")
        );
        health.record_checkpoint("p", None);
        let p = &health.snapshot()["p"];
        assert_eq!(
            (
                p.consecutive_checkpoint_failures,
                p.last_checkpoint_ok,
                p.last_checkpoint_error.clone()
            ),
            (0, Some(true), None)
        );
    }

    #[test]
    fn the_open_files_limit_is_readable() {
        assert!(open_files_limit().is_some_and(|limit| limit > 0));
    }
}
