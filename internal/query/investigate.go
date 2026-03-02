package query

import (
	"context"
	"database/sql"
	"fmt"
	"time"

	"github.com/akshatagarwl/observe/pkg/schema"
)

// InvestigateRequest defines the parameters for an investigation query.
type InvestigateRequest struct {
	Service string    `json:"service"`
	From    time.Time `json:"from"`
	To      time.Time `json:"to"`
	TraceID string    `json:"trace_id,omitempty"`
}

// Investigate runs a single Parquet scan into a temp table, then 4 cheap
// in-memory SQL queries for each signal type, plus linked trace follow-up.
func Investigate(ctx context.Context, db *sql.DB, table string, req InvestigateRequest) (*schema.InvestigationBlock, error) {
	t1 := req.From.UnixNano()
	t2 := req.To.UnixNano()

	block := &schema.InvestigationBlock{
		Service: req.Service,
		Window:  fmt.Sprintf("%s - %s", req.From.Format("15:04"), req.To.Format("15:04")),
	}

	if err := runPrimaryQueries(ctx, db, table, req.Service, t1, t2, block); err != nil {
		return nil, err
	}

	// Collect error trace IDs from spans and logs
	traceIDs := collectErrorTraceIDs(block.Spans, block.Logs)

	if err := addLinkedData(ctx, db, table, req.Service, traceIDs, block); err != nil {
		return nil, err
	}

	// Build span trees for the main spans section
	block.Spans = schema.BuildSpanTrees(block.Spans)

	return block, nil
}

// runPrimaryQueries materializes one Parquet scan into a temp table, then
// runs 4 SQL aggregations against it. All I/O happens in the CREATE TABLE;
// the follow-up queries are pure in-memory scans.
func runPrimaryQueries(
	ctx context.Context,
	db *sql.DB,
	table, service string,
	t1, t2 int64,
	block *schema.InvestigationBlock,
) error {
	// Pin a single connection so the temp table is visible to all queries.
	conn, err := db.Conn(ctx)
	if err != nil {
		return fmt.Errorf("acquire connection: %w", err)
	}
	defer conn.Close()

	// Single Parquet scan → temp table (CREATE OR REPLACE handles leftover
	// from a previous request that reused this pooled connection).
	_, err = conn.ExecContext(ctx, `
		CREATE OR REPLACE TEMP TABLE _scan AS
		SELECT * FROM `+table+`
		WHERE service = $1 AND timestamp BETWEEN $2 AND $3
	`, service, t1, t2)
	if err != nil {
		return fmt.Errorf("create scan table: %w", err)
	}
	defer conn.ExecContext(context.Background(), `DROP TABLE IF EXISTS _scan`)

	block.Metrics, err = scanMetrics(ctx, conn)
	if err != nil {
		return fmt.Errorf("metrics: %w", err)
	}

	block.Spans, err = scanSpans(ctx, conn)
	if err != nil {
		return fmt.Errorf("spans: %w", err)
	}

	block.Logs, err = scanLogs(ctx, conn)
	if err != nil {
		return fmt.Errorf("logs: %w", err)
	}

	block.Profiles, err = scanProfiles(ctx, conn)
	if err != nil {
		return fmt.Errorf("profiles: %w", err)
	}

	return nil
}

func scanMetrics(ctx context.Context, conn *sql.Conn) ([]schema.MetricRow, error) {
	rows, err := conn.QueryContext(ctx, `
		SELECT strftime(to_timestamp(timestamp/1000000000)::TIMESTAMP, '%H:%M') AS bucket,
		       name,
		       quantile_cont(value, 0.95) AS p95,
		       avg(value) AS avg,
		       count(*) AS n
		FROM _scan
		WHERE signal = 'metric'
		GROUP BY bucket, name
		ORDER BY bucket, name
	`)
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

func scanSpans(ctx context.Context, conn *sql.Conn) ([]schema.SpanNode, error) {
	rows, err := conn.QueryContext(ctx, `
		SELECT trace_id, span_id, parent_span_id, name,
		       value AS duration_ms, status, kind
		FROM _scan
		WHERE signal = 'span' AND (status = 'ERROR' OR value > 1000)
		ORDER BY value DESC
		LIMIT 50
	`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var result []schema.SpanNode

	for rows.Next() {
		var (
			s                            schema.SpanNode
			tid, sid, psid, status, kind sql.NullString
			dur                          sql.NullFloat64
		)

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

func scanLogs(ctx context.Context, conn *sql.Conn) ([]schema.LogRow, error) {
	rows, err := conn.QueryContext(ctx, `
		SELECT strftime(to_timestamp(timestamp/1000000000)::TIMESTAMP, '%H:%M:%S') AS t,
		       status, message, trace_id
		FROM _scan
		WHERE signal = 'log'
		ORDER BY timestamp DESC
		LIMIT 100
	`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var result []schema.LogRow

	for rows.Next() {
		var (
			l                schema.LogRow
			status, msg, tid sql.NullString
		)

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

func scanProfiles(ctx context.Context, conn *sql.Conn) ([]schema.ProfileRow, error) {
	rows, err := conn.QueryContext(ctx, `
		SELECT stacktrace[len(stacktrace)] AS leaf,
		       sum(value) AS samples
		FROM _scan
		WHERE signal = 'profile' AND stacktrace IS NOT NULL AND len(stacktrace) > 0
		GROUP BY leaf
		ORDER BY samples DESC
		LIMIT 10
	`)
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

func addLinkedData(
	ctx context.Context,
	db *sql.DB,
	table, service string,
	traceIDs []string,
	block *schema.InvestigationBlock,
) error {
	if len(traceIDs) == 0 {
		return nil
	}

	spansByTrace, err := queryLinkedSpans(ctx, db, table, traceIDs)
	if err != nil {
		return fmt.Errorf("linked spans: %w", err)
	}

	for tid, spans := range spansByTrace {
		trees := schema.BuildSpanTrees(spans)
		block.Linked.Traces = append(block.Linked.Traces, schema.LinkedTrace{
			TraceID: tid,
			Spans:   trees,
		})
	}

	linkedLogs, err := queryLinkedLogs(ctx, db, table, service, traceIDs)
	if err != nil {
		return fmt.Errorf("linked logs: %w", err)
	}

	block.Linked.Logs = linkedLogs

	return nil
}

// collectErrorTraceIDs gathers trace IDs from error spans and logs.
func collectErrorTraceIDs(spans []schema.SpanNode, logs []schema.LogRow) []string {
	seen := map[string]bool{}

	for _, s := range spans {
		if s.Status == "ERROR" && s.TraceID != "" {
			seen[s.TraceID] = true
		}
	}

	for _, l := range logs {
		if l.TraceID != nil && l.Status == "ERROR" {
			seen[*l.TraceID] = true
		}
	}

	result := make([]string, 0, len(seen))
	for tid := range seen {
		result = append(result, tid)
	}

	return result
}
