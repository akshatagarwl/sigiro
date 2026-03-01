package main

import (
	"database/sql"
	"encoding/json"
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
	S3Bucket    string `env:"S3_BUCKET"`
	ListenAddr  string `env:"LISTEN_ADDR"`
}

func main() {
	cfg, err := env.ParseAs[Config]()
	if err != nil {
		slog.Error("parse config", "error", err)
		os.Exit(1)
	}

	db, err := sql.Open("duckdb", "")
	if err != nil {
		slog.Error("open duckdb", "error", err)
		os.Exit(1)
	}
	defer db.Close()

	// Configure S3/httpfs
	mustExec(db, "INSTALL httpfs; LOAD httpfs")
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

	http.HandleFunc("POST /v1/query", func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Service string `json:"service"`
			From    string `json:"from"`
			To      string `json:"to"`
			TraceID string `json:"trace_id"`
		}
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}

		var fromT, toT time.Time
		var table string
		if req.TraceID != "" {
			// Trace lookup: scan all partitions
			fromT = time.Date(2000, 1, 1, 0, 0, 0, 0, time.UTC)
			toT = time.Now().UTC()
			table = "read_parquet('s3://" + cfg.S3Bucket + "/**/*.parquet')"
		}
		if req.TraceID == "" {
			if req.Service == "" || req.From == "" || req.To == "" {
				http.Error(w, "service, from, and to are required", http.StatusBadRequest)
				return
			}
			var err error
			fromT, err = time.Parse(time.RFC3339, req.From)
			if err != nil {
				http.Error(w, "invalid from: "+err.Error(), http.StatusBadRequest)
				return
			}
			toT, err = time.Parse(time.RFC3339, req.To)
			if err != nil {
				http.Error(w, "invalid to: "+err.Error(), http.StatusBadRequest)
				return
			}
			table = query.ParquetTable(cfg.S3Bucket, fromT, toT)
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
		json.NewEncoder(w).Encode(block)
	})

	slog.Info("query-server listening", "addr", cfg.ListenAddr)
	if err := http.ListenAndServe(cfg.ListenAddr, nil); err != nil {
		slog.Error("server exited", "error", err)
		os.Exit(1)
	}
}

func mustExec(db *sql.DB, q string) {
	if _, err := db.Exec(q); err != nil {
		slog.Error("exec failed", "query", q, "error", err)
		os.Exit(1)
	}
}
