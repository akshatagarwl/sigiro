//! Write-Ahead Log for crash recovery.
//!
//! Before each DuckDB flush, buffered RecordBatches are serialized to a WAL
//! file. After successful DuckDB insert the WAL file is deleted. On startup,
//! any leftover WAL files are replayed into DuckDB and then removed.
//!
//! File format (per WAL file):
//!   Repeated entries of:
//!     [u32 table_name_len][table_name UTF-8][u32 ipc_len][Arrow IPC stream bytes]
//!
//! WAL directory layout:
//!   {data_dir}/wal/{project_id}/{epoch_nanos}.wal

use anyhow::{Context, Result};
use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use std::fs;
use std::io::{BufWriter, Cursor, Read, Write};
use std::path::{Path, PathBuf};

/// Write a set of table→batches entries to a new WAL file.
/// Returns the path of the created file (caller deletes after successful flush).
pub fn write_wal(
    wal_dir: &Path,
    project_id: &str,
    entries: &[(&str, &[RecordBatch])],
) -> Result<PathBuf> {
    let project_dir = wal_dir.join(project_id);
    fs::create_dir_all(&project_dir)
        .with_context(|| format!("failed to create WAL dir: {}", project_dir.display()))?;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_nanos();
    let path = project_dir.join(format!("{ts}.wal"));

    let file = fs::File::create(&path)
        .with_context(|| format!("failed to create WAL file: {}", path.display()))?;
    let mut w = BufWriter::new(file);

    for &(table_name, batches) in entries {
        if batches.is_empty() {
            continue;
        }

        let name_bytes = table_name.as_bytes();
        w.write_all(&(name_bytes.len() as u32).to_le_bytes())?;
        w.write_all(name_bytes)?;

        // Serialize batches to Arrow IPC stream
        let mut ipc_buf = Vec::new();
        {
            let schema = batches[0].schema();
            let mut ipc_writer = StreamWriter::try_new(&mut ipc_buf, &schema)?;
            for batch in batches {
                ipc_writer.write(batch)?;
            }
            ipc_writer.finish()?;
        }

        w.write_all(&(ipc_buf.len() as u32).to_le_bytes())?;
        w.write_all(&ipc_buf)?;
    }

    w.flush()?;
    // fsync for durability
    w.into_inner()
        .map_err(|e| anyhow::anyhow!("flush error: {e}"))?
        .sync_all()?;

    Ok(path)
}

/// Read a WAL file back into table→batches entries.
pub fn read_wal(path: &Path) -> Result<Vec<(String, Vec<RecordBatch>)>> {
    let data =
        fs::read(path).with_context(|| format!("failed to read WAL file: {}", path.display()))?;
    let mut cursor = Cursor::new(&data);
    let mut entries = Vec::new();

    while (cursor.position() as usize) < data.len() {
        // Read table name
        let mut len_buf = [0u8; 4];
        cursor.read_exact(&mut len_buf)?;
        let name_len = u32::from_le_bytes(len_buf) as usize;

        let mut name_buf = vec![0u8; name_len];
        cursor.read_exact(&mut name_buf)?;
        let table_name = String::from_utf8(name_buf).context("invalid UTF-8 in WAL table name")?;

        // Read IPC stream
        cursor.read_exact(&mut len_buf)?;
        let ipc_len = u32::from_le_bytes(len_buf) as usize;

        let pos = cursor.position() as usize;
        let ipc_data = &data[pos..pos + ipc_len];
        cursor.set_position((pos + ipc_len) as u64);

        let reader = StreamReader::try_new(Cursor::new(ipc_data), None)?;
        let batches: Vec<RecordBatch> = reader
            .into_iter()
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("failed to read IPC batches for {table_name}"))?;

        entries.push((table_name, batches));
    }

    Ok(entries)
}

