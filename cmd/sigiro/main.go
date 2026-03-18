package main

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"time"

	_ "github.com/duckdb/duckdb-go/v2"
)

func main() {
	if len(os.Args) < 2 {
		printUsage()
		os.Exit(1)
	}

	switch os.Args[1] {
	case "investigate":
		runInvestigate(os.Args[2:])
	case "query":
		runQuery(os.Args[2:])
	case "schema":
		runSchema()
	case "--help", "-h", "help":
		printUsage()
	default:
		exitErr("unknown command", os.Args[1], "run 'sigiro --help' for usage")
	}
}

func printUsage() {
	fmt.Fprint(os.Stderr, `sigiro — observability for agents

Usage:
  sigiro investigate --service NAME --window DURATION
  sigiro query "SELECT ..."
  sigiro schema

Commands:
  investigate   Run a structured investigation (metrics, spans, logs, profiles)
  query         Execute raw DuckDB SQL against the unified table
  schema        Print the column schema (generates agents.txt)

Environment:
  SIGIRO_BUCKET           S3 bucket name (required)
  SIGIRO_S3_ENDPOINT      S3 endpoint for MinIO/dev (optional)
  AWS_ACCESS_KEY_ID       S3 credentials (or ~/.aws/credentials)
  AWS_SECRET_ACCESS_KEY
  AWS_REGION              defaults to us-east-1
`)
}

// exitErr writes a structured JSON error to stdout and exits 1.
func exitErr(errMsg, detail, hint string) {
	e := map[string]string{"error": errMsg}
	if detail != "" {
		e["detail"] = detail
	}

	if hint != "" {
		e["hint"] = hint
	}

	json.NewEncoder(os.Stdout).Encode(e)
	os.Exit(1)
}

// openDB creates a DuckDB connection with S3 credentials loaded from env.
func openDB() *sql.DB {
	db, err := sql.Open("duckdb", "")
	if err != nil {
		exitErr("duckdb open failed", err.Error(), "")
	}

	stmts := []string{
		"INSTALL httpfs",
		"LOAD httpfs",
	}

	region := os.Getenv("AWS_REGION")
	if region == "" {
		region = "us-east-1"
	}

	stmts = append(stmts, fmt.Sprintf("SET s3_region = '%s'", region))

	if key := os.Getenv("AWS_ACCESS_KEY_ID"); key != "" {
		stmts = append(stmts, fmt.Sprintf("SET s3_access_key_id = '%s'", key))
		stmts = append(stmts, fmt.Sprintf("SET s3_secret_access_key = '%s'", os.Getenv("AWS_SECRET_ACCESS_KEY")))
	}

	if ep := os.Getenv("SIGIRO_S3_ENDPOINT"); ep != "" {
		stmts = append(stmts, fmt.Sprintf("SET s3_endpoint = '%s'", ep))
		stmts = append(stmts, "SET s3_use_ssl = false")
		stmts = append(stmts, "SET s3_url_style = 'path'")
	}

	for _, s := range stmts {
		if _, err := db.Exec(s); err != nil {
			exitErr("duckdb setup failed", err.Error(), "check AWS credentials and SIGIRO_S3_ENDPOINT")
		}
	}

	return db
}

func requireBucket() string {
	b := os.Getenv("SIGIRO_BUCKET")
	if b == "" {
		exitErr("missing SIGIRO_BUCKET", "", "set SIGIRO_BUCKET to the S3 bucket name")
	}

	return b
}

func parseWindow(s string) time.Duration {
	s = strings.TrimSpace(s)

	d, err := time.ParseDuration(s)
	if err != nil {
		exitErr("invalid window", s, "use Go duration format: 15m, 1h, 30m")
	}

	return d
}
