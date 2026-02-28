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

	"github.com/AkshatM/observe/internal/query"
	"github.com/AkshatM/observe/pkg/schema"
	_ "github.com/marcboeker/go-duckdb"
)

func TestQueryHandler(t *testing.T) {
	db, err := sql.Open("duckdb", "")
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()

	parquetPath := "../../experiments/incident-gen/testdata/incident.parquet"
	_, err = db.Exec(fmt.Sprintf(`CREATE VIEW observe AS SELECT * FROM read_parquet('%s')`, parquetPath))
	if err != nil {
		t.Fatalf("create view: %v", err)
	}

	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
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
	})

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
	if len(block.Metrics) == 0 {
		t.Error("no metrics")
	}
	if len(block.Spans) == 0 {
		t.Error("no spans")
	}
	if len(block.Logs) == 0 {
		t.Error("no logs")
	}
	if len(block.Profiles) == 0 {
		t.Error("no profiles")
	}
	if len(block.Linked.Traces) == 0 {
		t.Error("no linked traces")
	}

	// Verify JSON roundtrip produces valid structure
	out, err := json.Marshal(block)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	if len(out) < 100 {
		t.Errorf("response too small: %d bytes", len(out))
	}
}
