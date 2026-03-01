package query

import (
	"context"
	"testing"
	"time"

	_ "github.com/duckdb/duckdb-go/v2"
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

func TestParquetGlobs_SingleHour(t *testing.T) {
	from := time.Date(2026, 3, 1, 14, 5, 0, 0, time.UTC)
	to := time.Date(2026, 3, 1, 14, 25, 0, 0, time.UTC)

	globs := ParquetGlobs("observe", from, to)
	if len(globs) != 1 {
		t.Fatalf("expected 1 glob, got %d: %v", len(globs), globs)
	}
	// 2026-03-01 14:00 UTC = 1772373600
	want := "s3://observe/1772373600/*.parquet"
	if globs[0] != want {
		t.Errorf("got %q, want %q", globs[0], want)
	}
}

func TestParquetGlobs_MultiHour(t *testing.T) {
	from := time.Date(2026, 3, 1, 13, 30, 0, 0, time.UTC)
	to := time.Date(2026, 3, 1, 15, 0, 0, 0, time.UTC)

	globs := ParquetGlobs("observe", from, to)
	if len(globs) != 3 {
		t.Fatalf("expected 3 globs (hours 13,14,15), got %d: %v", len(globs), globs)
	}
	// 2026-03-01 13:00 UTC = 1772370000
	want0 := "s3://observe/1772370000/*.parquet"
	if globs[0] != want0 {
		t.Errorf("globs[0] = %q, want %q", globs[0], want0)
	}
}

func TestParquetGlobs_CrossDay(t *testing.T) {
	from := time.Date(2026, 3, 1, 23, 30, 0, 0, time.UTC)
	to := time.Date(2026, 3, 2, 0, 15, 0, 0, time.UTC)

	globs := ParquetGlobs("observe", from, to)
	if len(globs) != 2 {
		t.Fatalf("expected 2 globs (hour 23, hour 0), got %d: %v", len(globs), globs)
	}
}

func TestParquetTable_SingleHour(t *testing.T) {
	from := time.Date(2026, 3, 1, 14, 5, 0, 0, time.UTC)
	to := time.Date(2026, 3, 1, 14, 25, 0, 0, time.UTC)
	got := ParquetTable("observe", from, to)

	want := "read_parquet('s3://observe/1772373600/*.parquet')"
	if got != want {
		t.Errorf("got %q, want %q", got, want)
	}
}

func TestParquetTable_MultiHour(t *testing.T) {
	from := time.Date(2026, 3, 1, 13, 30, 0, 0, time.UTC)
	to := time.Date(2026, 3, 1, 14, 15, 0, 0, time.UTC)
	got := ParquetTable("observe", from, to)

	want := "read_parquet(['s3://observe/1772370000/*.parquet', 's3://observe/1772373600/*.parquet'])"
	if got != want {
		t.Errorf("got %q,\nwant %q", got, want)
	}
}
