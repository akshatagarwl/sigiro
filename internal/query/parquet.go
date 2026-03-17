package query

import (
	"fmt"
	"strings"
	"time"
)

// ParquetGlobs returns the S3 glob patterns covering all epoch-hour
// partitions between from and to. Each hour in the range gets its own
// glob. The signal parameter filters to a specific signal partition
// (e.g. "metric"); empty string means all signals.
//
// Layout: s3://{bucket}/{epochSeconds}/signal={signal}/{node}-{ts}.parquet
//
// FRAGILE: generates one glob per hour — a 30-day query produces 720 globs.
// DuckDB must open and scan each glob independently, which can be very slow
// or hit S3 rate limits. No upper bound on the time range means a careless
// caller can generate thousands of globs.
func ParquetGlobs(bucket string, from, to time.Time, signal string) []string {
	fromH := from.Truncate(time.Hour).Unix()
	toH := to.Truncate(time.Hour).Unix()

	signalPart := "signal=*"
	if signal != "" {
		signalPart = "signal=" + signal
	}

	var globs []string
	for h := fromH; h <= toH; h += 3600 {
		globs = append(globs, fmt.Sprintf("s3://%s/%d/%s/*.parquet", bucket, h, signalPart))
	}

	return globs
}

// ParquetTable returns a DuckDB read_parquet expression for the given
// bucket and time range. Signal filters to a specific signal partition;
// empty string means all signals (with hive_partitioning so DuckDB
// auto-populates the signal column from the path).
func ParquetTable(bucket string, from, to time.Time, signal string) string {
	globs := ParquetGlobs(bucket, from, to, signal)

	// All-signals queries need hive_partitioning=true so DuckDB reads
	// the signal column from the directory name instead of requiring it
	// in the Parquet file (each file only contains one signal's rows,
	// but the signal column is still in the Parquet schema, so this is
	// belt-and-suspenders — it works either way).
	hiveOpt := ""
	if signal == "" {
		hiveOpt = ", hive_partitioning=true"
	}

	if len(globs) == 1 {
		return fmt.Sprintf("read_parquet('%s'%s)", globs[0], hiveOpt)
	}

	quoted := make([]string, len(globs))
	for i, g := range globs {
		quoted[i] = fmt.Sprintf("'%s'", g)
	}

	return fmt.Sprintf("read_parquet([%s]%s)", strings.Join(quoted, ", "), hiveOpt)
}
