//! DRAIN — log template extraction via fixed-depth tree clustering.
//!
//! Wrapper around the `drain3` crate that adds pre-masking of common variable
//! patterns (UUIDs, IPs, numbers, hex) before tokenization to reduce noise and
//! prevent high-cardinality tokens from creating unique clusters.
//!
//! Widely used for production log clustering. Reference:
//! "Drain: An Online Log Parsing Approach with Fixed Depth Tree" (2017).

use itertools::Itertools;
use regex::Regex;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::LazyLock;

/// Pre-compiled regexes for token masking.
static MASK_UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
        .unwrap()
});
static MASK_IPV4: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").unwrap());
static MASK_IPV6: LazyLock<Regex> = LazyLock::new(|| {
    // Simplified IPv6 — matches compressed and full forms
    Regex::new(r"[0-9a-fA-F:]{3,}:[0-9a-fA-F:.]+").unwrap()
});
static MASK_HEX: LazyLock<Regex> = LazyLock::new(|| {
    // Hex strings of 8+ chars (trace IDs, span IDs, hashes)
    Regex::new(r"\b[0-9a-fA-F]{8,}\b").unwrap()
});
static MASK_NEGATIVE_INT: LazyLock<Regex> = LazyLock::new(|| {
    // Negative integers only (e.g. -1, -42).
    Regex::new(r"-\d+").unwrap()
});
static MASK_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    // Positive numbers: integer, decimal, or scientific notation.
    Regex::new(r"\d+(?:\.\d+)?(?:[eE][+-]?\d+)?").unwrap()
});

/// Returns true if the match at `m` in `haystack` is a standalone number
/// (not part of a larger word like `user_0`).
fn is_standalone_number(haystack: &str, m: &regex::Match) -> bool {
    let start = m.start();
    let prev = start
        .checked_sub(1)
        .and_then(|i| haystack.as_bytes().get(i));
    // A number is standalone if it is not immediately preceded by a letter or
    // underscore (e.g. user_0, test1).  We allow digits and punctuation before
    // the number so that values like latency=42.5ms still get masked.
    !prev.is_some_and(|&b| b.is_ascii_alphabetic() || b == b'_')
}

fn mask_numbers_in(input: &str) -> String {
    // Two-pass masking:
    // 1. Negative integers (e.g. code=-1 → code=<NUM>).
    // 2. All other numbers (int, float, scientific).
    // This prevents "thread-0" from being rejected as a negative number
    // (because '-' is preceded by 'd') while still masking the '0'.
    let mut result = String::with_capacity(input.len());
    let mut last_end = 0;
    for m in MASK_NEGATIVE_INT.find_iter(input) {
        if is_standalone_number(input, &m) {
            result.push_str(&input[last_end..m.start()]);
            result.push_str("<NUM>");
        } else {
            result.push_str(&input[last_end..m.end()]);
        }
        last_end = m.end();
    }
    result.push_str(&input[last_end..]);

    let pass1 = result;
    result = String::with_capacity(pass1.len());
    last_end = 0;
    for m in MASK_NUMBER.find_iter(&pass1) {
        if is_standalone_number(&pass1, &m) {
            result.push_str(&pass1[last_end..m.start()]);
            result.push_str("<NUM>");
        } else {
            result.push_str(&pass1[last_end..m.end()]);
        }
        last_end = m.end();
    }
    result.push_str(&pass1[last_end..]);
    result
}

/// A DRAIN tree instance backed by `drain3::Matcher`.
pub struct DrainTree {
    matcher: drain3::Matcher,
}

impl Default for DrainTree {
    fn default() -> Self {
        Self::new()
    }
}

impl DrainTree {
    pub fn new() -> Self {
        let cfg = drain3::Config::default();
        Self {
            matcher: drain3::Matcher::new(cfg),
        }
    }

    /// Mask common variable patterns in a raw log line before tokenization.
    ///
    /// Rules (applied in order):
    /// 1. UUIDs → `<UUID>`
    /// 2. IPv4 / IPv6 → `<IP>`
    /// 3. Hex strings (8+ chars) → `<HEX>`
    /// 4. Numbers (int, float, scientific) → `<NUM>`
    pub fn mask_tokens(text: &str) -> String {
        let mut result = text.to_string();
        result = MASK_UUID.replace_all(&result, "<UUID>").to_string();
        result = MASK_IPV4.replace_all(&result, "<IP>").to_string();
        result = MASK_IPV6.replace_all(&result, "<IP>").to_string();
        result = MASK_HEX.replace_all(&result, "<HEX>").to_string();
        result = mask_numbers_in(&result);
        result
    }

