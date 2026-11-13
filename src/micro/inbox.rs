use super::Frame;
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::path::Path;

/// SQLite is raw truth. Receipts derive from the row itself, not a separate
/// success log that could commit independently of the original protobuf.
#[derive(Clone)]
pub struct Inbox {
    connection: async_rusqlite::Connection,
    max_bytes: u64,
    max_batches: u64,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Acceptance {
    Stored,
    Quota,
    Conflict,
}
pub struct Pending {
    pub row: i64,
    pub frame: Frame,
}
impl Inbox {
    pub async fn open(data_dir: &Path, max_bytes: u64, max_batches: u64) -> Result<Self> {
        let dir = data_dir.join(".micro");
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                std::fs::metadata(&dir)?.permissions().mode() & 0o077 == 0,
                "micro data directory must be private"
            );
        }
        let path = dir.join("inbox.sqlite");
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(&path)?;
        file.sync_all()?;
        drop(file);
        std::fs::File::open(&dir)?.sync_all()?;
        std::fs::File::open(data_dir)?.sync_all()?;
        let connection = async_rusqlite::Connection::builder()
            .channel_size(8)
            .open(path)
            .await?;
        let version: u32 = connection
            .call(|connection| {
                connection.busy_timeout(std::time::Duration::from_secs(5))?;
                connection.pragma_update(None, "journal_mode", "WAL")?;
                connection.pragma_update(None, "synchronous", "FULL")?;
                connection.pragma_query_value(None, "user_version", |row| row.get(0))
            })
            .await?;
        ensure!(version <= 1, "micro inbox schema is newer than this binary");
        connection.call(|connection| connection.execute_batch("BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS batches (
                row INTEGER PRIMARY KEY, device TEXT NOT NULL, batch_id BLOB NOT NULL CHECK(length(batch_id)=16),
                project TEXT NOT NULL, signal INTEGER NOT NULL CHECK(signal IN(1,2,3)), payload BLOB NOT NULL,
                sha256 BLOB NOT NULL CHECK(length(sha256)=32),
                state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN('pending','indexed','quarantined')), diagnostic TEXT,
                attempts INTEGER NOT NULL DEFAULT 0, next_attempt INTEGER NOT NULL DEFAULT 0,
                UNIQUE(device,batch_id));
            CREATE INDEX IF NOT EXISTS pending_batches ON batches(state,row);
            CREATE TABLE IF NOT EXISTS usage(singleton INTEGER PRIMARY KEY CHECK(singleton=1), bytes INTEGER NOT NULL, batches INTEGER NOT NULL);
            INSERT OR IGNORE INTO usage SELECT 1,COALESCE(SUM(length(payload)),0),COUNT(*) FROM batches;
            PRAGMA user_version=1; COMMIT;")).await?;
        Ok(Self {
            connection,
            max_bytes,
            max_batches,
        })
    }
    pub async fn accept(&self, frame: Frame) -> Result<Acceptance> {
        let (max_bytes, max_batches) = (self.max_bytes, self.max_batches);
        self.connection
            .call(move |connection| {
                Ok::<_, rusqlite::Error>(accept(connection, &frame, max_bytes, max_batches))
            })
            .await?
    }
    pub async fn next(&self) -> Result<Option<Pending>> {
        self.connection
            .call(|connection| Ok::<_, rusqlite::Error>(next(connection)))
            .await?
    }
    pub async fn retry(&self, row: i64, diagnostic: String) -> Result<()> {
        self.connection.call(move |connection| connection.execute("UPDATE batches SET attempts=attempts+1,next_attempt=unixepoch()+MIN(60,2*(attempts+1)),diagnostic=?1 WHERE row=?2 AND state='pending'", params![diagnostic,row])).await?;
        Ok(())
    }
    pub async fn finish(&self, row: i64, diagnostic: Option<String>) -> Result<()> {
        let state = if diagnostic.is_some() {
            "quarantined"
        } else {
            "indexed"
        };
        self.connection
            .call(move |connection| {
                connection.execute(
                    "UPDATE batches SET state=?1,diagnostic=?2 WHERE row=?3 AND state='pending'",
                    params![state, diagnostic, row],
                )
            })
            .await?;
        Ok(())
    }
}

// Domain policy stays in one SQLite transaction; the library owns thread and
// connection scheduling. A cancelled caller cannot publish a premature receipt.
fn accept(
    connection: &mut Connection,
    frame: &Frame,
    max_bytes: u64,
    max_batches: u64,
) -> Result<Acceptance> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing: Option<(String, u8, Vec<u8>, Vec<u8>)> = transaction
        .query_row(
            "SELECT project,signal,payload,sha256 FROM batches WHERE device=?1 AND batch_id=?2",
            params![frame.device, frame.id.0.as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    if let Some((project, signal, payload, checksum)) = existing {
        ensure!(
            Sha256::digest(&payload).as_slice() == checksum.as_slice(),
            "micro raw checksum mismatch"
        );
        return Ok(
            if project == frame.project && signal == frame.signal as u8 && payload == frame.payload
            {
                Acceptance::Stored
            } else {
                Acceptance::Conflict
            },
        );
    }
    let (bytes, batches): (u64, u64) = transaction.query_row(
        "SELECT bytes,batches FROM usage WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if bytes.saturating_add(frame.payload.len() as u64) > max_bytes || batches >= max_batches {
        return Ok(Acceptance::Quota);
    }
    transaction.execute(
            "INSERT INTO batches(device,batch_id,project,signal,payload,sha256) VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                frame.device,
                frame.id.0.as_slice(),
                frame.project,
                frame.signal as u8,
                frame.payload,
                Sha256::digest(&frame.payload).as_slice()
            ],
        )?;
    transaction.execute(
        "UPDATE usage SET bytes=bytes+?1,batches=batches+1 WHERE singleton=1",
        [frame.payload.len() as u64],
    )?;
    transaction
        .commit()
        .context("commit micro raw batch before receipt")?;
    Ok(Acceptance::Stored)
}
fn next(connection: &Connection) -> Result<Option<Pending>> {
    type RawRow = (i64, String, Vec<u8>, String, u8, Vec<u8>, Vec<u8>);
    let row: Option<RawRow> = connection.query_row("SELECT row,device,batch_id,project,signal,payload,sha256 FROM batches WHERE state='pending' AND next_attempt<=unixepoch() ORDER BY row LIMIT 1", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?))).optional()?;
    row.map(|(row, device, id, project, signal, payload, checksum)| {
        ensure!(
            Sha256::digest(&payload).as_slice() == checksum.as_slice(),
            "micro raw checksum mismatch"
        );
        Ok(Pending {
            row,
            frame: Frame {
                device,
                id: otlp_outbox::BatchId(
                    id.try_into()
                        .map_err(|_| anyhow::anyhow!("corrupt inbox batch ID"))?,
                ),
                project,
                signal: signal
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("corrupt inbox signal"))?,
                payload,
            },
        })
    })
    .transpose()
}
