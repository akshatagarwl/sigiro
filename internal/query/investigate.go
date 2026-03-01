package query

import (
	"context"
	"database/sql"
	"fmt"
	"sync"
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

// Investigate runs the 4 parallel signal queries plus linked trace follow-up,
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

func runPrimaryQueries(
	ctx context.Context,
	db *sql.DB,
	table, service string,
	t1, t2 int64,
	block *schema.InvestigationBlock,
) error {
	var (
		mu   sync.Mutex
		wg   sync.WaitGroup
		errs [4]error
	)

	wg.Add(4)

	go func() {
		defer wg.Done()

		metrics, err := queryMetrics(ctx, db, table, service, t1, t2)

		mu.Lock()
		block.Metrics = metrics
		errs[0] = err
		mu.Unlock()
	}()

	go func() {
		defer wg.Done()

		spans, err := querySpans(ctx, db, table, service, t1, t2)

		mu.Lock()
		block.Spans = spans
		errs[1] = err
		mu.Unlock()
	}()

	go func() {
		defer wg.Done()

		logs, err := queryLogs(ctx, db, table, service, t1, t2)

		mu.Lock()
		block.Logs = logs
		errs[2] = err
		mu.Unlock()
	}()

	go func() {
		defer wg.Done()

		profiles, err := queryProfiles(ctx, db, table, service, t1, t2)

		mu.Lock()
		block.Profiles = profiles
		errs[3] = err
		mu.Unlock()
	}()

	wg.Wait()

	for i, err := range errs {
		if err != nil {
			return fmt.Errorf("query %d failed: %w", i, err)
		}
	}

	return nil
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