    /// Classify a log line and return the template string, template_id (hash), and extracted values.
    /// If no existing cluster matches, creates a new one.
    ///
    /// Extracted values include:
    /// - Original tokens for positions that become `<*>` wildcards via DRAIN merging
    /// - Original tokens for positions that were pre-masked as `<NUM>`, `<IP>`, `<UUID>`, `<HEX>`
    pub fn classify(&mut self, log_body: &str) -> (String, u64, Vec<String>) {
        let trimmed = log_body.trim();
        if trimmed.is_empty() {
            return (String::new(), 0, Vec::new());
        }
        let masked = Self::mask_tokens(trimmed);

        // Train/update the matcher with the masked line.
        if let Err(e) = self.matcher.add_log_message(&masked) {
            tracing::debug!("drain3 training failed for line: {e}");
        }

        // Match to get the template id and drain3-extracted args.
        let mut args = Vec::new();
        let (template_id, matched) = self.matcher.match_into(&masked, &mut args);

        // Build template string from the matched cluster.
        let template = if matched {
            if let Some(t) = self.matcher.template_for_id(template_id) {
                // Reconstruct full token sequence including params.
                let mut dense = t.tokens().iter();
                (0..t.token_count())
                    .map(|i| {
                        if t.is_param(i) {
                            "<*>".to_string()
                        } else {
                            dense
                                .next()
                                .map(|s| s.as_ref().to_string())
                                .unwrap_or_else(|| "<*>".to_string())
                        }
                    })
                    .join(" ")
            } else {
                masked.clone()
            }
        } else {
            masked.clone()
        };

        let id = hash_template(&template);

        // Collect original values for pre-masked placeholders.
        let masked_tokens: Vec<&str> = masked.split_whitespace().collect();
        let original_tokens: Vec<&str> = log_body.split_whitespace().collect();
        let mut values = args;
        values.extend(
            itertools::zip_eq(&masked_tokens, &original_tokens)
                .filter(|(m, _)| is_pre_masked(m))
                .map(|(_, o)| o.to_string()),
        );

        (template, id, values)
    }
}

/// Returns true if a token contains a pre-masked placeholder (not drain3's `<*>`).
fn is_pre_masked(token: &str) -> bool {
    token.contains("<NUM>")
        || token.contains("<IP>")
        || token.contains("<UUID>")
        || token.contains("<HEX>")
}

/// Stable hash of a template string → u64 template_id.
fn hash_template(template: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    template.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_uuid() {
        let input = "request c4430d44-63de-cc80-3423-60111727a438 completed";
        assert_eq!(DrainTree::mask_tokens(input), "request <UUID> completed");
    }

    #[test]
    fn mask_ipv4() {
        let input = "connection from 192.168.1.42:8080 timeout";
        assert_eq!(
            DrainTree::mask_tokens(input),
            "connection from <IP>:<NUM> timeout"
        );
    }

    #[test]
    fn mask_hex() {
        let input = "trace_id=deadbeef12345678 span_id=abcd1234";
        assert_eq!(
            DrainTree::mask_tokens(input),
            "trace_id=<HEX> span_id=<HEX>"
        );
    }

    #[test]
    fn mask_numbers() {
        let input = "latency=42.5ms retries=3 code=-1 size=1.2e3";
        assert_eq!(
            DrainTree::mask_tokens(input),
            "latency=<NUM>ms retries=<NUM> code=<NUM> size=<NUM>"
        );
        // Numbers inside identifiers should NOT be masked.
        assert_eq!(DrainTree::mask_tokens("user_0"), "user_0");
        // Numbers after hyphens SHOULD be masked (common in log tokens).
        assert_eq!(DrainTree::mask_tokens("thread-0"), "thread-<NUM>");
    }

    #[test]
    fn classify_with_masking() {
        let mut tree = DrainTree::new();
        let (t1, id1, _) = tree.classify("connection from 10.0.0.1:8080 timeout after 5000ms");
        let (t2, id2, _) = tree.classify("connection from 10.0.0.2:9090 timeout after 3000ms");
        // Same mask pattern should produce same template
        assert_eq!(t1, t2);
        assert_eq!(id1, id2);
        assert!(t1.contains("<IP>"));
        assert!(t1.contains("<NUM>"));
    }

    #[test]
    fn classify_different_structure() {
        let mut tree = DrainTree::new();
        let (t1, _, _) = tree.classify("GET /api/users <NUM>");
        let (t2, _, _) = tree.classify("POST /api/items <NUM>");
        // Different HTTP methods and paths → different templates
        assert_ne!(t1, t2);
    }

    #[test]
    fn classify_extracts_values() {
        let mut tree = DrainTree::new();
        let (_, _, v1) = tree.classify("connection from 10.0.0.1 timeout after 5000ms");
        let (_, _, v2) = tree.classify("connection from 10.0.0.2 timeout after 3000ms");
        // First call has no prior cluster, so no drain3-extracted args yet.
        // But pre-masked values are always extracted.
        assert!(!v1.is_empty(), "values should not be empty");
        assert!(!v2.is_empty(), "values should not be empty");
    }
}
