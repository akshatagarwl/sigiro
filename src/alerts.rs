use crate::admin::AdminConnection;
use crate::declared_schema::DeclaredTable;
use anyhow::{Context, Result};
use arrow::array::{Array, StringArray, TimestampMicrosecondArray};
use arrow::record_batch::RecordBatch;
use parking_lot::Mutex;
use prefixed_api_key::sha2::{Digest, Sha256};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub(crate) const OOM_FIRING_EVENT: &str = "container.oom_killed";
pub(crate) const OOM_RECOVERED_EVENT: &str = "container.oom_recovered";
const OOM_RULE_ID: &str = "container-oom";

/// Non-empty bearer credential used only for alert webhook delivery.
///
/// Deliberately omits `Debug`, `Display`, and Serde traits so configuration
/// logging and generic serialization cannot disclose the credential.
#[nutype::nutype(validate(not_empty), derive(Clone, AsRef))]
pub struct AlertWebhookToken(String);

#[derive(Clone)]
pub struct AlertStore {
    conn: Arc<Mutex<AdminConnection>>,
    /// Refreshed by [`run_delivery`]; `None` while delivery is not running.
    delivery_health: Arc<Mutex<Option<DeliverySummary>>>,
}

#[derive(Debug, Clone)]
pub struct AlertEvent {
    pub(crate) project_id: String,
    pub(crate) event_name: String,
    pub(crate) timestamp_us: i64,
    pub(crate) service_name: String,
    pub(crate) container_name: String,
    pub(crate) container_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AlertInstance {
    pub(crate) fingerprint: String,
    pub(crate) rule_id: String,
    pub(crate) status: String,
    pub(crate) service_name: String,
    pub(crate) container_name: String,
    pub(crate) starts_at: String,
    pub(crate) ends_at: Option<String>,
    pub(crate) occurrence_count: u64,
}

#[derive(Debug, Clone)]
pub struct PendingNotification {
    pub(crate) id: i64,
    pub(crate) payload: String,
}

#[derive(Clone)]
pub struct DeliveryConfig {
    url: url::Url,
    token: Option<AlertWebhookToken>,
}

impl fmt::Debug for DeliveryConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeliveryConfig")
            .field("configured", &true)
            .finish_non_exhaustive()
    }
}

impl DeliveryConfig {
    pub fn from_env() -> Result<Option<Self>> {
        let raw_url = crate::config::ALERT_WEBHOOK_URL.value()?;
        if raw_url.is_empty() {
            return Ok(None);
        }
        let url = url::Url::parse(&raw_url).context("SIGIRO_ALERT_WEBHOOK_URL is not a URL")?;
        anyhow::ensure!(
            url.username().is_empty() && url.password().is_none(),
            "SIGIRO_ALERT_WEBHOOK_URL must not contain credentials"
        );
        let loopback_http = url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        anyhow::ensure!(
            url.scheme() == "https" || loopback_http,
            "SIGIRO_ALERT_WEBHOOK_URL must use HTTPS unless it targets loopback"
        );
        let token = crate::config::alert_webhook_token()?;
        Ok(Some(Self { url, token }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessResult {
    pub(crate) accepted_events: usize,
    pub(crate) notifications_enqueued: usize,
}

struct AlertRow {
    fingerprint: String,
    rule_id: String,
    status: String,
    service_name: String,
    container_name: String,
    starts_at_us: i64,
    ends_at_us: Option<i64>,
    occurrence_count: i64,
}

/// Lifecycle of one alert fingerprint. "No row yet" is `Option::None`.
///
/// Every transition lives in [`step`]; SQLite and PostgreSQL only load and
/// save this value, so the two backends cannot disagree about the rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlertState {
    /// A recover arrived before any fire it could close. OTLP batches from
    /// different requests race, so the fire may still be on its way; keeping
    /// the recover lets that late fire resolve instead of firing forever.
    RecoveredFirst {
        recovered_at_us: i64,
        occurrence_count: u64,
    },
    Firing {
        starts_at_us: i64,
        occurrence_count: u64,
    },
    Resolved {
        starts_at_us: i64,
        ends_at_us: i64,
        occurrence_count: u64,
        /// A recover newer than this episode, seen before its fire. The late
        /// fire then closes silently instead of reopening forever.
        recovered_ahead_us: Option<i64>,
    },
}

/// One accepted (deduplicated) OOM event, reduced to what the machine needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlertInput {
    Fire { at_us: i64 },
    Recover { at_us: i64 },
}

/// What [`step`] decided for one event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlertStep {
    /// Stale or out-of-episode event: nothing is written.
    Ignore,
    /// Persist `next`; `notify` enqueues one webhook for the new state.
    Move { next: AlertState, notify: bool },
}

const STATUS_RECOVERED_FIRST: &str = "recovered_first";
const STATUS_FIRING: &str = "firing";
const STATUS_RESOLVED: &str = "resolved";

impl AlertState {
    fn status(self) -> &'static str {
        match self {
            Self::RecoveredFirst { .. } => STATUS_RECOVERED_FIRST,
            Self::Firing { .. } => STATUS_FIRING,
            Self::Resolved { .. } => STATUS_RESOLVED,
        }
    }

    fn occurrence_count(self) -> u64 {
        match self {
            Self::RecoveredFirst {
                occurrence_count, ..
            }
            | Self::Firing {
                occurrence_count, ..
            }
            | Self::Resolved {
                occurrence_count, ..
            } => occurrence_count,
        }
    }

    /// `(starts_at_us, ends_at_us)` as stored. A recovered-first row has no
    /// fire yet, so both columns hold the recover time.
    fn bounds(self) -> (i64, Option<i64>) {
        match self {
            Self::RecoveredFirst {
                recovered_at_us, ..
            } => (recovered_at_us, Some(recovered_at_us)),
            Self::Firing { starts_at_us, .. } => (starts_at_us, None),
            Self::Resolved {
                starts_at_us,
                ends_at_us,
                ..
            } => (starts_at_us, Some(ends_at_us)),
        }
    }

    fn recovered_ahead_us(self) -> Option<i64> {
        match self {
            Self::Resolved {
                recovered_ahead_us, ..
            } => recovered_ahead_us,
            Self::RecoveredFirst { .. } | Self::Firing { .. } => None,
        }
    }

    fn decode(
        status: &str,
        starts_at_us: i64,
        ends_at_us: Option<i64>,
        count: i64,
        recovered_ahead_us: Option<i64>,
    ) -> Result<Self> {
        let occurrence_count = u64::try_from(count).context("negative alert occurrence count")?;
        match (status, ends_at_us) {
            (STATUS_RECOVERED_FIRST, _) => Ok(Self::RecoveredFirst {
                recovered_at_us: ends_at_us.unwrap_or(starts_at_us),
                occurrence_count,
            }),
            (STATUS_FIRING, None) => Ok(Self::Firing {
                starts_at_us,
                occurrence_count,
            }),
            (STATUS_RESOLVED, Some(ends_at_us)) => Ok(Self::Resolved {
                starts_at_us,
                ends_at_us,
                occurrence_count,
                recovered_ahead_us,
            }),
            (status, ends_at_us) => anyhow::bail!(
                "alert row has inconsistent state: status {status:?}, ends_at_us {ends_at_us:?}"
            ),
        }
    }
}

