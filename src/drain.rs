//! DRAIN — log template extraction via fixed-depth tree clustering.
//!
//! Splits log lines into tokens, clusters by token count and prefix,
//! then replaces variable tokens with `<*>`. O(1) per log line.
//!
//! Widely used for production log clustering. Reference:
//! "Drain: An Online Log Parsing Approach with Fixed Depth Tree" (2017).

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

/// Similarity threshold: fraction of tokens that must match for a log to
/// join an existing cluster. 0.5 = half the tokens must be identical.
const SIMILARITY_THRESHOLD: f64 = 0.5;

/// Maximum clusters per token-count group. Prevents unbounded growth.
const MAX_CLUSTERS_PER_GROUP: usize = 500;

/// A DRAIN tree instance. One per project, lives in memory alongside the flusher.
pub struct DrainTree {
    /// token_count → list of clusters
    groups: HashMap<usize, Vec<Cluster>>,
}

struct Cluster {
    /// Template tokens: either literal strings or `<*>` for variable positions.
    tokens: Vec<String>,
    /// Number of log lines that matched this cluster.
    count: u64,
}

impl Default for DrainTree {
    fn default() -> Self {
        Self::new()
    }
}

impl DrainTree {
    pub fn new() -> Self {
        Self {
            groups: HashMap::new(),
        }
    }

    /// Classify a log line and return the template string + template_id (hash).
    /// If no existing cluster matches, creates a new one.
    pub fn classify(&mut self, log_body: &str) -> (String, u64) {
        let tokens: Vec<&str> = log_body.split_whitespace().collect();
        if tokens.is_empty() {
            return (String::new(), 0);
        }

        let token_count = tokens.len();
        let group = self.groups.entry(token_count).or_default();

        // Find best matching cluster
        let mut best_idx = None;
        let mut best_sim = 0.0_f64;

        for (i, cluster) in group.iter().enumerate() {
            let sim = similarity(&tokens, &cluster.tokens);
            if sim > best_sim && sim >= SIMILARITY_THRESHOLD {
                best_sim = sim;
                best_idx = Some(i);
            }
        }

        if let Some(idx) = best_idx {
            // Update existing cluster: merge tokens, replacing differing positions with <*>
            let cluster = &mut group[idx];
            for (j, token) in tokens.iter().enumerate() {
                if cluster.tokens[j] != "<*>" && cluster.tokens[j] != *token {
                    cluster.tokens[j] = "<*>".to_string();
                }
            }
            cluster.count += 1;
            let template = cluster.tokens.join(" ");
            let id = hash_template(&template);
            (template, id)
        } else {
            // Create new cluster (if under limit)
            let template_tokens: Vec<String> = tokens.iter().map(|t| (*t).to_string()).collect();
            let template = template_tokens.join(" ");
            let id = hash_template(&template);

            if group.len() < MAX_CLUSTERS_PER_GROUP {
                group.push(Cluster {
                    tokens: template_tokens,
                    count: 1,
                });
            }
            // If at limit, still return the template but don't create a cluster.
            // This log line won't contribute to future matching but we still classify it.
            (template, id)
        }
    }
}

/// Fraction of tokens that match (identical or wildcard).
fn similarity(tokens: &[&str], cluster_tokens: &[String]) -> f64 {
    if tokens.len() != cluster_tokens.len() {
        return 0.0;
    }
    let matches = tokens
        .iter()
        .zip(cluster_tokens.iter())
        .filter(|(t, c)| c.as_str() == "<*>" || **t == c.as_str())
        .count();
    matches as f64 / tokens.len() as f64
}

/// Stable hash of a template string → u64 template_id.
fn hash_template(template: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    template.hash(&mut hasher);
    hasher.finish()
}
