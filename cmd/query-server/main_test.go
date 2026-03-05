package main

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/akshatagarwl/observe/internal/query"
	"github.com/akshatagarwl/observe/pkg/schema"
	_ "github.com/duckdb/duckdb-go/v2"
)

func newTestHandler(t *testing.T) http.HandlerFunc {
	t.Helper()

	db, err := sql.Open("duckdb", "")
	if err != nil {
		t.Fatal(err)
	}

	t.Cleanup(func() { db.Close() })

	parquetPath := "../../testdata/incident.parquet"

	_, err = db.Exec(fmt.Sprintf(`CREATE VIEW observe AS SELECT * FROM read_parquet('%s')`, parquetPath))
	if err != nil {
		t.Fatalf("create view: %v", err)
	}

	return func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Service string `json:"service"`
			From    string `json:"from"`
			To      string `json:"to"`
		}
		json.NewDecoder(r.Body).Decode(&req)
		fromT, _ := time.Parse(time.RFC3339, req.From)
		toT, _ := time.Parse(time.RFC3339, req.To)

		block, err := query.Investigate(r.Context(), db, "observe", query.InvestigateRequest{
			Service: req.Service,
			From:    fromT,
			To:      toT,
		})
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}

		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(block)
	}
}

func TestQueryHandler_IncidentBlock(t *testing.T) {
	handler := newTestHandler(t)

	body := `{"service":"checkout","from":"2026-03-01T14:00:00Z","to":"2026-03-01T14:25:00Z"}`
	req := httptest.NewRequest("POST", "/v1/query", strings.NewReader(body))
	req.Header.Set("Content-Type", "application/json")

	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, body = %s", w.Code, w.Body.String())
	}

	var block schema.InvestigationBlock
	if err := json.NewDecoder(w.Body).Decode(&block); err != nil {
		t.Fatalf("decode: %v", err)
	}

	if block.Service != "checkout" {
		t.Errorf("service = %q", block.Service)
	}

	// Metrics: memory should climb
	var memValues []float64

	for _, m := range block.Metrics {
		if m.Name == "memory.usage" && m.Avg != nil {
			memValues = append(memValues, *m.Avg)
		}
	}

	if len(memValues) < 3 {
		t.Errorf("only %d memory metric buckets", len(memValues))
	}

	var peakMem float64
	for _, v := range memValues {
		if v > peakMem {
			peakMem = v
		}
	}

	if peakMem < 500 {
		t.Errorf("peak memory = %.0f MB, want >= 500 (OOM)", peakMem)
	}

	// Spans: errors should appear before OK spans
	seenOK := false
	for _, s := range block.Spans {
		if s.Status == "ERROR" && seenOK {
			t.Errorf("error span %s appeared after OK span — errors should sort first", s.Name)
		}

		if s.Status != "ERROR" {
			seenOK = true
		}
	}

	// Logs: must contain OOMKill
	hasOOM := false

	for _, l := range block.Logs {
		if strings.Contains(l.Message, "OOMKilled") {
			hasOOM = true
		}
	}

	if !hasOOM {
		t.Error("logs missing OOMKilled")
	}

	// Profiles: top function should be GC
	if len(block.Profiles) == 0 {
		t.Fatal("no profiles")
	}

	if !strings.Contains(block.Profiles[0].Function, "gc") {
		t.Errorf("top profile = %q, want GC-related", block.Profiles[0].Function)
	}

	// Linked: cross-service traces
	if len(block.Linked.Traces) == 0 {
		t.Fatal("no linked traces")
	}

	for _, lt := range block.Linked.Traces {
		services := map[string]bool{}
		for _, s := range lt.Spans {
			services[s.Service] = true
			for _, c := range s.Children {
				services[c.Service] = true
			}
		}

		if !services["payments"] {
			t.Errorf("linked trace %s missing payments service", lt.TraceID[:16])
		}
	}
}