/// The whole alert lifecycle as one pure function.
///
/// | from            | event                | to              | webhook |
/// |-----------------|----------------------|-----------------|---------|
/// | none            | fire t               | Firing(t, 1)    | firing  |
/// | none            | recover t            | RecoveredFirst  | —       |
/// | RecoveredFirst r| fire t ≤ r           | Resolved(t, r)  | —       |
/// | RecoveredFirst r| fire t > r           | Firing(t)       | firing  |
/// | RecoveredFirst r| recover t            | RecoveredFirst(max) | —   |
/// | Firing s        | fire                 | Firing, count+1 | —       |
/// | Firing s        | recover t ≥ s        | Resolved(s, t)  | resolved|
/// | Firing s        | recover t < s        | ignore          | —       |
/// | Resolved s..e   | fire t ≤ e           | ignore          | —       |
/// | Resolved, ahead r | fire e < t ≤ r     | Resolved(t, r)  | —       |
/// | Resolved s..e   | fire t > e (and > r) | Firing(t)       | firing  |
/// | Resolved s..e   | recover t ≤ e        | ignore          | —       |
/// | Resolved s..e   | recover t > e        | Resolved, ahead max(r, t) | — |
pub(crate) fn step(state: Option<AlertState>, input: AlertInput) -> AlertStep {
    use AlertInput::{Fire, Recover};
    use AlertState::{Firing, RecoveredFirst, Resolved};
    let count = state.map_or(0, AlertState::occurrence_count);
    let fire = |at_us| AlertStep::Move {
        next: Firing {
            starts_at_us: at_us,
            occurrence_count: count + 1,
        },
        notify: true,
    };
    let closed_silently = |starts_at_us, ends_at_us| AlertStep::Move {
        next: Resolved {
            starts_at_us,
            ends_at_us,
            occurrence_count: count + 1,
            recovered_ahead_us: None,
        },
        notify: false,
    };
    let recovered_first = |at_us| AlertStep::Move {
        next: RecoveredFirst {
            recovered_at_us: at_us,
            occurrence_count: count,
        },
        notify: false,
    };
    match (state, input) {
        (None, Fire { at_us }) => fire(at_us),
        (None, Recover { at_us }) => recovered_first(at_us),
        (
            Some(RecoveredFirst {
                recovered_at_us, ..
            }),
            Fire { at_us },
        ) if at_us <= recovered_at_us => closed_silently(at_us, recovered_at_us),
        (Some(RecoveredFirst { .. }), Fire { at_us }) => fire(at_us),
        (
            Some(RecoveredFirst {
                recovered_at_us, ..
            }),
            Recover { at_us },
        ) => recovered_first(recovered_at_us.max(at_us)),
        (Some(Firing { starts_at_us, .. }), Fire { .. }) => AlertStep::Move {
            next: Firing {
                starts_at_us,
                occurrence_count: count + 1,
            },
            notify: false,
        },
        (Some(Firing { starts_at_us, .. }), Recover { at_us }) if at_us >= starts_at_us => {
            AlertStep::Move {
                next: Resolved {
                    starts_at_us,
                    ends_at_us: at_us,
                    occurrence_count: count,
                    recovered_ahead_us: None,
                },
                notify: true,
            }
        }
        (Some(Firing { .. }), Recover { .. }) => AlertStep::Ignore,
        (
            Some(Resolved {
                ends_at_us,
                recovered_ahead_us: Some(ahead),
                ..
            }),
            Fire { at_us },
        ) if at_us > ends_at_us && at_us <= ahead => closed_silently(at_us, ahead),
        (Some(Resolved { ends_at_us, .. }), Fire { at_us }) if at_us > ends_at_us => fire(at_us),
        (
            Some(Resolved {
                starts_at_us,
                ends_at_us,
                occurrence_count,
                recovered_ahead_us,
            }),
            Recover { at_us },
        ) if at_us > ends_at_us => AlertStep::Move {
            next: Resolved {
                starts_at_us,
                ends_at_us,
                occurrence_count,
                recovered_ahead_us: Some(
                    recovered_ahead_us.map_or(at_us, |ahead| ahead.max(at_us)),
                ),
            },
            notify: false,
        },
        (Some(Resolved { .. }), Fire { .. } | Recover { .. }) => AlertStep::Ignore,
    }
}

/// The storage a backend must offer the alert machine. Implemented by a
/// SQLite and a PostgreSQL transaction; neither holds any transition logic.
trait AlertPersistence {
    /// Record the occurrence; `false` means it was already seen (dedupe).
    fn record_occurrence(&mut self, event: &AlertEvent, fingerprint: &str) -> Result<bool>;
    fn load(&mut self, project_id: &str, fingerprint: &str) -> Result<Option<AlertState>>;
    fn save(&mut self, event: &AlertEvent, fingerprint: &str, state: AlertState) -> Result<()>;
    fn enqueue(
        &mut self,
        event: &AlertEvent,
        fingerprint: &str,
        transition: &str,
        payload: &str,
    ) -> Result<()>;
}

fn apply_events(store: &mut impl AlertPersistence, events: &[AlertEvent]) -> Result<ProcessResult> {
    let mut accepted_events = 0;
    let mut notifications_enqueued = 0;
    for event in events {
        let input = match event.event_name.as_str() {
            OOM_FIRING_EVENT => AlertInput::Fire {
                at_us: event.timestamp_us,
            },
            OOM_RECOVERED_EVENT => AlertInput::Recover {
                at_us: event.timestamp_us,
            },
            _ => continue,
        };
        let fingerprint = alert_fingerprint(event);
        if !store.record_occurrence(event, &fingerprint)? {
            continue;
        }
        accepted_events += 1;
        let state = store.load(&event.project_id, &fingerprint)?;
        let AlertStep::Move { next, notify } = step(state, input) else {
            continue;
        };
        store.save(event, &fingerprint, next)?;
        if notify {
            let payload = webhook_payload(event, &fingerprint, next)?;
            store.enqueue(event, &fingerprint, next.status(), &payload)?;
            notifications_enqueued += 1;
        }
    }
    Ok(ProcessResult {
        accepted_events,
        notifications_enqueued,
    })
}

impl AlertPersistence for rusqlite::Transaction<'_> {
    fn record_occurrence(&mut self, event: &AlertEvent, fingerprint: &str) -> Result<bool> {
        let inserted = self.execute(
            "INSERT OR IGNORE INTO alert_occurrences (project_id, occurrence_key, fingerprint, observed_at_us) VALUES (?1, ?2, ?3, ?4)",
            params![event.project_id, occurrence_key(event), fingerprint, event.timestamp_us],
        )?;
        Ok(inserted == 1)
    }

    fn load(&mut self, project_id: &str, fingerprint: &str) -> Result<Option<AlertState>> {
        self.query_row(
            "SELECT status, starts_at_us, ends_at_us, occurrence_count, recovered_ahead_us FROM alert_instances WHERE project_id = ?1 AND fingerprint = ?2",
            params![project_id, fingerprint],
            |row| Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .optional()
        .context("failed to read alert state")?
        .map(|(status, starts, ends, count, ahead)| AlertState::decode(&status, starts, ends, count, ahead))
        .transpose()
    }

    fn save(&mut self, event: &AlertEvent, fingerprint: &str, state: AlertState) -> Result<()> {
        let (starts_at_us, ends_at_us) = state.bounds();
        let count =
            i64::try_from(state.occurrence_count()).context("alert occurrence count overflow")?;
        self.execute(
            "INSERT INTO alert_instances (project_id, fingerprint, rule_id, status, service_name, container_name, starts_at_us, ends_at_us, occurrence_count, updated_at_us, recovered_ahead_us)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT (project_id, fingerprint) DO UPDATE SET status = excluded.status, service_name = excluded.service_name,
               container_name = excluded.container_name, starts_at_us = excluded.starts_at_us, ends_at_us = excluded.ends_at_us,
               occurrence_count = excluded.occurrence_count, updated_at_us = excluded.updated_at_us,
               recovered_ahead_us = excluded.recovered_ahead_us",
            params![
                event.project_id,
                fingerprint,
                OOM_RULE_ID,
                state.status(),
                event.service_name,
                event.container_name,
                starts_at_us,
                ends_at_us,
                count,
                event.timestamp_us,
                state.recovered_ahead_us()
            ],
        )?;
        Ok(())
    }

    fn enqueue(
        &mut self,
        event: &AlertEvent,
        fingerprint: &str,
        transition: &str,
        payload: &str,
    ) -> Result<()> {
        self.execute(
            "INSERT INTO alert_outbox (project_id, fingerprint, transition, payload_json, next_attempt_at_us, enqueued_at_us) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![event.project_id, fingerprint, transition, payload, now_us()],
        )?;
        Ok(())
    }
}

