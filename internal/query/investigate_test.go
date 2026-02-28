package query

import (
	"context"
	"testing"
	"time"

	_ "github.com/marcboeker/go-duckdb"
)

func TestInvestigate_NoData(t *testing.T) {
	db := setupE2EDB(t)
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
