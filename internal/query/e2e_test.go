package query

import (
	"context"
	"database/sql"
	"fmt"
	"strings"
	"testing"
	"time"

	_ "github.com/duckdb/duckdb-go/v2"
)

// setupE2EDB creates a DuckDB connection with a view over the incident parquet.
func setupE2EDB(t *testing.T) *sql.DB {
	t.Helper()
	db, err := sql.Open("duckdb", "")
	if err != nil {
		t.Fatal(err)
	}
	path := "../../experiments/incident-gen/testdata/incident.parquet"
	_, err = db.Exec(fmt.Sprintf(`CREATE VIEW observe AS SELECT * FROM read_parquet('%s')`, path))
	if err != nil {
		db.Close()
		t.Fatalf("create view: %v (run incident-gen first)", err)
	}
	return db
}

func queryScalar[T any](t *testing.T, db *sql.DB, q string) T {
	t.Helper()
	var v T
	if err := db.QueryRow(q).Scan(&v); err != nil {
		t.Fatalf("query %q: %v", q, err)
	}
	return v
}

func queryStrings(t *testing.T, db *sql.DB, q string) []string {
	t.Helper()
	rows, err := db.Query(q)
	if err != nil {
		t.Fatalf("query: %v", err)
	}
	defer rows.Close()
	var result []string
	for rows.Next() {
		var s string
		if err := rows.Scan(&s); err != nil {
			t.Fatal(err)
		}
		result = append(result, s)
	}
	return result
}

// --- Raw data validation ---

func TestE2E_SignalTypes(t *testing.T) {
	db := setupE2EDB(t)
	defer db.Close()

	signals := queryStrings(t, db, `SELECT DISTINCT signal FROM observe ORDER BY signal`)
	want := []string{"log", "metric", "profile", "span"}
	if len(signals) != len(want) {
		t.Fatalf("signals = %v, want %v", signals, want)
	}
	for i, s := range signals {
		if s != want[i] {
			t.Errorf("signal[%d] = %q, want %q", i, s, want[i])
		}
	}
}

func TestE2E_Services(t *testing.T) {
	db := setupE2EDB(t)
	defer db.Close()

	services := queryStrings(t, db, `SELECT DISTINCT service FROM observe ORDER BY service`)
	if len(services) != 2 || services[0] != "checkout" || services[1] != "payments" {
		t.Fatalf("services = %v, want [checkout, payments]", services)
	}
}

func TestE2E_MemoryClimbsToOOM(t *testing.T) {
	db := setupE2EDB(t)
	defer db.Close()

	// Memory should start around 200MB and reach 512MB
	minMem := queryScalar[float64](t, db,
		`SELECT min(value) FROM observe WHERE signal='metric' AND name='memory.usage' AND service='checkout'`)
	maxMem := queryScalar[float64](t, db,
		`SELECT max(value) FROM observe WHERE signal='metric' AND name='memory.usage' AND service='checkout'`)

	if minMem > 250 {
		t.Errorf("min memory = %.0f MB, want < 250 (baseline)", minMem)
	}
	if maxMem < 500 {
		t.Errorf("max memory = %.0f MB, want >= 500 (OOM)", maxMem)
	}
}

func TestE2E_LatencyDegrades(t *testing.T) {
	db := setupE2EDB(t)
	defer db.Close()

	// Baseline latency should be ~50-80ms, peak should be >1000ms
	rows, err := db.Query(`
		SELECT strftime(to_timestamp(timestamp/1e9)::TIMESTAMP, '%H:%M') AS bucket,
		       avg(value) AS avg_ms
		FROM observe
		WHERE signal='metric' AND name='http.duration' AND service='checkout'
		GROUP BY bucket ORDER BY bucket
	`)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()

	var baseline, peak float64
	for rows.Next() {
		var bucket string
		var avg float64
		if err := rows.Scan(&bucket, &avg); err != nil {
			t.Fatal(err)
		}
		if bucket <= "14:04" {
			baseline = avg
		}
		if avg > peak {
			peak = avg
		}
	}

	if baseline > 100 {
		t.Errorf("baseline latency = %.0fms, want < 100ms", baseline)
	}
	if peak < 500 {
		t.Errorf("peak latency = %.0fms, want > 500ms (degradation)", peak)
	}
}