impl AlertPersistence for postgres::Transaction<'_> {
    fn record_occurrence(&mut self, event: &AlertEvent, fingerprint: &str) -> Result<bool> {
        let inserted = self.execute(
            "INSERT INTO sigiro_admin.alert_occurrences (project_id, occurrence_key, fingerprint, observed_at_us) VALUES ($1, $2, $3, $4) ON CONFLICT (project_id, occurrence_key) DO NOTHING",
            &[&event.project_id, &occurrence_key(event), &fingerprint, &event.timestamp_us],
        )?;
        Ok(inserted == 1)
    }

    fn load(&mut self, project_id: &str, fingerprint: &str) -> Result<Option<AlertState>> {
        self.query_opt(
            "SELECT status, starts_at_us, ends_at_us, occurrence_count, recovered_ahead_us FROM sigiro_admin.alert_instances WHERE project_id = $1 AND fingerprint = $2",
            &[&project_id, &fingerprint],
        )
        .context("failed to read alert state")?
        .map(|row| AlertState::decode(row.get::<_, &str>(0), row.get(1), row.get(2), row.get(3), row.get(4)))
        .transpose()
    }

    fn save(&mut self, event: &AlertEvent, fingerprint: &str, state: AlertState) -> Result<()> {
        let (starts_at_us, ends_at_us) = state.bounds();
        let count =
            i64::try_from(state.occurrence_count()).context("alert occurrence count overflow")?;
        self.execute(
            "INSERT INTO sigiro_admin.alert_instances (project_id, fingerprint, rule_id, status, service_name, container_name, starts_at_us, ends_at_us, occurrence_count, updated_at_us, recovered_ahead_us)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             ON CONFLICT (project_id, fingerprint) DO UPDATE SET status = excluded.status, service_name = excluded.service_name,
               container_name = excluded.container_name, starts_at_us = excluded.starts_at_us, ends_at_us = excluded.ends_at_us,
               occurrence_count = excluded.occurrence_count, updated_at_us = excluded.updated_at_us,
               recovered_ahead_us = excluded.recovered_ahead_us",
            &[
                &event.project_id,
                &fingerprint,
                &OOM_RULE_ID,
                &state.status(),
                &event.service_name,
                &event.container_name,
                &starts_at_us,
                &ends_at_us,
                &count,
                &event.timestamp_us,
                &state.recovered_ahead_us(),
            ],
        )?;
        Ok(())
    }

    fn enqueue(
        &mut self,
        event: &AlertEvent,
        fingerprint: &str,
        transition: &str,
        payload: &str,
    ) -> Result<()> {
        self.execute(
            "INSERT INTO sigiro_admin.alert_outbox (project_id, fingerprint, transition, payload_json, next_attempt_at_us, enqueued_at_us) VALUES ($1, $2, $3, $4, $5, $5)",
            &[&event.project_id, &fingerprint, &transition, &payload, &now_us()],
        )?;
        Ok(())
    }
}

impl AlertStore {
    pub fn new(conn: Arc<Mutex<AdminConnection>>) -> Result<Self> {
        Ok(Self {
            conn,
            delivery_health: Arc::default(),
        })
    }

    pub(crate) fn admin_connection(&self) -> Arc<Mutex<AdminConnection>> {
        Arc::clone(&self.conn)
    }

    pub(crate) fn process_log_batch(
        &self,
        project_id: &str,
        batch: &RecordBatch,
    ) -> Result<ProcessResult> {
        self.process_events(&events_from_log_batch(project_id, batch)?)
    }

    pub(crate) fn process_events(&self, events: &[AlertEvent]) -> Result<ProcessResult> {
        match &mut *self.conn.lock() {
            AdminConnection::Local(conn) => process_events_sqlite(conn, events),
            AdminConnection::Hosted(client) => process_events_postgres(client, events),
        }
    }

    pub(crate) fn list(
        &self,
        project_id: &str,
        status: Option<&str>,
    ) -> Result<Vec<AlertInstance>> {
        match &mut *self.conn.lock() {
            AdminConnection::Local(conn) => list_sqlite(conn, project_id, status),
            AdminConnection::Hosted(client) => list_postgres(client, project_id, status),
        }
    }

    pub(crate) fn next_due(&self, now_us: i64) -> Result<Option<PendingNotification>> {
        match &mut *self.conn.lock() {
            AdminConnection::Local(conn) => conn.query_row(
                "SELECT id, payload_json FROM alert_outbox WHERE sent_at_us IS NULL AND dead_at_us IS NULL AND next_attempt_at_us <= ?1 ORDER BY id LIMIT 1",
                [now_us],
                |row| Ok(PendingNotification { id: row.get(0)?, payload: row.get(1)? }),
            ).optional().context("failed to read alert outbox"),
            AdminConnection::Hosted(client) => Ok(client.query_opt(
                "SELECT id, payload_json FROM sigiro_admin.alert_outbox WHERE sent_at_us IS NULL AND dead_at_us IS NULL AND next_attempt_at_us <= $1 ORDER BY id LIMIT 1",
                &[&now_us],
            ).context("failed to read alert outbox")?.map(|row| PendingNotification {
                id: row.get(0), payload: row.get(1),
            })),
        }
    }

    pub(crate) fn mark_sent(&self, id: i64, sent_at_us: i64) -> Result<()> {
        let mut conn = self.conn.lock();
        let changed = match &mut *conn {
            AdminConnection::Local(conn) => conn.execute(
                "UPDATE alert_outbox SET sent_at_us = ?2, last_error = NULL WHERE id = ?1 AND sent_at_us IS NULL AND dead_at_us IS NULL",
                params![id, sent_at_us],
            )? as u64,
            AdminConnection::Hosted(client) => client.execute(
                "UPDATE sigiro_admin.alert_outbox SET sent_at_us = $2, last_error = NULL WHERE id = $1 AND sent_at_us IS NULL AND dead_at_us IS NULL",
                &[&id, &sent_at_us],
            )?,
        };
        ensure_was_queued(&mut conn, id, changed)
    }

    /// Record a failed attempt and move the row to `Queued` (retry later) or
    /// `Dead`, as [`delivery_step`] decides. Returns the decision.
    pub(crate) fn mark_failed(
        &self,
        id: i64,
        now_us: i64,
        outcome: DeliveryOutcome,
    ) -> Result<DeliveryStep> {
        let mut conn = self.conn.lock();
        let attempts: i32 = match &mut *conn {
            AdminConnection::Local(conn) => conn.query_row(
                "SELECT attempts FROM alert_outbox WHERE id = ?1",
                [id],
                |row| row.get(0),
            )?,
            AdminConnection::Hosted(client) => client
                .query_one(
                    "SELECT attempts FROM sigiro_admin.alert_outbox WHERE id = $1",
                    &[&id],
                )?
                .get(0),
        };
        let step = delivery_step(attempts, outcome, now_us);
        let (next_attempt_at_us, dead_at_us) = match step {
            DeliveryStep::Retry { next_attempt_at_us } => (next_attempt_at_us, None),
            DeliveryStep::Dead => (now_us, Some(now_us)),
        };
        let error = outcome.error_class();
        let changed = match &mut *conn {
            AdminConnection::Local(conn) => conn.execute(
                "UPDATE alert_outbox SET attempts = attempts + 1, next_attempt_at_us = ?2, dead_at_us = ?3, last_error = ?4 WHERE id = ?1 AND sent_at_us IS NULL AND dead_at_us IS NULL",
                params![id, next_attempt_at_us, dead_at_us, error],
            )? as u64,
            AdminConnection::Hosted(client) => client.execute(
                "UPDATE sigiro_admin.alert_outbox SET attempts = attempts + 1, next_attempt_at_us = $2, dead_at_us = $3, last_error = $4 WHERE id = $1 AND sent_at_us IS NULL AND dead_at_us IS NULL",
                &[&id, &next_attempt_at_us, &dead_at_us, &error],
            )?,
        };
        ensure_was_queued(&mut conn, id, changed)?;
        Ok(step)
    }

