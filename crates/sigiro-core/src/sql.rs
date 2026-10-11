//! SQL string helpers. The read-only SQL firewall lives in
//! `sigiro_segments::read_only_sql`, shared with the Workers.

/// Escape a string for safe use inside a SQL single-quoted string literal.
pub fn escape_sql_string(s: &str) -> String {
    s.replace('\'', "''")
}
