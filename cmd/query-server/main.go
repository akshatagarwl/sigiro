package main

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"log/slog"
	"net/http"
	"os"
	"time"

	"github.com/akshatagarwl/observe/internal/query"
	"github.com/caarlos0/env/v11"

	_ "github.com/duckdb/duckdb-go/v2"
)

type Config struct {
	S3Region    string `env:"S3_REGION"`
	S3AccessKey string `env:"S3_ACCESS_KEY"`
	S3SecretKey string `env:"S3_SECRET_KEY"`
	S3Endpoint  string `env:"S3_ENDPOINT"`
	S3Bucket    string `env:"S3_BUCKET,required,notEmpty"`
	ListenAddr  string `env:"LISTEN_ADDR,required,notEmpty"`
}

func main() {
	cfg, err := env.ParseAs[Config]()
	if err != nil {
		slog.Error("parse config", "error", err)
		os.Exit(1)
	}

	db := openDuckDB()
	defer db.Close()

	configureDuckDBS3(db, cfg)
	http.HandleFunc("POST /v1/query", queryHandler(db, cfg))

	slog.Info("query-server listening", "addr", cfg.ListenAddr)

	if err := http.ListenAndServe(cfg.ListenAddr, nil); err != nil {
		slog.Error("server exited", "error", err)
		os.Exit(1)
	}
}

// FRAGILE: single in-memory DuckDB connection shared across all HTTP
// requests. DuckDB is not designed for concurrent writers through database/sql;
// concurrent queries may serialize or error depending on the driver version.
// No connection pool, no query timeout, no memory limit.
func openDuckDB() *sql.DB {
	db, err := sql.Open("duckdb", "")
	if err != nil {
		slog.Error("open duckdb", "error", err)
		os.Exit(1)
	}

	return db
}

func configureDuckDBS3(db *sql.DB, cfg Config) {
	mustExec(db, "INSTALL httpfs; LOAD httpfs")

	// FRAGILE: env var values are string-concatenated into SQL. If any env
	// var contains a single quote, this either breaks the SET command or
	// allows SQL injection into the DuckDB session. These come from env
	// vars (not user input) today, but this is still a ticking time bomb.
	if cfg.S3Region != "" {
		mustExec(db, "SET s3_region='"+cfg.S3Region+"'")
	}

	if cfg.S3AccessKey != "" {
		mustExec(db, "SET s3_access_key_id='"+cfg.S3AccessKey+"'")
	}

	if cfg.S3SecretKey != "" {
		mustExec(db, "SET s3_secret_access_key='"+cfg.S3SecretKey+"'")
	}

	if cfg.S3Endpoint != "" {
		mustExec(db, "SET s3_endpoint='"+cfg.S3Endpoint+"'")
		mustExec(db, "SET s3_use_ssl=false")
		mustExec(db, "SET s3_url_style='path'")
	}
}

type queryRequest struct {
	Service string `json:"service"`
	From    string `json:"from"`
	To      string `json:"to"`
	TraceID string `json:"trace_id"`
}

func queryHandler(db *sql.DB, cfg Config) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		// FRAGILE: no request body size limit — an attacker can send a
		// multi-GB JSON body and OOM the server.
		var req queryRequest
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}

		fromT, toT, table, err := resolveQueryWindowAndTable(cfg.S3Bucket, req)
		if err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}

		block, err := query.Investigate(r.Context(), db, table, query.InvestigateRequest{
			Service: req.Service,
			From:    fromT,
			To:      toT,
			TraceID: req.TraceID,
		})
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}

		w.Header().Set("Content-Type", "application/json")

		if err := json.NewEncoder(w).Encode(block); err != nil {
			slog.Error("write response", "error", err)
		}
	}
}

func resolveQueryWindowAndTable(bucket string, req queryRequest) (time.Time, time.Time, string, error) {
	if req.TraceID != "" {
		// Trace lookup: scan all partitions.
		// FRAGILE: scans EVERY parquet file ever written (year 2000 to now).
		// This is an unbounded full-table scan that will get slower over time
		// and could time out or OOM on a bucket with months of data.
		// Also, bucket is string-concatenated into the glob — SQL injection
		// if bucket ever comes from user input.
		fromT := time.Date(2000, 1, 1, 0, 0, 0, 0, time.UTC)
		toT := time.Now().UTC()

		return fromT, toT, "read_parquet('s3://" + bucket + "/**/*.parquet')", nil
	}

	if req.Service == "" || req.From == "" || req.To == "" {
		return time.Time{}, time.Time{}, "", fmt.Errorf("service, from, and to are required")
	}

	fromT, err := time.Parse(time.RFC3339, req.From)
	if err != nil {
		return time.Time{}, time.Time{}, "", fmt.Errorf("invalid from: %w", err)
	}

	toT, err := time.Parse(time.RFC3339, req.To)
	if err != nil {
		return time.Time{}, time.Time{}, "", fmt.Errorf("invalid to: %w", err)
	}

	return fromT, toT, query.ParquetTable(bucket, fromT, toT), nil
}

func mustExec(db *sql.DB, q string) {
	if _, err := db.Exec(q); err != nil {
		slog.Error("exec failed", "query", q, "error", err)
		os.Exit(1)
	}
}