    /// Put every `Dead` row of a project back in the queue, due now, with a
    /// fresh attempt budget. Returns how many rows were replayed.
    pub(crate) fn replay_dead(&self, project_id: &str, now_us: i64) -> Result<u64> {
        Ok(match &mut *self.conn.lock() {
            AdminConnection::Local(conn) => conn.execute(
                "UPDATE alert_outbox SET dead_at_us = NULL, attempts = 0, next_attempt_at_us = ?2 WHERE project_id = ?1 AND dead_at_us IS NOT NULL AND sent_at_us IS NULL",
                params![project_id, now_us],
            )? as u64,
            AdminConnection::Hosted(client) => client.execute(
                "UPDATE sigiro_admin.alert_outbox SET dead_at_us = NULL, attempts = 0, next_attempt_at_us = $2 WHERE project_id = $1 AND dead_at_us IS NOT NULL AND sent_at_us IS NULL",
                &[&project_id, &now_us],
            )?,
        })
    }

    /// Queue depth and dead count, for one project or (None) every project.
    pub(crate) fn delivery_summary(&self, project_id: Option<&str>) -> Result<DeliverySummary> {
        let (queued, dead, oldest, last_error): (i64, i64, Option<i64>, Option<String>) =
            match &mut *self.conn.lock() {
                AdminConnection::Local(conn) => conn.query_row(
                    "SELECT
                       count(*) FILTER (WHERE sent_at_us IS NULL AND dead_at_us IS NULL),
                       count(*) FILTER (WHERE sent_at_us IS NULL AND dead_at_us IS NOT NULL),
                       min(enqueued_at_us) FILTER (WHERE sent_at_us IS NULL AND dead_at_us IS NULL),
                       (SELECT last_error FROM alert_outbox d WHERE d.dead_at_us IS NOT NULL AND d.sent_at_us IS NULL
                          AND (?1 IS NULL OR d.project_id = ?1) ORDER BY d.dead_at_us DESC LIMIT 1)
                     FROM alert_outbox WHERE ?1 IS NULL OR project_id = ?1",
                    [project_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?,
                AdminConnection::Hosted(client) => {
                    let row = client.query_one(
                        "SELECT
                           count(*) FILTER (WHERE sent_at_us IS NULL AND dead_at_us IS NULL),
                           count(*) FILTER (WHERE sent_at_us IS NULL AND dead_at_us IS NOT NULL),
                           min(enqueued_at_us) FILTER (WHERE sent_at_us IS NULL AND dead_at_us IS NULL),
                           (SELECT last_error FROM sigiro_admin.alert_outbox d WHERE d.dead_at_us IS NOT NULL AND d.sent_at_us IS NULL
                              AND ($1::TEXT IS NULL OR d.project_id = $1) ORDER BY d.dead_at_us DESC LIMIT 1)
                         FROM sigiro_admin.alert_outbox WHERE $1::TEXT IS NULL OR project_id = $1",
                        &[&project_id],
                    )?;
                    (row.get(0), row.get(1), row.get(2), row.get(3))
                }
            };
        Ok(DeliverySummary {
            queued: u64::try_from(queued).unwrap_or(0),
            dead: u64::try_from(dead).unwrap_or(0),
            oldest_queued_at: oldest.map(format_timestamp).transpose()?,
            last_dead_error: last_error,
        })
    }

    /// The last summary the delivery loop computed across every project;
    /// `None` when webhook delivery is not configured.
    pub fn delivery_health(&self) -> Option<DeliverySummary> {
        self.delivery_health.lock().clone()
    }
}

/// Lifecycle of one webhook outbox row. It is derived from `sent_at_us` and
/// `dead_at_us` in one place ([`OutboxState::of`]); a separate state column
/// could not be backfilled for rows sent before it existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutboxState {
    /// Waiting for `next_attempt_at_us`; retried with backoff.
    Queued,
    /// The receiver answered 2xx. Terminal.
    Sent,
    /// Rejected permanently or out of attempts. Terminal until replayed.
    Dead,
}

impl OutboxState {
    pub(crate) fn of(sent_at_us: Option<i64>, dead_at_us: Option<i64>) -> Self {
        match (sent_at_us, dead_at_us) {
            (Some(_), _) => Self::Sent,
            (None, Some(_)) => Self::Dead,
            (None, None) => Self::Queued,
        }
    }
}

/// Attempts after which a still-failing notification is `Dead`. With the
/// 5s·2ⁿ backoff capped at an hour this is about eleven hours of retrying.
pub(crate) const MAX_DELIVERY_ATTEMPTS: i32 = 20;

/// What one delivery attempt produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeliveryOutcome {
    /// Non-2xx HTTP status.
    Http(u16),
    Timeout,
    Connect,
    Request,
}

impl DeliveryOutcome {
    fn error_class(self) -> String {
        match self {
            Self::Http(status) => format!("http_{status}"),
            Self::Timeout => "timeout".into(),
            Self::Connect => "connect".into(),
            Self::Request => "request".into(),
        }
    }

    /// A 4xx other than 408/429 will not change on retry: bad URL, token, or
    /// payload. Everything else (5xx, 429, 408, timeouts, connect errors) may.
    fn is_permanent(self) -> bool {
        matches!(self, Self::Http(status) if (400..500).contains(&status) && status != 408 && status != 429)
    }
}

/// Where a failed attempt sends the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeliveryStep {
    Retry { next_attempt_at_us: i64 },
    Dead,
}

/// The failure edge of the outbox machine. `attempts` counts earlier failures.
pub(crate) fn delivery_step(attempts: i32, outcome: DeliveryOutcome, now_us: i64) -> DeliveryStep {
    if outcome.is_permanent() || attempts.saturating_add(1) >= MAX_DELIVERY_ATTEMPTS {
        return DeliveryStep::Dead;
    }
    let exponent = attempts.clamp(0, 10) as u32;
    let delay_secs = 5_i64.saturating_mul(1_i64 << exponent).min(3_600);
    DeliveryStep::Retry {
        next_attempt_at_us: now_us + delay_secs * 1_000_000,
    }
}

/// Outbox counts shown on `/health` and `GET /v1/alerts`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeliverySummary {
    /// Notifications waiting to be (re)sent.
    pub queued: u64,
    /// Notifications that failed permanently; replay with `POST /v1/alerts/replay`.
    pub dead: u64,
    /// When the oldest queued notification was enqueued (RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_queued_at: Option<String>,
    /// Error class of the most recent dead notification, e.g. `http_401`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_dead_error: Option<String>,
}