func TestE2E_ErrorSpans(t *testing.T) {
	db := setupE2EDB(t)
	defer db.Close()

	count := queryScalar[int](t, db,
		`SELECT count(*) FROM observe WHERE signal='span' AND status='ERROR'`)
	if count == 0 {
		t.Fatal("no error spans found")
	}

	// Error spans should be slow (>2000ms)
	minDur := queryScalar[float64](t, db,
		`SELECT min(value) FROM observe WHERE signal='span' AND status='ERROR'`)
	if minDur < 2000 {
		t.Errorf("min error span duration = %.0fms, want >= 2000", minDur)
	}

	// Error spans should be in both checkout and payments
	errorServices := queryStrings(t, db,
		`SELECT DISTINCT service FROM observe WHERE signal='span' AND status='ERROR' ORDER BY service`)
	if len(errorServices) != 2 || errorServices[0] != "checkout" || errorServices[1] != "payments" {
		t.Errorf("error span services = %v, want [checkout, payments]", errorServices)
	}
}

func TestE2E_OOMKillLog(t *testing.T) {
	db := setupE2EDB(t)
	defer db.Close()

	msg := queryScalar[string](t, db,
		`SELECT message FROM observe WHERE signal='log' AND message LIKE '%OOMKilled%'`)
	if !strings.Contains(msg, "512Mi") {
		t.Errorf("OOMKill message = %q, want to contain '512Mi'", msg)
	}
	if !strings.Contains(msg, "exitCode=137") {
		t.Errorf("OOMKill message = %q, want to contain 'exitCode=137'", msg)
	}
}

func TestE2E_LogTimeline(t *testing.T) {
	db := setupE2EDB(t)
	defer db.Close()

	// The error log sequence should be: WARN pool near capacity → WARN memory pressure → ERROR OOMKilled → ERROR pool exhausted
	rows, err := db.Query(`
		SELECT strftime(to_timestamp(timestamp/1e9)::TIMESTAMP, '%H:%M:%S') AS t,
		       status, message
		FROM observe
		WHERE signal='log' AND service='checkout' AND status IN ('WARN','ERROR')
		ORDER BY timestamp
	`)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()

	var timeline []string
	for rows.Next() {
		var ts, status, msg string
		if err := rows.Scan(&ts, &status, &msg); err != nil {
			t.Fatal(err)
		}
		timeline = append(timeline, fmt.Sprintf("%s %s %s", ts, status, msg))
	}

	if len(timeline) < 4 {
		t.Fatalf("expected at least 4 warn/error logs, got %d: %v", len(timeline), timeline)
	}

	// Verify ordering: pool near capacity before OOM before pool exhausted
	var poolWarnIdx, oomIdx, poolExhaustIdx int
	for i, line := range timeline {
		if strings.Contains(line, "pool near capacity") {
			poolWarnIdx = i
		}
		if strings.Contains(line, "OOMKilled") {
			oomIdx = i
		}
		if strings.Contains(line, "pool exhausted") {
			poolExhaustIdx = i
		}
	}
	if poolWarnIdx >= oomIdx {
		t.Error("pool near capacity should come before OOMKill")
	}
	if oomIdx >= poolExhaustIdx {
		t.Error("OOMKill should come before pool exhausted")
	}
}

