//! Trusted, tenant-scoped live telemetry summary; Assessor sees aggregates, never credentials or raw records.
use crate::{pool::ProjectPool, server::AppState, types::TenantId};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

#[derive(Debug, Clone)]
pub(crate) struct Decision {
    pub probability: f64,
    pub evidence: Value,
}

#[derive(Deserialize)]
struct AssessorAnswer {
    answers: AssessorQuestions,
}
#[derive(Deserialize)]
struct AssessorQuestions {
    investigate: AssessorNoul,
}
#[derive(Deserialize)]
struct AssessorNoul {
    #[serde(rename = "type")]
    answer_type: String,
    noul: f64,
}

const SCANS: [(&str, &str, &str); 3] = [
    ("spans", "sigiro_spans", "status_code=2"),
    ("logs", "sigiro_logs", "severity_number>=17"),
    ("gauges", "sigiro_metrics_gauge", "FALSE"),
];

fn summary_sql(
    prefix: &str,
    table: &str,
    service: &str,
    failure: &str,
    bounds: &[String; 4],
) -> String {
    let escaped_service = service.replace('\'', "''");
    let [start, recent, fresh, end] = bounds;
    let duration = if table == "sigiro_spans" {
        format!(
            ", avg(duration) FILTER (WHERE timestamp >= TIMESTAMP '{recent}') AS recent_duration_us, avg(duration) FILTER (WHERE timestamp < TIMESTAMP '{recent}') AS baseline_duration_us"
        )
    } else {
        String::new()
    };
    format!(
        "SELECT \
        count(*) FILTER (WHERE timestamp >= TIMESTAMP '{recent}') AS recent, \
        count(*) FILTER (WHERE timestamp < TIMESTAMP '{recent}') AS baseline, \
        count(*) FILTER (WHERE timestamp >= TIMESTAMP '{fresh}') AS fresh, \
        count(*) FILTER (WHERE timestamp >= TIMESTAMP '{recent}' AND {failure}) AS recent_failures, \
        count(*) FILTER (WHERE timestamp < TIMESTAMP '{recent}' AND {failure}) AS baseline_failures{duration} \
        FROM {prefix}{table} WHERE service_name='{escaped_service}' \
        AND timestamp >= TIMESTAMP '{start}' AND timestamp < TIMESTAMP '{end}' \
        AND (resource_attributes IS NULL OR resource_attributes NOT LIKE '%production-journald-backfill%')"
    )
}

pub(crate) async fn assess(
    state: &Arc<AppState>,
    tenant: &TenantId,
    service: &str,
) -> Result<Decision> {
    let key = std::env::var("SIGIRO_ASSESSOR_API_KEY").unwrap_or_default();
    if key.len() < 32 {
        bail!("trusted model assessment is not configured");
    }
    let prefix = ProjectPool::prefix();
    let now = time::OffsetDateTime::now_utc();
    let format = time::format_description::parse_borrowed::<1>(
        "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:6]",
    )?;
    let bound = |seconds_ago| (now - time::Duration::seconds(seconds_ago)).format(&format);
    let bounds = [bound(3600)?, bound(1800)?, bound(300)?, bound(0)?];
    // A zero preceding window is insufficient evidence for a change. Missing or
    // stale signals abstain instead of fabricating an incident from a log count.
    let mut evidence = serde_json::Map::new();
    let mut comparable = false;
    for (signal, table, failure) in SCANS {
        let sql = summary_sql(prefix, table, service, failure, &bounds);
        let (rows, truncated) = crate::server::execute_sql(state, tenant, &sql)
            .await
            .context("unable to read tenant-scoped live telemetry")?;
        if truncated || rows.len() != 1 {
            bail!("live telemetry summary unavailable");
        }
        let row = &rows[0];
        let count = |name: &str| {
            row.get(name)
                .and_then(Value::as_i64)
                .filter(|n| *n >= 0)
                .context("invalid live telemetry count")
        };
        let (previous, current, newest) = (count("baseline")?, count("recent")?, count("fresh")?);
        comparable |= previous > 0 && current > 0 && newest > 0;
        for field in ["recent_failures", "baseline_failures"] {
            count(field)?;
        }
        evidence.insert(signal.to_string(), row.clone());
    }
    if !comparable {
        bail!("no fresh production telemetry with a comparable baseline for the requested service");
    }
    let evidence = json!({"service": service, "recent_window_minutes": 30,
        "baseline_window_minutes": 30, "signals": evidence});
    let endpoint = std::env::var("SIGIRO_ASSESSOR_API_URL")
        .unwrap_or_else(|_| "https://assessor.example.com/v1/assess".into());
    let url = reqwest::Url::parse(&endpoint).context("invalid Assessor endpoint")?;
    if url.username() != ""
        || url.password().is_some()
        || (url.scheme() != "https"
            && !(url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost"))))
    {
        bail!("Assessor endpoint must use TLS or loopback without URL credentials");
    }
    let reply = state.http_client.post(url)
        .bearer_auth(key)
        .timeout(Duration::from_secs(15))
        .json(&json!({"model":"assessor-latest","state":evidence,"questions":{"investigate":{
            "type":"noul",
            "instructions":"Do the recent 30 minutes show a credible, actionable degradation compared with the preceding 30 minutes for this one service? Abstain when the evidence does not support a causal investigation. A change in raw log count alone is not proof of a problem.",
            "criteria":{"true":"Fresh, comparable evidence warrants investigation, not an automatic change","false":"Insufficient evidence or normal expected activity"}
        }}}))
        .send().await.context("model assessment request failed")?;
    if !reply.status().is_success() {
        bail!("model assessment rejected the request ({})", reply.status());
    }
    let answer: AssessorAnswer = reply
        .json()
        .await
        .context("model assessment response was malformed")?;
    let probability = answer.answers.investigate.noul;
    if answer.answers.investigate.answer_type != "noul"
        || !probability.is_finite()
        || !(0.0..=1.0).contains(&probability)
    {
        bail!("model assessment did not provide a valid probability");
    }
    Ok(Decision {
        probability,
        evidence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_is_read_only_and_excludes_replays() {
        for (_, table, failure) in SCANS {
            let bounds = [
                "2026-09-28 01:00:00.000000",
                "2026-09-28 01:30:00.000000",
                "2026-09-28 01:55:00.000000",
                "2026-09-28 02:00:00.000000",
            ]
            .map(str::to_owned);
            let sql = summary_sql("", table, "tenant's-service", failure, &bounds);
            assert!(crate::sql::validate_read_only(&sql).is_ok(), "{sql}");
            assert!(sql.contains("service_name='tenant''s-service'"));
            assert!(sql.contains("resource_attributes NOT LIKE '%production-journald-backfill%'"));
        }
    }
}
