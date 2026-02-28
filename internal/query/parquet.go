package query

import (
	"fmt"
	"time"
)

// ParquetGlob generates the S3 glob pattern covering all hour partitions
// between from and to. For single-hour windows it returns a specific path;
// for multi-hour windows it uses hive_partitioning.
func ParquetGlob(bucket string, from, to time.Time) string {
	fromH := from.Truncate(time.Hour)
	toH := to.Truncate(time.Hour)

	if fromH.Equal(toH) {
		return fmt.Sprintf("s3://%s/year=%04d/month=%02d/day=%02d/hour=%02d/*.parquet",
			bucket, fromH.Year(), fromH.Month(), fromH.Day(), fromH.Hour())
	}

	// Multi-hour: use hive partitioning glob
	if fromH.Year() == toH.Year() && fromH.Month() == toH.Month() && fromH.Day() == toH.Day() {
		return fmt.Sprintf("s3://%s/year=%04d/month=%02d/day=%02d/hour=*/*.parquet",
			bucket, fromH.Year(), fromH.Month(), fromH.Day())
	}

	// Multi-day: broad glob with hive partitioning (DuckDB will prune)
	return fmt.Sprintf("s3://%s/**/*.parquet", bucket)
}
