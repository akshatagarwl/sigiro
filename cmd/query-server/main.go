package main

import (
	"database/sql"
	"encoding/json"
	"log"
	"net/http"
	"os"
	"time"

	"github.com/akshatagarwl/observe/internal/query"

	_ "github.com/duckdb/duckdb-go/v2"
)

func main() {
	db, err := sql.Open("duckdb", "")
	if err != nil {
		log.Fatal(err)
	}
	defer db.Close()

	// Configure S3/httpfs
	mustExec(db, "INSTALL httpfs; LOAD httpfs")
	if region := os.Getenv("S3_REGION"); region != "" {
		mustExec(db, "SET s3_region='"+region+"'")
	}
	if key := os.Getenv("S3_ACCESS_KEY"); key != "" {
		mustExec(db, "SET s3_access_key_id='"+key+"'")
	}
	if secret := os.Getenv("S3_SECRET_KEY"); secret != "" {
		mustExec(db, "SET s3_secret_access_key='"+secret+"'")
	}
	if ep := os.Getenv("S3_ENDPOINT"); ep != "" {
		mustExec(db, "SET s3_endpoint='"+ep+"'")
		mustExec(db, "SET s3_use_ssl=false")
		mustExec(db, "SET s3_url_style='path'")
	}

	bucket := os.Getenv("S3_BUCKET")
	if bucket == "" {
		bucket = "observe"
	}

	addr := os.Getenv("LISTEN_ADDR")
	if addr == "" {
		addr = ":8080"
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
			table = "read_parquet('s3://" + bucket + "/**/*.parquet')"
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
			table = query.ParquetTable(bucket, fromT, toT)
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

	log.Printf("query-server listening on %s", addr)
	log.Fatal(http.ListenAndServe(addr, nil))
}

func mustExec(db *sql.DB, q string) {
	if _, err := db.Exec(q); err != nil {
		log.Fatalf("exec %q: %v", q, err)
	}
}