fn outbox_state(conn: &mut AdminConnection, id: i64) -> Result<Option<OutboxState>> {
    let columns: Option<(Option<i64>, Option<i64>)> = match conn {
        AdminConnection::Local(conn) => conn
            .query_row(
                "SELECT sent_at_us, dead_at_us FROM alert_outbox WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?,
        AdminConnection::Hosted(client) => client
            .query_opt(
                "SELECT sent_at_us, dead_at_us FROM sigiro_admin.alert_outbox WHERE id = $1",
                &[&id],
            )?
            .map(|row| (row.get(0), row.get(1))),
    };
    Ok(columns.map(|(sent, dead)| OutboxState::of(sent, dead)))
}

/// Sent and Dead are terminal: a transition out of them is a caller bug.
fn ensure_was_queued(conn: &mut AdminConnection, id: i64, changed: u64) -> Result<()> {
    if changed == 1 {
        return Ok(());
    }
    match outbox_state(conn, id)? {
        Some(state) => anyhow::bail!("alert outbox row {id} is {state:?}, not Queued"),
        None => anyhow::bail!("alert outbox row {id} does not exist"),
    }
}

fn process_events_sqlite(
    conn: &mut rusqlite::Connection,
    events: &[AlertEvent],
) -> Result<ProcessResult> {
    let mut tx = conn
        .transaction()
        .context("failed to start alert transaction")?;
    let result = apply_events(&mut tx, events)?;
    tx.commit().context("failed to commit alert transaction")?;
    Ok(result)
}

fn process_events_postgres(
    client: &mut postgres::Client,
    events: &[AlertEvent],
) -> Result<ProcessResult> {
    let mut tx = client
        .transaction()
        .context("failed to start alert transaction")?;
    let result = apply_events(&mut tx, events)?;
    tx.commit().context("failed to commit alert transaction")?;
    Ok(result)
}

fn alert_instance(row: AlertRow) -> Result<AlertInstance> {
    Ok(AlertInstance {
        fingerprint: row.fingerprint,
        rule_id: row.rule_id,
        status: row.status,
        service_name: row.service_name,
        container_name: row.container_name,
        starts_at: format_timestamp(row.starts_at_us)?,
        ends_at: row.ends_at_us.map(format_timestamp).transpose()?,
        occurrence_count: row
            .occurrence_count
            .try_into()
            .context("negative alert occurrence count")?,
    })
}

fn list_sqlite(
    conn: &rusqlite::Connection,
    project_id: &str,
    status: Option<&str>,
) -> Result<Vec<AlertInstance>> {
    let mut sql = String::from(
        "SELECT fingerprint, rule_id, status, service_name, container_name, starts_at_us, ends_at_us, occurrence_count FROM alert_instances WHERE project_id = ?1 AND status IN ('firing', 'resolved')",
    );
    if status.is_some() {
        sql.push_str(" AND status = ?2");
    }
    sql.push_str(" ORDER BY updated_at_us DESC");
    let mut stmt = conn.prepare(&sql)?;
    let map = |row: &rusqlite::Row<'_>| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
        ))
    };
    let rows = match status {
        Some(status) => stmt.query_map(params![project_id, status], map)?,
        None => stmt.query_map(params![project_id], map)?,
    };
    rows.collect::<rusqlite::Result<
        Vec<(
            String,
            String,
            String,
            String,
            String,
            i64,
            Option<i64>,
            i64,
        )>,
    >>()?
    .into_iter()
    .map(|v| {
        alert_instance(AlertRow {
            fingerprint: v.0,
            rule_id: v.1,
            status: v.2,
            service_name: v.3,
            container_name: v.4,
            starts_at_us: v.5,
            ends_at_us: v.6,
            occurrence_count: v.7,
        })
    })
    .collect()
}

fn list_postgres(
    client: &mut postgres::Client,
    project_id: &str,
    status: Option<&str>,
) -> Result<Vec<AlertInstance>> {
    let rows = match status {
        Some(status) => client.query("SELECT fingerprint, rule_id, status, service_name, container_name, starts_at_us, ends_at_us, occurrence_count FROM sigiro_admin.alert_instances WHERE project_id = $1 AND status = $2 ORDER BY updated_at_us DESC", &[&project_id, &status])?,
        None => client.query("SELECT fingerprint, rule_id, status, service_name, container_name, starts_at_us, ends_at_us, occurrence_count FROM sigiro_admin.alert_instances WHERE project_id = $1 AND status IN ('firing', 'resolved') ORDER BY updated_at_us DESC", &[&project_id])?,
    };
    rows.into_iter()
        .map(|row| {
            alert_instance(AlertRow {
                fingerprint: row.get(0),
                rule_id: row.get(1),
                status: row.get(2),
                service_name: row.get(3),
                container_name: row.get(4),
                starts_at_us: row.get(5),
                ends_at_us: row.get(6),
                occurrence_count: row.get(7),
            })
        })
        .collect()
}

/// Deliver persisted notifications. A crash after HTTP success and before
/// `mark_sent` can repeat a request, so receivers must deduplicate by fingerprint.
pub async fn run_delivery(store: AlertStore, config: DeliveryConfig) {
    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            tracing::error!("failed to build alert webhook client: {error}");
            return;
        }
    };
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut ticks: u64 = 0;
    loop {
        interval.tick().await;
        let mut attempted = 0;
        loop {
            // Every AlertStore call below reaches the sync postgres client,
            // whose internal block_on ABORTS the process from a runtime thread.
            // This loop is async, so each call moves to a blocking thread.
            let due = blocking(&store, |store| store.next_due(now_us())).await;
            let notification = match due {
                Ok(Some(notification)) => notification,
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!("failed to read alert outbox: {error:#}");
                    break;
                }
            };
            attempted += 1;
            let id = notification.id;
            let mut request = client
                .post(config.url.clone())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(notification.payload);
            if let Some(token) = &config.token {
                request = request.bearer_auth(token.as_ref());
            }
            let outcome = match request.send().await {
                Ok(response) if response.status().is_success() => {
                    if let Err(error) =
                        blocking(&store, move |store| store.mark_sent(id, now_us())).await
                    {
                        tracing::warn!(
                            notification_id = id,
                            "webhook accepted alert but sent state was not persisted: {error:#}"
                        );
                        break;
                    }
                    continue;
                }
                Ok(response) => DeliveryOutcome::Http(response.status().as_u16()),
                Err(error) if error.is_timeout() => DeliveryOutcome::Timeout,
                Err(error) if error.is_connect() => DeliveryOutcome::Connect,
                Err(_) => DeliveryOutcome::Request,
            };
            match blocking(&store, move |store| {
                store.mark_failed(id, now_us(), outcome)
            })
            .await
            {
                Ok(DeliveryStep::Dead) => tracing::warn!(
                    notification_id = id,
                    error = %outcome.error_class(),
                    "alert webhook is dead: permanent rejection or out of attempts; replay with POST /v1/alerts/replay"
                ),
                Ok(DeliveryStep::Retry { .. }) => {}
                Err(error) => {
                    tracing::warn!(
                        notification_id = id,
                        "failed to record alert delivery failure: {error:#}"
                    );
                    break;
                }
            }
        }
        // Refresh the /health summary when the queue moved, and every 30s so
        // newly enqueued or replayed rows show up without an attempt.
        if attempted > 0 || ticks.is_multiple_of(30) {
            match blocking(&store, |store| store.delivery_summary(None)).await {
                Ok(summary) => *store.delivery_health.lock() = Some(summary),
                Err(error) => tracing::warn!("failed to summarise alert outbox: {error:#}"),
            }
        }
        ticks = ticks.wrapping_add(1);
    }
}

/// Run a sync [`AlertStore`] call on a blocking thread.
async fn blocking<T: Send + 'static>(
    store: &AlertStore,
    call: impl FnOnce(&AlertStore) -> Result<T> + Send + 'static,
) -> Result<T> {
    let store = store.clone();
    tokio::task::spawn_blocking(move || call(&store))
        .await
        .map_err(|error| anyhow::anyhow!("alert outbox task panicked: {error}"))?
}

pub(crate) const SQLITE_TABLES: &[DeclaredTable] = &[
    DeclaredTable {
        name: "alert_instances",
        create: "CREATE TABLE {table} (
            project_id TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            rule_id TEXT NOT NULL,
            status TEXT NOT NULL,
            service_name TEXT NOT NULL,
            container_name TEXT NOT NULL,
            starts_at_us INTEGER NOT NULL,
            ends_at_us INTEGER,
            occurrence_count INTEGER NOT NULL,
            updated_at_us INTEGER NOT NULL,
            recovered_ahead_us INTEGER,
            PRIMARY KEY (project_id, fingerprint))",
        indexes: &[
            "CREATE INDEX alert_instances_project_status ON {table} (project_id, status, updated_at_us)",
        ],
    },
    DeclaredTable {
        name: "alert_occurrences",
        create: "CREATE TABLE {table} (
            project_id TEXT NOT NULL,
            occurrence_key TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            observed_at_us INTEGER NOT NULL,
            PRIMARY KEY (project_id, occurrence_key))",
        indexes: &[],
    },
    DeclaredTable {
        name: "alert_outbox",
        create: "CREATE TABLE {table} (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            transition TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0,
            next_attempt_at_us INTEGER NOT NULL,
            sent_at_us INTEGER,
            last_error TEXT,
            dead_at_us INTEGER,
            enqueued_at_us INTEGER)",
        indexes: &[
            "CREATE INDEX alert_outbox_due ON {table} (sent_at_us, dead_at_us, next_attempt_at_us, id)",
        ],
    },
];

