package query

import (
	"context"
	"database/sql"
	"fmt"
	"testing"
	"time"

	_ "github.com/marcboeker/go-duckdb"
)

func setupTestDB(t *testing.T) *sql.DB {
	t.Helper()
	db, err := sql.Open("duckdb", "")
	if err != nil {
		t.Fatal(err)
	}
	parquetPath := "../../experiments/incident-gen/testdata/incident.parquet"
	_, err = db.Exec(fmt.Sprintf(`CREATE VIEW observe AS SELECT * FROM read_parquet('%s')`, parquetPath))
	if err != nil {
		db.Close()
		t.Fatalf("create view: %v (does %s exist?)", err, parquetPath)
	}
	return db
}

func TestInvestigate_Checkout(t *testing.T) {
	db := setupTestDB(t)
	defer db.Close()

	block, err := Investigate(context.Background(), db, "observe", InvestigateRequest{
		Service: "checkout",
		From:    time.Date(2026, 3, 1, 14, 0, 0, 0, time.UTC),
		To:      time.Date(2026, 3, 1, 14, 25, 0, 0, time.UTC),
	})
	if err != nil {
		t.Fatal(err)
	}

	if block.Service != "checkout" {
		t.Errorf("service = %q, want checkout", block.Service)
	}
	if len(block.Metrics) == 0 {
		t.Error("expected metrics, got none")
	}
	if len(block.Spans) == 0 {
		t.Error("expected spans, got none")
	}
	if len(block.Logs) == 0 {
		t.Error("expected logs, got none")
	}
	if len(block.Profiles) == 0 {
		t.Error("expected profiles, got none")
	}

	// Should have error spans that trigger linked traces
	hasError := false
	for _, s := range block.Spans {
		if s.Status == "ERROR" {
			hasError = true
			break
		}
		for _, c := range s.Children {
			if c.Status == "ERROR" {
				hasError = true
				break
			}
		}
	}
	if !hasError {
		t.Error("expected at least one error span")
	}

	// Linked traces should include cross-service spans
	if len(block.Linked.Traces) == 0 {
		t.Error("expected linked traces, got none")
	}
	for _, lt := range block.Linked.Traces {
		if lt.TraceID == "" {
			t.Error("linked trace has empty trace_id")
		}
		if len(lt.Spans) == 0 {
			t.Error("linked trace has no spans")
		}
	}
}

func TestInvestigate_NoData(t *testing.T) {
	db := setupTestDB(t)
	defer db.Close()

	block, err := Investigate(context.Background(), db, "observe", InvestigateRequest{
		Service: "nonexistent",
		From:    time.Date(2026, 3, 1, 14, 0, 0, 0, time.UTC),
		To:      time.Date(2026, 3, 1, 14, 25, 0, 0, time.UTC),
	})
	if err != nil {
		t.Fatal(err)
	}
	if len(block.Metrics) != 0 || len(block.Spans) != 0 || len(block.Logs) != 0 || len(block.Profiles) != 0 {
		t.Error("expected empty results for nonexistent service")
	}
}

func TestParquetGlob_SingleHour(t *testing.T) {
	from := time.Date(2026, 3, 1, 14, 5, 0, 0, time.UTC)
	to := time.Date(2026, 3, 1, 14, 25, 0, 0, time.UTC)
	got := ParquetGlob("observe", from, to)
	want := "s3://observe/year=2026/month=03/day=01/hour=14/*.parquet"
	if got != want {
		t.Errorf("got %q, want %q", got, want)
	}
}

func TestParquetGlob_MultiHour(t *testing.T) {
	from := time.Date(2026, 3, 1, 13, 30, 0, 0, time.UTC)
	to := time.Date(2026, 3, 1, 15, 0, 0, 0, time.UTC)
	got := ParquetGlob("observe", from, to)
	want := "s3://observe/year=2026/month=03/day=01/hour=*/*.parquet"
	if got != want {
		t.Errorf("got %q, want %q", got, want)
	}
}
