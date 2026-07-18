//! DRAIN — log template extraction via fixed-depth tree clustering.
//!
//! Thin wrapper over the `drain3` crate. Masking of high-cardinality values
//! (UUIDs, IPs, hex, numbers) into placeholders happens inside drain3 via
//! `Config.masking` before tokenization, so identical-shaped logs collapse to a
//! single template instead of spawning a distinct cluster per value.
//!
//! Widely used for production log clustering. Reference:
//! "Drain: An Online Log Parsing Approach with Fixed Depth Tree" (2017).

use itertools::Itertools;
use std::hash::{DefaultHasher, Hash, Hasher};

/// Masking rules applied (in order) inside drain3 before tokenization. Order is
/// load-bearing: specific patterns run before the generic number rule so e.g.
/// `10.0.0.1` masks as `<IP>`, and an 8+-char hex id as `<HEX>`, before the
/// number rule could touch their digits.
fn masking_rules() -> Vec<drain3::MaskingInstruction> {
    let rule = |pattern: &str, mask: &str| drain3::MaskingInstruction {
        pattern: pattern.to_string(),
        mask: mask.to_string(),
    };
    vec![
        rule(
            r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
            "<UUID>",
        ),
        rule(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b", "<IP>"),
        // Simplified IPv6 — compressed and full forms.
        rule(r"[0-9a-fA-F:]{3,}:[0-9a-fA-F:.]+", "<IP>"),
        // Hex ids of 8+ chars (trace/span ids, hashes).
        rule(r"\b[0-9a-fA-F]{8,}\b", "<HEX>"),
        // Int / decimal / scientific. The `(^|[^\w])` prefix plus `${1}` in the
        // mask re-emit the boundary char, so a digit inside an identifier
        // (`user_0`) is left alone while a punctuation-led or standalone number
        // is masked. A leading `-` is absorbed when it starts the number
        // (`code=-1` → `code=<NUM>`) but kept when it joins a word
        // (`thread-0` → `thread-<NUM>`). Needs drain3 >= 0.3 (mask expansion).
        rule(r"(^|[^\w])(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)", "${1}<NUM>"),
    ]
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
        let cfg = drain3::Config {
            masking: masking_rules(),
            ..Default::default()
        };
        Self {
            matcher: drain3::Matcher::new(cfg),
        }
    }

    /// Classify a log line and return `(template, template_id, values)`.
    ///
    /// drain3 masks high-cardinality values into placeholders and clusters the
    /// result. `values` holds the ORIGINAL whitespace-delimited token at every
    /// variable position of the template — both masking placeholders
    /// (`<NUM>`, `<IP>`, `<UUID>`, `<HEX>`) and drain3 `<*>` wildcards — in
    /// order, so the source line can be reconstructed from `template` + `values`.
    pub fn classify(&mut self, log_body: &str) -> (String, u64, Vec<String>) {
        let trimmed = log_body.trim();
        if trimmed.is_empty() {
            return (String::new(), 0, Vec::new());
        }

        // drain3 masks the line (see `masking_rules()`) and returns the template
        // of the cluster it trained into — one masking pass, no separate match.
        let template = match self.matcher.add_log_message(trimmed) {
            Ok(t) => {
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
            }
            Err(e) => {
                tracing::debug!("drain3 training failed for line: {e}");
                trimmed.to_string()
            }
        };

        let id = hash_template(&template);

        // Recover the original values behind every variable position by aligning
        // the raw tokens with the template tokens. drain3 tokenizes on
        // whitespace, so token counts match; the guard keeps a surprising
        // mismatch from panicking (values are simply dropped for that line).
        let template_tokens: Vec<&str> = template.split(' ').collect();
        let orig_tokens: Vec<&str> = trimmed.split_whitespace().collect();
        let values = if template_tokens.len() == orig_tokens.len() {
            itertools::zip_eq(template_tokens, orig_tokens)
                .filter(|(t, _)| is_variable(t))
                .map(|(_, o)| o.to_string())
                .collect()
        } else {
            Vec::new()
        };

        (template, id, values)
    }
}

/// True if a template token is a variable position: a drain3 wildcard (`<*>`)
/// or a token carrying one of our masking placeholders.
fn is_variable(token: &str) -> bool {
    token == "<*>"
        || token.contains("<NUM>")
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

    // Masking now happens inside drain3, so it is exercised through `classify`:
    // a single line has no diverging tokens, so its template is the fully masked
    // line.

    #[test]
    fn masks_uuid() {
        let mut tree = DrainTree::new();
        let (t, _, _) = tree.classify("request c4430d44-63de-cc80-3423-60111727a438 completed");
        assert_eq!(t, "request <UUID> completed");
    }

    #[test]
    fn masks_ipv4_and_port() {
        let mut tree = DrainTree::new();
        let (t, _, _) = tree.classify("connection from 192.168.1.42:8080 timeout");
        assert_eq!(t, "connection from <IP>:<NUM> timeout");
    }

    #[test]
    fn masks_hex() {
        let mut tree = DrainTree::new();
        let (t, _, _) = tree.classify("trace_id=deadbeef12345678 span_id=abcd1234");
        assert_eq!(t, "trace_id=<HEX> span_id=<HEX>");
    }

    #[test]
    fn masks_numbers_but_not_identifiers() {
        assert_eq!(
            DrainTree::new()
                .classify("latency=42.5ms retries=3 size=1.2e3")
                .0,
            "latency=<NUM>ms retries=<NUM> size=<NUM>"
        );
        // A digit inside an identifier is left alone.
        assert_eq!(DrainTree::new().classify("user_0").0, "user_0");
        // A digit led by a hyphen is masked; the hyphen stays a constant.
        assert_eq!(DrainTree::new().classify("thread-0").0, "thread-<NUM>");
    }

    #[test]
    fn classify_groups_same_shape() {
        let mut tree = DrainTree::new();
        let (t1, id1, _) = tree.classify("connection from 10.0.0.1:8080 timeout after 5000ms");
        let (t2, id2, _) = tree.classify("connection from 10.0.0.2:9090 timeout after 3000ms");
        assert_eq!(t1, t2);
        assert_eq!(id1, id2);
        assert!(t1.contains("<IP>"));
        assert!(t1.contains("<NUM>"));
    }

    #[test]
    fn classify_different_structure() {
        let mut tree = DrainTree::new();
        let (t1, _, _) = tree.classify("GET /api/users 42");
        let (t2, _, _) = tree.classify("POST /api/items 99");
        assert_ne!(t1, t2);
    }

    #[test]
    fn classify_recovers_original_values() {
        let mut tree = DrainTree::new();
        let (_, _, v) = tree.classify("connection from 10.0.0.1 timeout after 5000ms");
        // The IP and the numeric duration are recovered as their raw tokens, so
        // the source line can be rebuilt from template + values.
        assert_eq!(v, vec!["10.0.0.1".to_string(), "5000ms".to_string()]);
    }
}