/// Replay all WAL files for all projects into DuckDB, then delete them.
pub fn replay_all(wal_dir: &Path, pool: &crate::pool::ProjectPool) -> Result<u64> {
    if !wal_dir.exists() {
        return Ok(0);
    }

    let mut total_rows = 0u64;

    let project_dirs = fs::read_dir(wal_dir)
        .with_context(|| format!("failed to read WAL dir: {}", wal_dir.display()))?;

    for entry in project_dirs {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let project_id = entry
            .file_name()
            .to_str()
            .context("non-UTF8 WAL project dir")?
            .to_string();

        let conn = pool
            .get_or_create(&project_id)
            .with_context(|| format!("failed to open DB for WAL replay: {project_id}"))?;
        let conn = conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;

        // Sort WAL files by name (timestamp order) for deterministic replay
        let mut wal_files: Vec<PathBuf> = fs::read_dir(entry.path())?
            .filter_map(std::result::Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "wal"))
            .map(|e| e.path())
            .collect();
        wal_files.sort();

        for wal_path in &wal_files {
            match read_wal(wal_path) {
                Ok(entries) => {
                    for (table_name, batches) in &entries {
                        if batches.is_empty() {
                            continue;
                        }
                        let mut appender = conn.appender(table_name).with_context(|| {
                            format!("WAL replay: failed to create appender for {table_name}")
                        })?;
                        for batch in batches {
                            total_rows += batch.num_rows() as u64;
                            appender
                                .append_record_batch(batch.clone())
                                .with_context(|| {
                                    format!("WAL replay: failed to append to {table_name}")
                                })?;
                        }
                    }
                    // Appenders drop here → auto-flush to DuckDB
                }
                Err(e) => {
                    tracing::error!(
                        path = %wal_path.display(),
                        "corrupt WAL file, skipping: {e:#}"
                    );
                }
            }

            // Delete replayed WAL file
            if let Err(e) = fs::remove_file(wal_path) {
                tracing::warn!(
                    path = %wal_path.display(),
                    "failed to remove WAL file after replay: {e}"
                );
            }
        }

        // Remove empty project WAL directory (best-effort, non-empty is fine)
        if entry
            .path()
            .read_dir()
            .is_ok_and(|mut d| d.next().is_none())
        {
            if let Err(e) = fs::remove_dir(entry.path()) {
                tracing::debug!(path = %entry.path().display(), "failed to remove empty WAL dir: {e}");
            }
        }
    }

    Ok(total_rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int32Array, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc;

    fn make_batch(n: usize) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, true),
        ]));
        let ids: Vec<i32> = (0..n as i32).collect();
        let names: Vec<&str> = (0..n).map(|_| "test").collect();
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(ids)),
                Arc::new(StringArray::from(names)),
            ],
        )
        .unwrap()
    }

    #[test]
    fn roundtrip_wal() {
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("wal");

        let spans = [make_batch(5)];
        let logs = [make_batch(3)];
        let entries: Vec<(&str, &[RecordBatch])> =
            vec![("sigiro_spans", &spans), ("sigiro_logs", &logs)];

        let path = write_wal(&wal_dir, "proj1", &entries).unwrap();
        assert!(path.exists());

        let read_back = read_wal(&path).unwrap();
        assert_eq!(read_back.len(), 2);
        assert_eq!(read_back[0].0, "sigiro_spans");
        assert_eq!(read_back[0].1.len(), 1);
        assert_eq!(read_back[0].1[0].num_rows(), 5);
        assert_eq!(read_back[1].0, "sigiro_logs");
        assert_eq!(read_back[1].1[0].num_rows(), 3);
    }

    #[test]
    fn empty_batches_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("wal");

        let logs = [make_batch(2)];
        let entries: Vec<(&str, &[RecordBatch])> =
            vec![("sigiro_spans", &[]), ("sigiro_logs", &logs)];

        let path = write_wal(&wal_dir, "proj1", &entries).unwrap();
        let read_back = read_wal(&path).unwrap();
        // Empty spans entry should be skipped
        assert_eq!(read_back.len(), 1);
        assert_eq!(read_back[0].0, "sigiro_logs");
    }

    #[test]
    fn multiple_batches_per_table() {
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("wal");

        let batches = [make_batch(3), make_batch(7)];
        let entries: Vec<(&str, &[RecordBatch])> = vec![("sigiro_spans", &batches)];

        let path = write_wal(&wal_dir, "proj1", &entries).unwrap();
        let read_back = read_wal(&path).unwrap();
        assert_eq!(read_back[0].1.len(), 2);
        assert_eq!(read_back[0].1[0].num_rows(), 3);
        assert_eq!(read_back[0].1[1].num_rows(), 7);
    }
}