pub(crate) const POSTGRES_TABLES: &[DeclaredTable] = &[
    DeclaredTable {
        name: "alert_instances",
        create: "CREATE TABLE {table} (
            project_id TEXT NOT NULL, fingerprint TEXT NOT NULL, rule_id TEXT NOT NULL,
            status TEXT NOT NULL, service_name TEXT NOT NULL, container_name TEXT NOT NULL,
            starts_at_us BIGINT NOT NULL, ends_at_us BIGINT, occurrence_count BIGINT NOT NULL,
            updated_at_us BIGINT NOT NULL, recovered_ahead_us BIGINT, PRIMARY KEY (project_id, fingerprint))",
        indexes: &["CREATE INDEX alert_instances_project_status ON {table} (project_id, status, updated_at_us)"],
    },
    DeclaredTable {
        name: "alert_occurrences",
        create: "CREATE TABLE {table} (
            project_id TEXT NOT NULL, occurrence_key TEXT NOT NULL, fingerprint TEXT NOT NULL,
            observed_at_us BIGINT NOT NULL, PRIMARY KEY (project_id, occurrence_key))",
        indexes: &[],
    },
    DeclaredTable {
        name: "alert_outbox",
        create: "CREATE TABLE {table} (
            id BIGSERIAL PRIMARY KEY, project_id TEXT NOT NULL, fingerprint TEXT NOT NULL,
            transition TEXT NOT NULL, payload_json TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0,
            next_attempt_at_us BIGINT NOT NULL, sent_at_us BIGINT, last_error TEXT,
            dead_at_us BIGINT, enqueued_at_us BIGINT)",
        indexes: &["CREATE INDEX alert_outbox_due ON {table} (sent_at_us, dead_at_us, next_attempt_at_us, id)"],
    },
];

pub(crate) fn events_from_log_batch(
    project_id: &str,
    batch: &RecordBatch,
) -> Result<Vec<AlertEvent>> {
    let schema = batch.schema();
    let event_names = string_column(batch, schema.index_of("event_name")?)?;
    if !(0..batch.num_rows()).any(|row| {
        let name = event_names.value(row);
        name == OOM_FIRING_EVENT || name == OOM_RECOVERED_EVENT
    }) {
        return Ok(Vec::new());
    }
    let timestamps = batch
        .column(schema.index_of("timestamp")?)
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .context("timestamp column is not TimestampMicrosecondArray")?;
    let services = string_column(batch, schema.index_of("service_name")?)?;
    let resource_attrs = string_column(batch, schema.index_of("resource_attributes")?)?;
    let log_attrs = string_column(batch, schema.index_of("log_attributes")?)?;
    let mut events = Vec::new();

    for row in 0..batch.num_rows() {
        let event_name = event_names.value(row);
        if event_name != OOM_FIRING_EVENT && event_name != OOM_RECOVERED_EVENT {
            continue;
        }
        let resource = parse_attributes(resource_attrs, row)?;
        let attributes = parse_attributes(log_attrs, row)?;
        let service_name = resource
            .get("service.name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| services.value(row));
        anyhow::ensure!(
            service_name.len() <= 128,
            "service.name exceeds the alert label limit"
        );
        let container_name = required_string(&attributes, "container.name", 128)?;
        let container_id = required_string(&attributes, "container.id", 128)?;
        events.push(AlertEvent {
            project_id: project_id.to_string(),
            event_name: event_name.to_string(),
            timestamp_us: timestamps.value(row),
            service_name: service_name.to_string(),
            container_name: container_name.to_string(),
            container_id: container_id.to_string(),
        });
    }
    Ok(events)
}

fn webhook_payload(event: &AlertEvent, fingerprint: &str, state: AlertState) -> Result<String> {
    let status = state.status();
    let (starts_at_us, ends_at_us) = state.bounds();
    let starts_at = format_timestamp(starts_at_us)?;
    let ends_at = ends_at_us
        .map(format_timestamp)
        .transpose()?
        .unwrap_or_else(|| "0001-01-01T00:00:00Z".to_string());
    serde_json::to_string(&serde_json::json!({
        "receiver": "sigiro",
        "status": status,
        "alerts": [{
            "status": status,
            "labels": {
                "alertname": "ContainerOOMKilled",
                "project": event.project_id,
                "service": event.service_name,
                "container": event.container_name,
                "severity": "critical",
            },
            "annotations": {
                "summary": format!("{} was killed because it ran out of memory", event.container_name),
                "occurrence_count": state.occurrence_count().to_string(),
            },
            "startsAt": starts_at,
            "endsAt": ends_at,
            "generatorURL": "",
            "fingerprint": fingerprint,
        }],
        "groupLabels": {"alertname": "ContainerOOMKilled"},
        "commonLabels": {
            "alertname": "ContainerOOMKilled",
            "project": event.project_id,
            "service": event.service_name,
            "container": event.container_name,
            "severity": "critical",
        },
        "commonAnnotations": {},
        "externalURL": "",
        "version": "4",
        "groupKey": format!("{{}}:{{alertname=\"ContainerOOMKilled\",fingerprint=\"{}\"}}", fingerprint),
        "truncatedAlerts": 0,
    }))
    .context("failed to serialize Alertmanager webhook")
}

fn alert_fingerprint(event: &AlertEvent) -> String {
    hash_hex(&[
        "sigiro/alert/v1",
        &event.project_id,
        OOM_RULE_ID,
        &event.service_name,
        &event.container_name,
    ])[..16]
        .to_string()
}

fn occurrence_key(event: &AlertEvent) -> String {
    hash_hex(&[
        "sigiro/alert-occurrence/v1",
        &event.project_id,
        &event.event_name,
        &event.timestamp_us.to_string(),
        &event.container_id,
    ])
}

fn hash_hex(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hex::encode(hasher.finalize())
}

fn format_timestamp(timestamp_us: i64) -> Result<String> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(timestamp_us) * 1_000)
        .context("alert timestamp is out of range")?
        .format(&Rfc3339)
        .context("failed to format alert timestamp")
}

pub(crate) fn now_us() -> i64 {
    (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000) as i64
}

fn string_column(batch: &RecordBatch, index: usize) -> Result<&StringArray> {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<StringArray>()
        .context("alert input column is not StringArray")
}

fn parse_attributes(
    column: &StringArray,
    row: usize,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    if column.is_null(row) {
        return Ok(serde_json::Map::new());
    }
    serde_json::from_str(column.value(row)).context("alert attributes are not a JSON object")
}

