package query

import (
	"context"
	"database/sql"

	"github.com/AkshatM/observe/pkg/schema"
)

func queryMetrics(ctx context.Context, db *sql.DB, table, service string, t1, t2 int64) ([]schema.MetricRow, error) {
	rows, err := db.QueryContext(ctx, `
		SELECT strftime(to_timestamp(timestamp/1000000000)::TIMESTAMP, '%H:%M') AS bucket,
		       name,
		       approx_quantile(value, 0.95) AS p95,
		       avg(value) AS avg,
		       count(*) AS n
		FROM `+table+`
		WHERE service=$1 AND signal='metric' AND timestamp BETWEEN $2 AND $3
		GROUP BY bucket, name
		ORDER BY bucket, name
	`, service, t1, t2)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var result []schema.MetricRow
	for rows.Next() {
		var m schema.MetricRow
		if err := rows.Scan(&m.Bucket, &m.Name, &m.P95, &m.Avg, &m.N); err != nil {
			return nil, err
		}
		result = append(result, m)
	}
	return result, rows.Err()
}

// querySpans returns error or slow spans for a service in a time window.
// value is span duration in milliseconds (see schema.UnitSpanDuration).
// The mapper converts OTel nanoseconds to ms at write time, so value > 1000 means > 1 second.
func querySpans(ctx context.Context, db *sql.DB, table, service string, t1, t2 int64) ([]schema.SpanNode, error) {
	rows, err := db.QueryContext(ctx, `
		SELECT trace_id, span_id, parent_span_id, name,
		       value AS duration_ms, status, kind
		FROM `+table+`
		WHERE service=$1 AND signal='span' AND timestamp BETWEEN $2 AND $3
		  AND (status='ERROR' OR value > 1000) -- status is upper-cased at write time in mapper.go
		ORDER BY value DESC
		LIMIT 50
	`, service, t1, t2)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var result []schema.SpanNode
	for rows.Next() {
		var s schema.SpanNode
		var tid, sid, psid, status, kind sql.NullString
		var dur sql.NullFloat64
		if err := rows.Scan(&tid, &sid, &psid, &s.Name, &dur, &status, &kind); err != nil {
			return nil, err
		}
		s.TraceID = tid.String
		s.SpanID = sid.String
		if psid.Valid {
			s.ParentID = &psid.String
		}
		s.DurationMs = dur.Float64
		s.Status = status.String
		s.Kind = kind.String
		result = append(result, s)
	}
	return result, rows.Err()
}

func queryLogs(ctx context.Context, db *sql.DB, table, service string, t1, t2 int64) ([]schema.LogRow, error) {
	rows, err := db.QueryContext(ctx, `
		SELECT strftime(to_timestamp(timestamp/1000000000)::TIMESTAMP, '%H:%M:%S') AS t,
		       status, message, trace_id
		FROM `+table+`
		WHERE service=$1 AND signal='log' AND timestamp BETWEEN $2 AND $3
		ORDER BY timestamp DESC
		LIMIT 100
	`, service, t1, t2)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var result []schema.LogRow
	for rows.Next() {
		var l schema.LogRow
		var status, msg, tid sql.NullString
		if err := rows.Scan(&l.T, &status, &msg, &tid); err != nil {
			return nil, err
		}
		l.Status = status.String
		l.Message = msg.String
		if tid.Valid {
			l.TraceID = &tid.String
		}
		result = append(result, l)
	}
	return result, rows.Err()
}

func queryProfiles(ctx context.Context, db *sql.DB, table, service string, t1, t2 int64) ([]schema.ProfileRow, error) {
	rows, err := db.QueryContext(ctx, `
		SELECT stacktrace[len(stacktrace)] AS leaf,
		       sum(value) AS samples
		FROM `+table+`
		WHERE service=$1 AND signal='profile' AND timestamp BETWEEN $2 AND $3
		GROUP BY leaf
		ORDER BY samples DESC
		LIMIT 10
	`, service, t1, t2)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var result []schema.ProfileRow
	for rows.Next() {
		var p schema.ProfileRow
		if err := rows.Scan(&p.Function, &p.Samples); err != nil {
			return nil, err
		}
		result = append(result, p)
	}
	return result, rows.Err()
}
