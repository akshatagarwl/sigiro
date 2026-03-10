package query

import (
	"context"
	"database/sql"
	"fmt"
	"strings"

	"github.com/akshatagarwl/sigiro/pkg/schema"
)

// sqlPlaceholders returns ("$1,$2,...$n", []any{vals...}) for use in IN clauses.
func sqlPlaceholders(vals []string) (string, []any) {
	ph := make([]string, len(vals))
	args := make([]any, len(vals))

	for i, v := range vals {
		ph[i] = fmt.Sprintf("$%d", i+1)
		args[i] = v
	}

	return strings.Join(ph, ","), args
}

// queryLinkedSpans fetches all spans (across all services) for the given trace IDs.
// FRAGILE: builds an IN clause with one placeholder per trace ID — no upper
// bound on len(traceIDs). A service with many error traces in the window can
// produce a query with thousands of placeholders, hitting DuckDB query-size
// limits or causing extreme memory use during plan/execution.
func queryLinkedSpans(ctx context.Context, db *sql.DB, table string, traceIDs []string) (map[string][]schema.SpanNode, error) {
	if len(traceIDs) == 0 {
		return nil, nil
	}

	ph, args := sqlPlaceholders(traceIDs)

	rows, err := db.QueryContext(ctx, fmt.Sprintf(`
		SELECT trace_id, span_id, parent_span_id, service, name,
		       value AS duration_ms, status, kind
		FROM %s
		-- FRAGILE: no time-range filter — scans ALL partitions for matching
		-- trace IDs. For a large bucket this is a full table scan.
		WHERE trace_id IN (%s) AND signal='span'
		ORDER BY trace_id, timestamp
	`, table, ph), args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	result := map[string][]schema.SpanNode{}

	for rows.Next() {
		var (
			s                                 schema.SpanNode
			tid, sid, psid, svc, status, kind sql.NullString
			dur                               sql.NullFloat64
		)

		if err := rows.Scan(&tid, &sid, &psid, &svc, &s.Name, &dur, &status, &kind); err != nil {
			return nil, err
		}

		s.TraceID = tid.String

		s.SpanID = sid.String
		s.ParentID = nullStrPtr(psid)
		s.Service = svc.String
		s.DurationMs = dur.Float64
		s.Status = status.String
		s.Kind = kind.String
		result[s.TraceID] = append(result[s.TraceID], s)
	}

	return result, rows.Err()
}

// queryLinkedLogs fetches logs from other services for the given trace IDs.
func queryLinkedLogs(ctx context.Context, db *sql.DB, table, excludeService string, traceIDs []string) ([]schema.LogRow, error) {
	if len(traceIDs) == 0 {
		return nil, nil
	}

	ph, args := sqlPlaceholders(traceIDs)
	svcPlaceholder := fmt.Sprintf("$%d", len(traceIDs)+1)

	args = append(args, excludeService)

	rows, err := db.QueryContext(ctx, fmt.Sprintf(`
		SELECT strftime(to_timestamp(timestamp/1000000000)::TIMESTAMP, '%%H:%%M:%%S') AS t,
		       service, status, message, trace_id
		FROM %s
		WHERE trace_id IN (%s) AND signal='log' AND service != %s
		ORDER BY timestamp
	`, table, ph, svcPlaceholder), args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var result []schema.LogRow

	for rows.Next() {
		var (
			l                     schema.LogRow
			svc, status, msg, tid sql.NullString
		)

		if err := rows.Scan(&l.T, &svc, &status, &msg, &tid); err != nil {
			return nil, err
		}

		l.Status = status.String

		l.Message = "[" + svc.String + "] " + msg.String
		l.TraceID = nullStrPtr(tid)

		result = append(result, l)
	}

	return result, rows.Err()
}
