package main

import (
	"context"
	"encoding/json"
	"flag"
	"os"
	"time"

	"github.com/akshatagarwl/sigiro/internal/query"
)

func runInvestigate(args []string) {
	fs := flag.NewFlagSet("investigate", flag.ContinueOnError)
	service := fs.String("service", "", "service name to investigate")
	window := fs.String("window", "15m", "time window (e.g. 15m, 1h)")
	traceID := fs.String("trace", "", "specific trace ID to investigate")

	if err := fs.Parse(args); err != nil {
		exitErr("invalid flags", err.Error(), "sigiro investigate --service NAME --window 15m")
	}

	if *service == "" && *traceID == "" {
		exitErr("missing --service or --trace", "", "sigiro investigate --service checkout --window 15m")
	}

	bucket := requireBucket()
	dur := parseWindow(*window)
	now := time.Now().UTC()
	from := now.Add(-dur)

	db := openDB()
	defer db.Close()

	table := query.ParquetTable(bucket, from, now, "")

	block, err := query.Investigate(context.Background(), db, table, query.InvestigateRequest{
		Service: *service,
		From:    from,
		To:      now,
		TraceID: *traceID,
	})
	if err != nil {
		exitErr("investigation failed", err.Error(), "check service name and time window")
	}

	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	enc.Encode(block)
}
