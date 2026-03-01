package query

import (
	"fmt"
	"strings"
	"time"
)

// ParquetGlobs returns the S3 glob patterns covering all epoch-hour
// partitions between from and to. Each hour in the range gets its own
// glob: s3://{bucket}/{epochSeconds}/*.parquet
func ParquetGlobs(bucket string, from, to time.Time) []string {
	fromH := from.Truncate(time.Hour).Unix()
	toH := to.Truncate(time.Hour).Unix()

	var globs []string
	for h := fromH; h <= toH; h += 3600 {
		globs = append(globs, fmt.Sprintf("s3://%s/%d/*.parquet", bucket, h))
	}

	return globs
}

// ParquetTable returns a DuckDB read_parquet expression for the given
// bucket and time range.
func ParquetTable(bucket string, from, to time.Time) string {
	globs := ParquetGlobs(bucket, from, to)
	if len(globs) == 1 {
		return fmt.Sprintf("read_parquet('%s')", globs[0])
	}

	quoted := make([]string, len(globs))
	for i, g := range globs {
		quoted[i] = fmt.Sprintf("'%s'", g)
	}

	return fmt.Sprintf("read_parquet([%s])", strings.Join(quoted, ", "))
}