func TestE2E_ProfilesShowGCThrashing(t *testing.T) {
	db := setupE2EDB(t)
	defer db.Close()

	// Top function by samples should be GC-related
	topFunc := queryScalar[string](t, db, `
		SELECT stacktrace[len(stacktrace)] AS leaf
		FROM observe
		WHERE signal='profile' AND service='checkout'
		GROUP BY leaf
		ORDER BY sum(value) DESC
		LIMIT 1
	`)
	if !strings.Contains(topFunc, "gcDrain") && !strings.Contains(topFunc, "gcBgMarkWorker") {
		t.Errorf("top profile function = %q, want GC-related (gcDrain or gcBgMarkWorker)", topFunc)
	}
}

func TestE2E_CrossServiceTraces(t *testing.T) {
	db := setupE2EDB(t)
	defer db.Close()

	// Trace IDs that appear in both checkout and payments
	count := queryScalar[int](t, db, `
		SELECT count(*) FROM (
			SELECT trace_id
			FROM observe
			WHERE signal='span' AND trace_id IS NOT NULL
			GROUP BY trace_id
			HAVING count(DISTINCT service) > 1
		)
	`)
	if count == 0 {
		t.Fatal("no cross-service traces found")
	}
	if count < 3 {
		t.Errorf("cross-service traces = %d, want >= 3 (one per error trace)", count)
	}
}

func TestE2E_SpanParentChild(t *testing.T) {
	db := setupE2EDB(t)
	defer db.Close()

	// Spans with parent_span_id should reference an existing span_id
	orphans := queryScalar[int](t, db, `
		SELECT count(*) FROM observe a
		WHERE a.signal='span' AND a.parent_span_id IS NOT NULL
		  AND NOT EXISTS (
			SELECT 1 FROM observe b WHERE b.signal='span' AND b.span_id = a.parent_span_id
		  )
	`)
	if orphans > 0 {
		t.Errorf("%d spans have parent_span_id referencing nonexistent span_id", orphans)
	}

	// Error traces should have checkout → payments parent-child relationship
	count := queryScalar[int](t, db, `
		SELECT count(*) FROM observe parent
		JOIN observe child ON parent.span_id = child.parent_span_id
		WHERE parent.signal='span' AND child.signal='span'
		  AND parent.service='checkout' AND child.service='payments'
		  AND parent.status='ERROR'
	`)
	if count == 0 {
		t.Fatal("no checkout→payments parent-child error spans found")
	}
}

func TestE2E_DeployVersionTransition(t *testing.T) {
	db := setupE2EDB(t)
	defer db.Close()

	// Should see both v2.3.0 (baseline) and v2.3.1 (after deploy)
	versions := queryStrings(t, db,
		`SELECT DISTINCT deploy_version FROM observe WHERE service='checkout' AND deploy_version != '' ORDER BY deploy_version`)
	if len(versions) != 2 || versions[0] != "v2.3.0" || versions[1] != "v2.3.1" {
		t.Errorf("deploy versions = %v, want [v2.3.0, v2.3.1]", versions)
	}

	// All errors should be on v2.3.1
	badVersion := queryScalar[int](t, db,
		`SELECT count(*) FROM observe WHERE signal='span' AND status='ERROR' AND service='checkout' AND deploy_version != 'v2.3.1'`)
	if badVersion > 0 {
		t.Errorf("%d error spans on non-v2.3.1 deploy version", badVersion)
	}
}

func TestE2E_SortOrder(t *testing.T) {
	db := setupE2EDB(t)
	defer db.Close()

	// Data should be sorted by (service, signal, timestamp)
	violations := queryScalar[int](t, db, `
		WITH numbered AS (
			SELECT service, signal, timestamp, row_number() OVER () AS rn
			FROM observe
		)
		SELECT count(*) FROM numbered a
		JOIN numbered b ON b.rn = a.rn + 1
		WHERE (a.service > b.service)
		   OR (a.service = b.service AND a.signal > b.signal)
		   OR (a.service = b.service AND a.signal = b.signal AND a.timestamp > b.timestamp)
	`)
	if violations > 0 {
		t.Errorf("%d sort order violations (service, signal, timestamp)", violations)
	}
}

// --- Investigation block validation ---

