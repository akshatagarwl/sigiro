package query

import (
	"context"
	"database/sql"
	"fmt"
	"strings"

	"github.com/akshatagarwl/observe/pkg/schema"
)

// queryLinkedSpans fetches all spans (across all services) for the given trace IDs.
func queryLinkedSpans(ctx context.Context, db *sql.DB, table string, traceIDs []string) (map[string][]schema.SpanNode, error) {
	if len(traceIDs) == 0 {
		return nil, nil
	}

	placeholders := make([]string, len(traceIDs))
	args := make([]any, len(traceIDs))
	for i, tid := range traceIDs {
		placeholders[i] = fmt.Sprintf("$%d", i+1)
		args[i] = tid
	}

	rows, err := db.QueryContext(ctx, fmt.Sprintf(`
		SELECT trace_id, span_id, parent_span_id, service, name,
		       value AS duration_ms, status, kind
		FROM %s
		WHERE trace_id IN (%s) AND signal='span'
		ORDER BY trace_id, timestamp
	`, table, strings.Join(placeholders, ",")), args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	result := map[string][]schema.SpanNode{}
	for rows.Next() {
		var s schema.SpanNode
		var tid, sid, psid, svc, status, kind sql.NullString
		var dur sql.NullFloat64
		if err := rows.Scan(&tid, &sid, &psid, &svc, &s.Name, &dur, &status, &kind); err != nil {
			return nil, err
		}
		s.TraceID = tid.String
		s.SpanID = sid.String
		if psid.Valid {
			s.ParentID = &psid.String
		}
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

	placeholders := make([]string, len(traceIDs))
	args := make([]any, len(traceIDs)+1)
	for i, tid := range traceIDs {
		placeholders[i] = fmt.Sprintf("$%d", i+1)
		args[i] = tid
	}
	svcPlaceholder := fmt.Sprintf("$%d", len(traceIDs)+1)
	args[len(traceIDs)] = excludeService

	rows, err := db.QueryContext(ctx, fmt.Sprintf(`
		SELECT strftime(to_timestamp(timestamp/1000000000)::TIMESTAMP, '%%H:%%M:%%S') AS t,
		       service, status, message, trace_id
		FROM %s
		WHERE trace_id IN (%s) AND signal='log' AND service != %s
		ORDER BY timestamp
	`, table, strings.Join(placeholders, ","), svcPlaceholder), args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var result []schema.LogRow
	for rows.Next() {
		var l schema.LogRow
		var svc, status, msg, tid sql.NullString
		if err := rows.Scan(&l.T, &svc, &status, &msg, &tid); err != nil {
			return nil, err
		}
		l.Status = status.String
		l.Message = "[" + svc.String + "] " + msg.String
		if tid.Valid {
			l.TraceID = &tid.String
		}
		result = append(result, l)
	}
	return result, rows.Err()
}