fn required_string<'a>(
    attributes: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
    max_len: usize,
) -> Result<&'a str> {
    let value = attributes
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("{key} is required for container OOM alerts"))?;
    anyhow::ensure!(
        value.len() <= max_len,
        "{key} exceeds the alert label limit"
    );
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use AlertInput::{Fire, Recover};
    use AlertState::{Firing, RecoveredFirst, Resolved};

    const fn firing(starts_at_us: i64, occurrence_count: u64) -> AlertState {
        Firing {
            starts_at_us,
            occurrence_count,
        }
    }
    const fn resolved(starts_at_us: i64, ends_at_us: i64, occurrence_count: u64) -> AlertState {
        Resolved {
            starts_at_us,
            ends_at_us,
            occurrence_count,
            recovered_ahead_us: None,
        }
    }
    const fn ahead(state: AlertState, at_us: i64) -> AlertState {
        match state {
            Resolved {
                starts_at_us,
                ends_at_us,
                occurrence_count,
                ..
            } => Resolved {
                starts_at_us,
                ends_at_us,
                occurrence_count,
                recovered_ahead_us: Some(at_us),
            },
            other => other,
        }
    }
    const fn recovered_first(recovered_at_us: i64, occurrence_count: u64) -> AlertState {
        RecoveredFirst {
            recovered_at_us,
            occurrence_count,
        }
    }
    const fn to(next: AlertState, notify: bool) -> AlertStep {
        AlertStep::Move { next, notify }
    }

    #[test]
    fn every_alert_transition_follows_the_table() {
        let cases: &[(Option<AlertState>, AlertInput, AlertStep)] = &[
            (None, Fire { at_us: 10 }, to(firing(10, 1), true)),
            (
                None,
                Recover { at_us: 20 },
                to(recovered_first(20, 0), false),
            ),
            (
                Some(recovered_first(20, 0)),
                Fire { at_us: 10 },
                to(resolved(10, 20, 1), false),
            ),
            (
                Some(recovered_first(20, 0)),
                Fire { at_us: 20 },
                to(resolved(20, 20, 1), false),
            ),
            (
                Some(recovered_first(20, 3)),
                Fire { at_us: 30 },
                to(firing(30, 4), true),
            ),
            (
                Some(recovered_first(20, 0)),
                Recover { at_us: 40 },
                to(recovered_first(40, 0), false),
            ),
            (
                Some(recovered_first(40, 0)),
                Recover { at_us: 20 },
                to(recovered_first(40, 0), false),
            ),
            (
                Some(firing(10, 1)),
                Fire { at_us: 15 },
                to(firing(10, 2), false),
            ),
            (
                Some(firing(10, 1)),
                Fire { at_us: 5 },
                to(firing(10, 2), false),
            ),
            (
                Some(firing(10, 2)),
                Recover { at_us: 20 },
                to(resolved(10, 20, 2), true),
            ),
            (
                Some(firing(10, 2)),
                Recover { at_us: 10 },
                to(resolved(10, 10, 2), true),
            ),
            (Some(firing(10, 1)), Recover { at_us: 5 }, AlertStep::Ignore),
            (
                Some(resolved(10, 20, 1)),
                Fire { at_us: 30 },
                to(firing(30, 2), true),
            ),
            (
                Some(resolved(10, 20, 1)),
                Fire { at_us: 20 },
                AlertStep::Ignore,
            ),
            (
                Some(resolved(10, 20, 1)),
                Fire { at_us: 15 },
                AlertStep::Ignore,
            ),
            (
                Some(resolved(10, 20, 1)),
                Recover { at_us: 20 },
                AlertStep::Ignore,
            ),
            (
                Some(resolved(10, 20, 1)),
                Recover { at_us: 5 },
                AlertStep::Ignore,
            ),
            (
                Some(resolved(10, 20, 1)),
                Recover { at_us: 40 },
                to(ahead(resolved(10, 20, 1), 40), false),
            ),
            (
                Some(ahead(resolved(10, 20, 1), 40)),
                Recover { at_us: 30 },
                to(ahead(resolved(10, 20, 1), 40), false),
            ),
            (
                Some(ahead(resolved(10, 20, 1), 40)),
                Recover { at_us: 50 },
                to(ahead(resolved(10, 20, 1), 50), false),
            ),
            (
                Some(ahead(resolved(10, 20, 1), 40)),
                Fire { at_us: 30 },
                to(resolved(30, 40, 2), false),
            ),
            (
                Some(ahead(resolved(10, 20, 1), 40)),
                Fire { at_us: 40 },
                to(resolved(40, 40, 2), false),
            ),
            (
                Some(ahead(resolved(10, 20, 1), 40)),
                Fire { at_us: 50 },
                to(firing(50, 2), true),
            ),
            (
                Some(ahead(resolved(10, 20, 1), 40)),
                Fire { at_us: 15 },
                AlertStep::Ignore,
            ),
        ];
        for (from, input, expected) in cases {
            assert_eq!(step(*from, *input), *expected, "{from:?} + {input:?}");
        }
    }

    fn run(inputs: &[AlertInput]) -> Option<AlertState> {
        inputs
            .iter()
            .fold(None, |state, input| match step(state, *input) {
                AlertStep::Ignore => state,
                AlertStep::Move { next, .. } => Some(next),
            })
    }

    fn permutations(items: &[AlertInput]) -> Vec<Vec<AlertInput>> {
        if items.len() <= 1 {
            return vec![items.to_vec()];
        }
        let mut all = Vec::new();
        for i in 0..items.len() {
            let mut rest = items.to_vec();
            let head = rest.remove(i);
            for mut tail in permutations(&rest) {
                tail.insert(0, head);
                all.push(tail);
            }
        }
        all
    }

    #[test]
    fn no_arrival_order_leaves_a_recovered_alert_firing() {
        let one = [Fire { at_us: 10 }, Recover { at_us: 20 }];
        let two = [
            Fire { at_us: 10 },
            Recover { at_us: 20 },
            Fire { at_us: 30 },
            Recover { at_us: 40 },
        ];
        for order in permutations(&one) {
            assert_eq!(run(&order), Some(resolved(10, 20, 1)), "{order:?}");
        }
        for order in permutations(&two) {
            match run(&order) {
                Some(
                    Resolved { ends_at_us: 40, .. }
                    | Resolved {
                        recovered_ahead_us: Some(40),
                        ..
                    },
                ) => {}
                other => panic!("{order:?} ended in {other:?}"),
            }
        }
    }

    #[test]
    fn stored_rows_decode_strictly() {
        for state in [
            firing(10, 1),
            resolved(10, 20, 2),
            ahead(resolved(10, 20, 2), 40),
            recovered_first(30, 2),
        ] {
            let (starts, ends) = state.bounds();
            let count = i64::try_from(state.occurrence_count()).unwrap();
            let decoded = AlertState::decode(
                state.status(),
                starts,
                ends,
                count,
                state.recovered_ahead_us(),
            );
            assert_eq!(decoded.unwrap(), state);
        }
        assert!(AlertState::decode("pending", 10, None, 1, None).is_err());
        assert!(AlertState::decode("firing", 10, Some(20), 1, None).is_err());
        assert!(AlertState::decode("resolved", 10, None, 1, None).is_err());
        assert!(AlertState::decode("firing", 10, None, -1, None).is_err());
    }

    pub(super) fn local_store() -> (tempfile::TempDir, AlertStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let conn = crate::admin::open_local_admin_db(&path).unwrap();
        (dir, AlertStore::new(Arc::new(Mutex::new(conn))).unwrap())
    }

    /// A store on the PostgreSQL named by `SIGIRO_TEST_POSTGRES_URL`
    /// (`scripts/test-admin-postgres.sh` starts one), with a fresh admin schema.
    pub(super) fn postgres_store() -> Option<AlertStore> {
        #[expect(
            clippy::disallowed_methods,
            reason = "selects an external test database, not application configuration"
        )]
        let url = std::env::var("SIGIRO_TEST_POSTGRES_URL").ok()?;
        crate::admin::connect_postgres(&url)
            .unwrap()
            .batch_execute("DROP SCHEMA IF EXISTS sigiro_admin CASCADE")
            .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let conn = runtime
            .block_on(crate::admin::open_hosted_admin_db(&url))
            .unwrap();
        drop(runtime);
        Some(AlertStore::new(Arc::new(Mutex::new(conn))).unwrap())
    }

    pub(super) fn oom(project: &str, name: &str, at_us: i64) -> AlertEvent {
        AlertEvent {
            project_id: project.into(),
            event_name: name.into(),
            timestamp_us: at_us,
            service_name: "api".into(),
            container_name: "api-1".into(),
            container_id: "c1".into(),
        }
    }

    fn recover_before_fire(store: &AlertStore, project: &str) {
        let first = store
            .process_events(&[oom(project, OOM_RECOVERED_EVENT, 2_000_000)])
            .unwrap();
        assert_eq!(
            (first.accepted_events, first.notifications_enqueued),
            (1, 0)
        );
        assert!(
            store.list(project, None).unwrap().is_empty(),
            "a recover with no fire is not an alert"
        );
        let second = store
            .process_events(&[oom(project, OOM_FIRING_EVENT, 1_000_000)])
            .unwrap();
        assert_eq!(
            (second.accepted_events, second.notifications_enqueued),
            (1, 0)
        );
        let alerts = store.list(project, None).unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].status, "resolved");
        assert_eq!(alerts[0].occurrence_count, 1);
        assert!(store.list(project, Some("firing")).unwrap().is_empty());
        let replay = store
            .process_events(&[oom(project, OOM_FIRING_EVENT, 1_000_000)])
            .unwrap();
        assert_eq!(replay.accepted_events, 0, "duplicates stay deduplicated");
    }

    fn recover_ahead_of_reopen(store: &AlertStore, project: &str) {
        store
            .process_events(&[
                oom(project, OOM_FIRING_EVENT, 1_000_000),
                oom(project, OOM_RECOVERED_EVENT, 2_000_000),
            ])
            .unwrap();
        store
            .process_events(&[oom(project, OOM_RECOVERED_EVENT, 4_000_000)])
            .unwrap();
        let resolved = store.list(project, Some("resolved")).unwrap();
        assert_eq!(resolved.len(), 1, "the earlier episode stays listed");
        let late = store
            .process_events(&[oom(project, OOM_FIRING_EVENT, 3_000_000)])
            .unwrap();
        assert_eq!(late.notifications_enqueued, 0);
        assert!(store.list(project, Some("firing")).unwrap().is_empty());
        assert_eq!(store.list(project, None).unwrap()[0].occurrence_count, 2);
    }

    fn in_order_pages_and_reopens(store: &AlertStore, project: &str) {
        let result = store
            .process_events(&[
                oom(project, OOM_FIRING_EVENT, 1_000_000),
                oom(project, OOM_FIRING_EVENT, 1_500_000),
                oom(project, OOM_RECOVERED_EVENT, 2_000_000),
                oom(project, OOM_FIRING_EVENT, 3_000_000),
            ])
            .unwrap();
        assert_eq!(
            (result.accepted_events, result.notifications_enqueued),
            (4, 3)
        );
        let alerts = store.list(project, Some("firing")).unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].occurrence_count, 3);
    }

    #[test]
    fn every_delivery_failure_follows_the_table() {
        use DeliveryOutcome::{Connect, Http, Request, Timeout};
        let retry = |secs: i64| DeliveryStep::Retry {
            next_attempt_at_us: 1_000_000 + secs * 1_000_000,
        };
        let cases: &[(i32, DeliveryOutcome, DeliveryStep)] = &[
            (0, Http(500), retry(5)),
            (1, Http(503), retry(10)),
            (0, Http(429), retry(5)),
            (0, Http(408), retry(5)),
            (0, Http(302), retry(5)),
            (3, Timeout, retry(40)),
            (0, Connect, retry(5)),
            (0, Request, retry(5)),
            (10, Http(502), retry(3_600)),
            (18, Http(502), retry(3_600)),
            (19, Http(502), DeliveryStep::Dead),
            (40, Timeout, DeliveryStep::Dead),
            (0, Http(400), DeliveryStep::Dead),
            (0, Http(401), DeliveryStep::Dead),
            (0, Http(403), DeliveryStep::Dead),
            (0, Http(404), DeliveryStep::Dead),
            (0, Http(410), DeliveryStep::Dead),
        ];
        for (attempts, outcome, expected) in cases {
            assert_eq!(
                delivery_step(*attempts, *outcome, 1_000_000),
                *expected,
                "{attempts} earlier failures, then {outcome:?}"
            );
        }
    }

    #[test]
    fn outbox_state_is_read_from_its_columns() {
        assert_eq!(OutboxState::of(None, None), OutboxState::Queued);
        assert_eq!(OutboxState::of(Some(1), None), OutboxState::Sent);
        assert_eq!(OutboxState::of(None, Some(1)), OutboxState::Dead);
        assert_eq!(OutboxState::of(Some(1), Some(2)), OutboxState::Sent);
    }

    fn outbox_lifecycle(store: &AlertStore, project: &str) {
        store
            .process_events(&[oom(project, OOM_FIRING_EVENT, 1_000_000)])
            .unwrap();
        let summary = store.delivery_summary(Some(project)).unwrap();
        assert_eq!((summary.queued, summary.dead), (1, 0));
        assert!(summary.oldest_queued_at.is_some());

        // Queued -> Dead at once on a permanent rejection.
        let due = store.next_due(i64::MAX).unwrap().unwrap();
        let step = store
            .mark_failed(due.id, 1, DeliveryOutcome::Http(401))
            .unwrap();
        assert_eq!(step, DeliveryStep::Dead);
        let summary = store.delivery_summary(Some(project)).unwrap();
        assert_eq!((summary.queued, summary.dead), (0, 1));
        assert_eq!(summary.last_dead_error.as_deref(), Some("http_401"));
        assert!(
            store.next_due(i64::MAX).unwrap().is_none(),
            "dead rows are not retried"
        );
        let late = store.mark_sent(due.id, 2).unwrap_err().to_string();
        assert!(late.contains("Dead"), "{late}");
        assert!(
            store
                .mark_failed(due.id, 2, DeliveryOutcome::Timeout)
                .is_err()
        );

        // Dead -> Queued on replay, with a fresh attempt budget.
        assert_eq!(store.replay_dead(project, 3).unwrap(), 1);
        assert_eq!(
            store.replay_dead(project, 3).unwrap(),
            0,
            "replay is idempotent"
        );
        assert_eq!(store.next_due(3).unwrap().map(|due| due.id), Some(due.id));

        // Queued -> Queued (backoff) until the attempt budget runs out.
        for attempt in 0..MAX_DELIVERY_ATTEMPTS - 1 {
            let step = store
                .mark_failed(due.id, 10, DeliveryOutcome::Http(503))
                .unwrap();
            assert!(
                matches!(step, DeliveryStep::Retry { .. }),
                "attempt {attempt}: {step:?}"
            );
            assert!(
                store.next_due(10).unwrap().is_none(),
                "backoff defers the retry"
            );
        }
        let last = store
            .mark_failed(due.id, 10, DeliveryOutcome::Http(503))
            .unwrap();
        assert_eq!(last, DeliveryStep::Dead);

        // Dead -> Queued -> Sent; Sent is terminal.
        assert_eq!(store.replay_dead(project, 20).unwrap(), 1);
        assert_eq!(store.next_due(20).unwrap().map(|due| due.id), Some(due.id));
        store.mark_sent(due.id, 21).unwrap();
        assert!(store.next_due(i64::MAX).unwrap().is_none());
        assert!(
            store
                .mark_sent(due.id, 21)
                .unwrap_err()
                .to_string()
                .contains("Sent")
        );
        assert_eq!(
            store.replay_dead(project, 22).unwrap(),
            0,
            "sent rows are never replayed"
        );
        let summary = store.delivery_summary(Some(project)).unwrap();
        assert_eq!((summary.queued, summary.dead), (0, 0));
    }

    type Scenario = fn(&AlertStore, &str);

    pub(super) const SCENARIOS: &[(&str, Scenario)] = &[
        ("recover_before_fire", recover_before_fire),
        ("recover_ahead_of_reopen", recover_ahead_of_reopen),
        ("in_order_pages_and_reopens", in_order_pages_and_reopens),
        ("outbox_lifecycle", outbox_lifecycle),
    ];

    #[test]
    fn sqlite_backend_runs_every_scenario() {
        for (name, scenario) in SCENARIOS {
            let (_dir, store) = local_store();
            scenario(&store, name);
        }
    }

    #[test]
    fn postgres_backend_runs_every_scenario() {
        for (name, scenario) in SCENARIOS {
            let Some(store) = postgres_store() else {
                eprintln!("skipped: set SIGIRO_TEST_POSTGRES_URL (scripts/test-admin-postgres.sh)");
                return;
            };
            scenario(&store, name);
        }
    }
}