func TestE2E_InvestigationBlock_Content(t *testing.T) {
	db := setupE2EDB(t)
	defer db.Close()

	block, err := Investigate(context.Background(), db, "observe", InvestigateRequest{
		Service: "checkout",
		From:    time.Date(2026, 3, 1, 14, 0, 0, 0, time.UTC),
		To:      time.Date(2026, 3, 1, 14, 25, 0, 0, time.UTC),
	})
	if err != nil {
		t.Fatal(err)
	}

	// --- Metrics ---
	if len(block.Metrics) == 0 {
		t.Fatal("no metrics in block")
	}
	// Should have memory.usage metrics showing the climb
	var memMetrics []float64
	for _, m := range block.Metrics {
		if m.Name == "memory.usage" && m.Avg != nil {
			memMetrics = append(memMetrics, *m.Avg)
		}
	}
	if len(memMetrics) < 5 {
		t.Errorf("only %d memory.usage buckets, want >= 5", len(memMetrics))
	}
	// Memory should peak above 500MB (OOM at 512Mi)
	var peakMem float64
	for _, v := range memMetrics {
		if v > peakMem {
			peakMem = v
		}
	}
	if peakMem < 500 {
		t.Errorf("peak memory = %.0f MB, want >= 500 (OOM)", peakMem)
	}

	// --- Spans ---
	if len(block.Spans) == 0 {
		t.Fatal("no spans in block")
	}
	// All returned spans should be errors (query filters for ERROR or slow >1000ms)
	for _, s := range block.Spans {
		if s.Status != "ERROR" && s.DurationMs <= 1000 {
			t.Errorf("span %q has status=%s dur=%.0fms, should be ERROR or >1000ms", s.Name, s.Status, s.DurationMs)
		}
	}
	// Each error span tree should have a db.query child
	for _, s := range block.Spans {
		if s.Status == "ERROR" && s.Name == "POST /checkout" {
			hasDBChild := false
			for _, c := range s.Children {
				if c.Name == "db.query" {
					hasDBChild = true
				}
			}
			if !hasDBChild {
				t.Errorf("error span %q (trace=%s) missing db.query child", s.Name, s.TraceID)
			}
		}
	}

	// --- Logs ---
	if len(block.Logs) == 0 {
		t.Fatal("no logs in block")
	}
	// Should contain OOMKill message
	hasOOM := false
	for _, l := range block.Logs {
		if strings.Contains(l.Message, "OOMKilled") {
			hasOOM = true
		}
	}
	if !hasOOM {
		t.Error("logs missing OOMKilled message")
	}

	// --- Profiles ---
	if len(block.Profiles) == 0 {
		t.Fatal("no profiles in block")
	}
	// Top function should be GC-related
	if !strings.Contains(block.Profiles[0].Function, "gc") && !strings.Contains(block.Profiles[0].Function, "GC") {
		t.Errorf("top profile function = %q, want GC-related", block.Profiles[0].Function)
	}

	// --- Linked traces ---
	if len(block.Linked.Traces) == 0 {
		t.Fatal("no linked traces in block")
	}
	// Linked traces should contain spans from both checkout and payments
	for _, lt := range block.Linked.Traces {
		services := map[string]bool{}
		for _, s := range lt.Spans {
			services[s.Service] = true
			for _, c := range s.Children {
				services[c.Service] = true
			}
		}
		if !services["checkout"] || !services["payments"] {
			t.Errorf("linked trace %s has services %v, want both checkout and payments", lt.TraceID[:16], services)
		}
	}

	// Linked logs should contain cross-service error
	if len(block.Linked.Logs) == 0 {
		t.Fatal("no linked logs in block")
	}
	hasPaymentsLog := false
	for _, l := range block.Linked.Logs {
		if strings.Contains(l.Message, "payments") {
			hasPaymentsLog = true
		}
	}
	if !hasPaymentsLog {
		t.Error("linked logs missing payments service error")
	}
}
