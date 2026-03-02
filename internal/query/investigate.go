package query

import (
	"context"
	"database/sql"
	"fmt"
	"math"
	"sort"
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

// Investigate runs a single scan of the unified table plus linked trace follow-up,
// assembling results into an InvestigationBlock.
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

type metricKey struct{ bucket, name string }

// runPrimaryQueries executes a single scan over the unified table and
// aggregates metrics, spans, logs, and profiles in Go.
func runPrimaryQueries(
	ctx context.Context,
	db *sql.DB,
	table, service string,
	t1, t2 int64,
	block *schema.InvestigationBlock,
) error {
	rows, err := db.QueryContext(ctx, `
		SELECT signal, timestamp, name, value, status, trace_id, span_id,
		       parent_span_id, kind, message,
		       CASE WHEN signal = 'profile' AND stacktrace IS NOT NULL AND len(stacktrace) > 0
		            THEN stacktrace[len(stacktrace)]
		            ELSE NULL END AS leaf_function
		FROM `+table+`
		WHERE service = $1 AND timestamp BETWEEN $2 AND $3
		ORDER BY signal, timestamp
	`, service, t1, t2)
	if err != nil {
		return err
	}
	defer rows.Close()

	metricAccum := map[metricKey][]float64{}

	var (
		spans []schema.SpanNode
		logs  []schema.LogRow
	)

	profileAccum := map[string]float64{}

	for rows.Next() {
		var (
			signal   string
			ts       int64
			name     sql.NullString
			value    sql.NullFloat64
			status   sql.NullString
			traceID  sql.NullString
			spanID   sql.NullString
			parentID sql.NullString
			kind     sql.NullString
			message  sql.NullString
			leaf     sql.NullString
		)

		if err := rows.Scan(&signal, &ts, &name, &value, &status,
			&traceID, &spanID, &parentID, &kind, &message, &leaf); err != nil {
			return fmt.Errorf("scan row: %w", err)
		}

		switch signal {
		case "metric":
			bucket := time.Unix(0, ts).UTC().Format("15:04")
			key := metricKey{bucket, name.String}
			metricAccum[key] = append(metricAccum[key], value.Float64)

		case "span":
			dur := value.Float64
			if status.String == "ERROR" || dur > 1000 {
				s := schema.SpanNode{
					TraceID:    traceID.String,
					SpanID:     spanID.String,
					Name:       name.String,
					DurationMs: dur,
					Status:     status.String,
					Kind:       kind.String,
				}
				if parentID.Valid {
					s.ParentID = &parentID.String
				}

				spans = append(spans, s)
			}

		case "log":
			l := schema.LogRow{
				T:       time.Unix(0, ts).UTC().Format("15:04:05"),
				Status:  status.String,
				Message: message.String,
			}
			if traceID.Valid {
				l.TraceID = &traceID.String
			}

			logs = append(logs, l)

		case "profile":
			if leaf.Valid && leaf.String != "" {
				profileAccum[leaf.String] += value.Float64
			}
		}
	}

	if err := rows.Err(); err != nil {
		return err
	}

	// Build metrics: sort values, compute p95/avg/count
	block.Metrics = buildMetrics(metricAccum)

	// Build spans: sort by duration desc, limit 50
	sort.Slice(spans, func(i, j int) bool { return spans[i].DurationMs > spans[j].DurationMs })

	if len(spans) > 50 {
		spans = spans[:50]
	}

	block.Spans = spans

	// Build logs: SQL gives ASC, we want DESC; reverse and limit 100
	for i, j := 0, len(logs)-1; i < j; i, j = i+1, j-1 {
		logs[i], logs[j] = logs[j], logs[i]
	}

	if len(logs) > 100 {
		logs = logs[:100]
	}

	block.Logs = logs

	// Build profiles: sort by samples desc, limit 10
	block.Profiles = buildProfiles(profileAccum)

	return nil
}

func buildMetrics(accum map[metricKey][]float64) []schema.MetricRow {
	result := make([]schema.MetricRow, 0, len(accum))

	for key, values := range accum {
		sort.Float64s(values)

		p95Idx := int(math.Ceil(0.95*float64(len(values)))) - 1
		if p95Idx < 0 {
			p95Idx = 0
		}

		p95 := values[p95Idx]

		var sum float64
		for _, v := range values {
			sum += v
		}

		avg := sum / float64(len(values))

		result = append(result, schema.MetricRow{
			Bucket: key.bucket,
			Name:   key.name,
			P95:    &p95,
			Avg:    &avg,
			N:      len(values),
		})
	}

	sort.Slice(result, func(i, j int) bool {
		if result[i].Bucket != result[j].Bucket {
			return result[i].Bucket < result[j].Bucket
		}

		return result[i].Name < result[j].Name
	})

	return result
}

func buildProfiles(accum map[string]float64) []schema.ProfileRow {
	result := make([]schema.ProfileRow, 0, len(accum))
	for fn, samples := range accum {
		result = append(result, schema.ProfileRow{Function: fn, Samples: samples})
	}

	sort.Slice(result, func(i, j int) bool { return result[i].Samples > result[j].Samples })

	if len(result) > 10 {
		result = result[:10]
	}

	return result
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
// Span status is upper-cased at write time (mapper.go); log severity text
// is already uppercase by OTel convention.
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
