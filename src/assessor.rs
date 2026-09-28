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

fn checked_probability(reply: AssessorAnswer) -> Result<f64> {
    let answer = reply.answers.investigate;
    if answer.answer_type != "noul"
        || !answer.noul.is_finite()
        || !(0.0..=1.0).contains(&answer.noul)
    {
        bail!("model assessment did not provide a valid probability");
    }
    Ok(answer.noul)
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
    let probability = ask_assessor(&state.http_client, &endpoint, &key, &evidence).await?;
    Ok(Decision {
        probability,
        evidence,
    })
}

async fn ask_assessor(
    client: &reqwest::Client,
    endpoint: &str,
    key: &str,
    evidence: &Value,
) -> Result<f64> {
    let url = reqwest::Url::parse(endpoint).context("invalid Assessor endpoint")?;
    if url.username() != ""
        || url.password().is_some()
        || (url.scheme() != "https"
            && !(url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost"))))
    {
        bail!("Assessor endpoint must use TLS or loopback without URL credentials");
    }
    let reply = client.post(url)
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
    checked_probability(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bounded_state_and_typed_question_reach_a_mock_assessor_without_raw_telemetry() {
        use axum::{Json, Router, http::HeaderMap, routing::post};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/v1/assess",
            post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                assert_eq!(
                    headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer test-only-assessor-token")
                );
                assert_eq!(body["model"], "assessor-latest");
                assert_eq!(body["questions"]["investigate"]["type"], "noul");
                assert_eq!(body["state"]["signals"]["logs"]["recent"], 4);
                assert!(body.to_string().len() < 2_000);
                Json(json!({"answers":{"investigate":{"type":"noul","noul":0.91}}}))
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let evidence = json!({"service":"worker","signals":{"logs":{"recent":4,"baseline":3}}});
        let probability = ask_assessor(
            &reqwest::Client::new(),
            &format!("http://{address}/v1/assess"),
            "test-only-assessor-token",
            &evidence,
        )
        .await
        .unwrap();
        assert_eq!(probability, 0.91);
        server.abort();
    }

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

    #[test]
    fn only_typed_finite_probabilities_can_authorize_an_investigation() {
        for (payload, accepted) in [
            (
                json!({"answers":{"investigate":{"type":"noul","noul":0.9}}}),
                true,
            ),
            (
                json!({"answers":{"investigate":{"type":"choice","noul":0.9}}}),
                false,
            ),
            (
                json!({"answers":{"investigate":{"type":"noul","noul":1.2}}}),
                false,
            ),
            (
                json!({"answers":{"investigate":{"type":"noul","noul":-0.1}}}),
                false,
            ),
            (json!({"answers":{}}), false),
        ] {
            let decision = serde_json::from_value::<AssessorAnswer>(payload)
                .map_err(anyhow::Error::from)
                .and_then(checked_probability);
            assert_eq!(decision.is_ok(), accepted);
        }
    }
}
