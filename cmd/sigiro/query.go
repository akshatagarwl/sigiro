package main

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/akshatagarwl/sigiro/internal/query"
)

func runQuery(args []string) {
	if len(args) == 0 {
		exitErr("missing SQL", "", `sigiro query "SELECT count(*) FROM obs WHERE signal='span'"`)
	}

	rawSQL := strings.Join(args, " ")
	bucket := requireBucket()

	db := openDB()
	defer db.Close()

	// Create the obs view covering the last hour by default.
	// Users can override by querying read_parquet() directly.
	now := time.Now().UTC()
	from := now.Add(-1 * time.Hour)
	table := query.ParquetTable(bucket, from, now, "")

	conn, err := db.Conn(context.Background())
	if err != nil {
		exitErr("connection failed", err.Error(), "")
	}
	defer conn.Close()

	_, err = conn.ExecContext(context.Background(), fmt.Sprintf("CREATE OR REPLACE TEMP VIEW obs AS SELECT * FROM %s", table))
	if err != nil {
		exitErr("failed to create obs view", err.Error(), "check S3 credentials and bucket")
	}

	rows, err := conn.QueryContext(context.Background(), rawSQL)
	if err != nil {
		exitErr("query failed", err.Error(), "check column names with: sigiro schema")
	}
	defer rows.Close()

	cols, err := rows.Columns()
	if err != nil {
		exitErr("columns failed", err.Error(), "")
	}

	var results []map[string]any

	for rows.Next() {
		vals := make([]any, len(cols))

		ptrs := make([]any, len(cols))
		for i := range vals {
			ptrs[i] = &vals[i]
		}

		if err := rows.Scan(ptrs...); err != nil {
			exitErr("scan failed", err.Error(), "")
		}

		row := make(map[string]any, len(cols))
		for i, col := range cols {
			row[col] = deref(vals[i])
		}

		results = append(results, row)
	}

	if err := rows.Err(); err != nil {
		exitErr("row iteration failed", err.Error(), "")
	}

	if results == nil {
		results = []map[string]any{}
	}

	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	enc.Encode(results)
}

// deref converts sql driver types to JSON-friendly values.
func deref(v any) any {
	switch t := v.(type) {
	case []byte:
		return string(t)
	case sql.NullString:
		if t.Valid {
			return t.String
		}

		return nil
	case sql.NullFloat64:
		if t.Valid {
			return t.Float64
		}

		return nil
	case sql.NullInt64:
		if t.Valid {
			return t.Int64
		}

		return nil
	default:
		return v
	}
}
