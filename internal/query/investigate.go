package query

import (
	"context"
	"database/sql"
	"fmt"
	"sync"
	"time"

	"github.com/AkshatM/observe/pkg/schema"
)

// InvestigateRequest defines the parameters for an investigation query.
type InvestigateRequest struct {
	Service string    `json:"service"`
	From    time.Time `json:"from"`
	To      time.Time `json:"to"`
	TraceID string    `json:"trace_id,omitempty"`
}

// Investigate runs the 4 parallel signal queries plus linked trace follow-up,
// assembling results into an InvestigationBlock.
func Investigate(ctx context.Context, db *sql.DB, table string, req InvestigateRequest) (*schema.InvestigationBlock, error) {
	t1 := req.From.UnixNano()
	t2 := req.To.UnixNano()

	block := &schema.InvestigationBlock{
		Service: req.Service,
		Window:  fmt.Sprintf("%s - %s", req.From.Format("15:04"), req.To.Format("15:04")),
	}

	var (
		mu   sync.Mutex
		wg   sync.WaitGroup
		errs [4]error
	)

	wg.Add(4)

	go func() {
		defer wg.Done()
		metrics, err := queryMetrics(ctx, db, table, req.Service, t1, t2)
		mu.Lock()
		block.Metrics = metrics
		errs[0] = err
		mu.Unlock()
	}()

	go func() {
		defer wg.Done()
		spans, err := querySpans(ctx, db, table, req.Service, t1, t2)
		mu.Lock()
		block.Spans = spans
		errs[1] = err
		mu.Unlock()
	}()

	go func() {
		defer wg.Done()
		logs, err := queryLogs(ctx, db, table, req.Service, t1, t2)
		mu.Lock()
		block.Logs = logs
		errs[2] = err
		mu.Unlock()
	}()

	go func() {
		defer wg.Done()
		profiles, err := queryProfiles(ctx, db, table, req.Service, t1, t2)
		mu.Lock()
		block.Profiles = profiles
		errs[3] = err
		mu.Unlock()
	}()

	wg.Wait()

	for i, err := range errs {
		if err != nil {
			return nil, fmt.Errorf("query %d failed: %w", i, err)
		}
	}

	// Collect error trace IDs from spans and logs
	traceIDs := collectErrorTraceIDs(block.Spans, block.Logs)

	if len(traceIDs) > 0 {
		spansByTrace, err := queryLinkedSpans(ctx, db, table, traceIDs)
		if err != nil {
			return nil, fmt.Errorf("linked spans: %w", err)
		}
		for tid, spans := range spansByTrace {
			trees := schema.BuildSpanTrees(spans)
			block.Linked.Traces = append(block.Linked.Traces, schema.LinkedTrace{
				TraceID: tid,
				Spans:   trees,
			})
		}

		linkedLogs, err := queryLinkedLogs(ctx, db, table, req.Service, traceIDs)
		if err != nil {
			return nil, fmt.Errorf("linked logs: %w", err)
		}
		block.Linked.Logs = linkedLogs
	}

	// Build span trees for the main spans section
	block.Spans = schema.BuildSpanTrees(block.Spans)

	return block, nil
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
