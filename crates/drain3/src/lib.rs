//! drain3 — fast log template extraction via fixed-depth prefix trees.
//!
//! Rust port of Axiom's drain3 (Go). Splits log lines into tokens,
//! clusters them by a prefix tree keyed on token count, and replaces
//! variable tokens with a param placeholder (`<*>`` by default).
//!
//! # Example
//! ```
//! use drain3::Config;
//!
//! let samples: Vec<String> = vec![
//!     "connection from 10.0.0.1 timeout after 5000ms".into(),
//!     "connection from 10.0.0.2 timeout after 3000ms".into(),
//!     "connection from 10.0.0.3 timeout after 1000ms".into(),
//! ];
//! let matcher = drain3::train(&samples, Config::default()).unwrap();
//! let (id, args, ok) = matcher.match_line("connection from 192.168.1.1 timeout after 42ms");
//! assert!(ok);
//! assert_eq!(args, vec!["192.168.1.1", "42ms"]);
//! ```
use std::cell::RefCell;
use std::collections::HashMap;
mod dict;
mod prefilter;
// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------
/// Controls training and matching behavior.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Prefix tree depth. Must be >= 3.
    pub depth: usize,
    /// Fraction of tokens that must match for a line to join a cluster during training.
    pub similarity_threshold: f64,
    /// Fraction of tokens that must match for a line to be considered a match.
    pub match_threshold: f64,
    /// Max children per inner node. One slot is reserved for the param catch-all.
    pub max_children: usize,
    /// Max tokens per line. Lines with more tokens are skipped.
    pub max_tokens: usize,
    /// Max bytes per line. Lines longer than this are skipped.
    pub max_bytes: usize,
    /// Max clusters. 0 = unlimited.
    pub max_clusters: usize,
    /// Placeholder string for param tokens (default: `<*>`).
    pub param_string: String,
    /// Whether numeric tokens are automatically parameterized during tree insertion.
    pub parametrize_numeric_tokens: bool,
    /// Additional delimiter strings to replace with spaces before tokenization.
    pub extra_delimiters: Vec<String>,
    /// Enable the first/last token prefilter optimization for matching.
    pub enable_match_prefilter: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            depth: 4,
            similarity_threshold: 0.5,
            match_threshold: 1.0,
            max_children: 100,
            max_tokens: 64,
            max_bytes: 1024,
            max_clusters: 0,
            param_string: "<*>".to_string(),
            parametrize_numeric_tokens: true,
            extra_delimiters: Vec::new(),
            enable_match_prefilter: true,
        }
    }
}
impl Config {
    fn normalize(&self) -> Result<Self, String> {
        if self.depth < 3 {
            return Err(format!("depth must be >= 3, got {}", self.depth));
        }
        if !(0.0..=1.0).contains(&self.similarity_threshold) {
            return Err(format!(
                "similarity threshold must be in [0, 1], got {}",
                self.similarity_threshold
            ));
        }
        if !(0.0..=1.0).contains(&self.match_threshold) {
            return Err(format!(
                "match threshold must be in [0, 1], got {}",
                self.match_threshold
            ));
        }
        if self.max_children < 2 {
            return Err(format!(
                "max children must be >= 2, got {}",
                self.max_children
            ));
        }
        if self.max_tokens < 1 {
            return Err(format!("max tokens must be >= 1, got {}", self.max_tokens));
        }
        if self.max_bytes < 1 {
            return Err(format!("max bytes must be >= 1, got {}", self.max_bytes));
        }
        if self.param_string.is_empty() {
            return Err("param string must not be empty".to_string());
        }
        let mut cfg = self.clone();
        cfg.extra_delimiters.retain(|d| !d.is_empty());
        Ok(cfg)
    }
}
// ---------------------------------------------------------------------------
// Template
// ---------------------------------------------------------------------------
/// A trained log template.
#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    pub id: usize,
    /// Dense token list: only non-param tokens, in order.
    pub tokens: Vec<String>,
    /// Bit i set = position i is a param placeholder.
    pub params: Vec<bool>,
    /// Total number of positions (len(tokens) + param_count).
    pub token_count: usize,
    /// Number of matching log lines.
    pub count: usize,
}
// ---------------------------------------------------------------------------
// Internal: cluster, node
// ---------------------------------------------------------------------------
pub(crate) struct Cluster {
    id: usize,
    size: usize,
    param_count: usize,
    token_str: Vec<String>,
    token_ids: Vec<u64>,
    /// Indices of non-param tokens, for fast scoring.
    non_param_idx: Vec<usize>,
    anchor0: isize,
    anchor1: isize,
}
impl Cluster {
    fn new(id: usize, token_str: Vec<String>, token_ids: Vec<u64>, param_id: u64) -> Self {
        let mut s = Self {
            id,
            size: 1,
            param_count: 0,
            token_str,
            token_ids,
            non_param_idx: Vec::new(),
            anchor0: -1,
            anchor1: -1,
        };
        s.rebuild_non_param_idx(param_id);
        s
    }
    fn rebuild_non_param_idx(&mut self, param_id: u64) {
        self.non_param_idx.clear();
        self.param_count = 0;
        for (i, &tid) in self.token_ids.iter().enumerate() {
            if tid == param_id {
                self.param_count += 1;
            } else {
                self.non_param_idx.push(i);
            }
        }
        match self.non_param_idx.len() {
            0 => {
                self.anchor0 = -1;
                self.anchor1 = -1;
            }
            1 => {
                self.anchor0 = self.non_param_idx[0] as isize;
                self.anchor1 = -1;
            }
            _ => {
                self.anchor0 = self.non_param_idx[0] as isize;
                self.anchor1 = self.non_param_idx[self.non_param_idx.len() - 1] as isize;
            }
        }
    }
    fn extract_args_into(&self, line_tokens: &[String], param_id: u64, dst: &mut Vec<String>) {
        if self.token_ids.is_empty() || line_tokens.is_empty() || self.param_count == 0 {
            return;
        }
        let limit = self.token_ids.len().min(line_tokens.len());
        dst.clear();
        for (i, tok) in line_tokens.iter().enumerate().take(limit) {
            if self.token_ids[i] == param_id {
                dst.push(tok.clone());
            }
        }
    }
}
struct Node {
    children: HashMap<u64, Box<Node>>,
    cluster_ids: Vec<usize>,
}
impl Node {
    fn new() -> Self {
        Self {
            children: HashMap::new(),
            cluster_ids: Vec::new(),
        }
    }
}
// ---------------------------------------------------------------------------
// Matcher
// ---------------------------------------------------------------------------
pub struct Matcher {
    cfg: Config,
    templates: Vec<Template>,
    root_by_len: Vec<Option<Box<Node>>>,
    clusters: Vec<Option<Box<Cluster>>>,
    dict_ids: HashMap<String, u64>,
    dict_next_id: u64,
    param_id: u64,
    next_cluster: usize,
    match_needed: Vec<usize>,
    prefilter_buckets: Vec<prefilter::PrefilterBucket>,
    dict_frozen: Option<dict::FrozenDict>,
    has_param_first: bool,
    scratch_tok: RefCell<Vec<String>>,
}
impl Matcher {
    pub fn new(cfg: Config) -> Self {
        let mut m = Self {
            cfg: cfg.clone(),
            templates: Vec::new(),
            root_by_len: Vec::new(),
            clusters: vec![None], // 0 is sentinel
            dict_ids: HashMap::new(),
            dict_next_id: 1,
            param_id: 0,
            next_cluster: 1,
            match_needed: Vec::new(),
            prefilter_buckets: Vec::new(),
            dict_frozen: None,
            has_param_first: false,
            scratch_tok: RefCell::new(Vec::new()),
        };
        m.param_id = m.intern_token(&cfg.param_string);
        m
    }
    fn freeze_dict(&mut self) {
        let entries: Vec<(String, u64)> =
            self.dict_ids.iter().map(|(k, v)| (k.clone(), *v)).collect();
        self.dict_frozen = Some(dict::FrozenDict::new(entries));
        let mut scratch = self.scratch_tok.borrow_mut();
        if scratch.capacity() < self.cfg.max_tokens {
            *scratch = Vec::with_capacity(self.cfg.max_tokens);
        }
        drop(scratch);
        self.has_param_first = false;
        for c in self.clusters.iter().filter_map(|c| c.as_ref()) {
            if !c.token_ids.is_empty() && c.token_ids[0] == self.param_id {
                self.has_param_first = true;
                break;
            }
        }
    }
    fn resolve_token_id(&self, token: &str) -> u64 {
        if let Some(ref dict) = self.dict_frozen {
            dict.lookup(token).unwrap_or(0)
        } else {
            self.dict_ids.get(token).copied().unwrap_or(0)
        }
    }
    fn intern_token(&mut self, token: &str) -> u64 {
        if let Some(&id) = self.dict_ids.get(token) {
            return id;
        }
        let id = self.dict_next_id;
        self.dict_next_id += 1;
        self.dict_ids.insert(token.to_string(), id);
        id
    }
    fn intern_token_ids(&mut self, tokens: &[String], dst: &mut Vec<u64>) {
        dst.clear();
        dst.reserve(tokens.len());
        for t in tokens {
            dst.push(self.intern_token(t));
        }
    }
    fn required_score(&self, token_count: usize, sim_th: f64) -> usize {
        if (sim_th - self.cfg.match_threshold).abs() < f64::EPSILON
            && token_count < self.match_needed.len()
        {
            return self.match_needed[token_count];
        }
        (sim_th * token_count as f64).ceil() as usize
    }
    fn rebuild_match_needed(&mut self) {
        self.match_needed.resize(self.root_by_len.len(), 0);
        for tc in 0..self.match_needed.len() {
            self.match_needed[tc] = (self.cfg.match_threshold * tc as f64).ceil() as usize;
        }
    }
    // ------------------------------------------------------------------
    // Public API
    // ------------------------------------------------------------------

    /// Match a line and return `(template_id, extracted_args, matched)`.
    pub fn match_line(&self, line: &str) -> (usize, Vec<String>, bool) {
        let mut dst = Vec::new();
        let (id, ok) = self.match_into(line, &mut dst);
        (id, dst, ok)
    }
    /// Match a line, writing extracted args into `dst`.
    /// Returns `(template_id, matched)`.
    pub fn match_into(&self, line: &str, dst: &mut Vec<String>) -> (usize, bool) {
        let (cluster, tokens) = self.find_match(line);
        if let Some(c) = cluster {
            let id = c.id;
            c.extract_args_into(&tokens, self.param_id, dst);
            return (id, true);
        }
        (0, false)
    }
    /// Match a line and return just the template id, or `None` if no match.
    pub fn match_id(&self, line: &str) -> Option<usize> {
        self.find_match(line).0.map(|c| c.id)
    }
    /// Return a deep copy of the matcher's config.
    pub fn config(&self) -> Config {
        self.cfg.clone()
    }
    /// Return trained templates sorted by descending count.
    ///
    /// This is a deep clone — mutations do not affect the matcher.
    pub fn templates(&self) -> Vec<Template> {
        self.templates.clone()
    }
    /// Return a single template by cluster id, or `None` if not found.
    pub fn template_for_id(&self, id: usize) -> Option<Template> {
        let c = self.clusters.get(id)?.as_ref()?;
        let token_count = c.token_ids.len();
        let mut params = vec![false; token_count];
        let mut dense = Vec::with_capacity(token_count - c.param_count);
        for (i, &tid) in c.token_ids.iter().enumerate() {
            if tid == self.param_id {
                params[i] = true;
            } else {
                dense.push(c.token_str[i].clone());
            }
        }
        Some(Template {
            id: c.id,
            tokens: dense,
            params,
            token_count,
            count: c.size,
        })
    }
    // ------------------------------------------------------------------
    // Internal matching
    // ------------------------------------------------------------------
    fn find_match(&self, line: &str) -> (Option<&Cluster>, Vec<String>) {
        if line.len() > self.cfg.max_bytes {
            return (None, Vec::new());
        }
        // Quick rejection: if no cluster has a param at position 0 and there
        // are no extra delimiters, an unknown first token means no match.
        if !self.has_param_first
            && self.cfg.extra_delimiters.is_empty()
            && let Some(ref dict) = self.dict_frozen
            && dict
                .lookup(&line[..line.find(' ').unwrap_or(line.len())])
                .is_none()
        {
            return (None, Vec::new());
        }
        let tokens = if self.cfg.extra_delimiters.is_empty() {
            let mut scratch = self.scratch_tok.borrow_mut();
            scratch.clear();
            let count = tokenize_whitespace_count(line, &mut scratch, self.cfg.max_tokens);
            if count == 0 || count > self.cfg.max_tokens {
                return (None, Vec::new());
            }
            let tc = scratch.len();
            if tc >= self.root_by_len.len() || self.root_by_len[tc].is_none() {
                return (None, scratch.clone());
            }
            scratch.clone()
        } else {
            let t = tokenize(line, &self.cfg.extra_delimiters, self.cfg.max_tokens);
            if t.is_empty() || t.len() > self.cfg.max_tokens {
                return (None, Vec::new());
            }
            t
        };
        let tc = tokens.len();
        if tc >= self.root_by_len.len() || self.root_by_len[tc].is_none() {
            return (None, tokens);
        }
        if self.cfg.enable_match_prefilter && tc < self.prefilter_buckets.len() {
            let mut candidates = Vec::new();
            if let Some(ids) = prefilter::prefilter_candidates_compact(
                &self.prefilter_buckets,
                &self.dict_ids,
                &tokens,
                &mut candidates,
            ) {
                let cluster = self.fast_match_strings(ids, &tokens, self.cfg.match_threshold, true);
                return (cluster, tokens);
            }
            return (None, tokens);
        }
        let cluster = self.tree_search_strings(&tokens);
        (cluster, tokens)
    }
    fn tree_search_strings(&self, tokens: &[String]) -> Option<&Cluster> {
        self.tree_search_with_threshold(tokens, self.cfg.match_threshold, true)
    }
    fn tree_search_train(&self, tokens: &[String]) -> Option<&Cluster> {
        self.tree_search_with_threshold(tokens, self.cfg.similarity_threshold, false)
    }
    fn tree_search_with_threshold(
        &self,
        tokens: &[String],
        threshold: f64,
        include_params: bool,
    ) -> Option<&Cluster> {
        let tc = tokens.len();
        if tc >= self.root_by_len.len() {
            return None;
        }
        let root = self.root_by_len[tc].as_ref()?;
        if tc == 0 {
            return root
                .cluster_ids
                .first()
                .and_then(|&id| self.clusters.get(id).and_then(|c| c.as_ref().map(|b| &**b)));
        }
        let max_depth = self.cfg.depth.saturating_sub(2);
        let mut cur_depth = 1;
        let mut cur_node = root;
        // Use a stack-allocated buffer for small token counts, Vec for larger.
        if tc <= 128 {
            let mut ids = [0u64; 128];
            for (i, tok) in tokens.iter().enumerate() {
                ids[i] = self.resolve_token_id(tok);
            }
            for &tid in ids[..tc].iter() {
                if cur_depth >= max_depth || cur_depth == tc {
                    break;
                }
                let next = cur_node
                    .children
                    .get(&tid)
                    .or_else(|| cur_node.children.get(&self.param_id));
                cur_node = next?;
                cur_depth += 1;
            }
        } else {
            let ids: Vec<u64> = tokens
                .iter()
                .map(|tok| self.resolve_token_id(tok))
                .collect();
            for &tid in ids.iter() {
                if cur_depth >= max_depth || cur_depth == tc {
                    break;
                }
                let next = cur_node
                    .children
                    .get(&tid)
                    .or_else(|| cur_node.children.get(&self.param_id));
                cur_node = next?;
                cur_depth += 1;
            }
        }
        self.fast_match_strings(&cur_node.cluster_ids, tokens, threshold, include_params)
    }
    fn fast_match_strings(
        &self,
        cluster_ids: &[usize],
        tokens: &[String],
        threshold: f64,
        include_params: bool,
    ) -> Option<&Cluster> {
        let n_tokens = tokens.len();
        let needed = self.required_score(n_tokens, threshold);

        // Fast path: at threshold=1.0 with includeParams, return the first
        // perfect match — every non-param token must match exactly.
        if include_params && threshold >= 1.0 {
            'next_candidate: for &cid in cluster_ids {
                let c = match self.clusters.get(cid).and_then(|c| c.as_ref()) {
                    Some(c) => c,
                    None => continue,
                };
                if c.token_str.len() != n_tokens {
                    continue;
                }
                if c.anchor0 >= 0 && c.token_str[c.anchor0 as usize] != tokens[c.anchor0 as usize] {
                    continue;
                }
                if c.anchor1 >= 0 && c.token_str[c.anchor1 as usize] != tokens[c.anchor1 as usize] {
                    continue;
                }
                for &idx in &c.non_param_idx {
                    if c.token_str[idx] != tokens[idx] {
                        continue 'next_candidate;
                    }
                }
                return Some(c);
            }
            return None;
        }

        let mut max_score: isize = -1;
        let mut max_param_count: isize = -1;
        let mut max_cluster: Option<&Cluster> = None;
        for &cid in cluster_ids {
            let c = match self.clusters.get(cid).and_then(|c| c.as_ref()) {
                Some(c) => c,
                None => continue,
            };
            if c.token_str.len() != n_tokens {
                continue;
            }
            let param_count = c.param_count;
            let mut sim_tokens = if include_params { param_count } else { 0 };
            let np_idx = &c.non_param_idx;
            let mut remaining = np_idx.len();

            // Anchor checks: score first and last non-param positions first.
            if c.anchor0 >= 0 {
                if c.token_str[c.anchor0 as usize] == tokens[c.anchor0 as usize] {
                    sim_tokens += 1;
                }
                remaining -= 1;
                if sim_tokens + remaining < needed {
                    continue;
                }
            }
            if c.anchor1 >= 0 && c.anchor1 != c.anchor0 {
                if c.token_str[c.anchor1 as usize] == tokens[c.anchor1 as usize] {
                    sim_tokens += 1;
                }
                remaining -= 1;
                if sim_tokens + remaining < needed {
                    continue;
                }
            }

            for &idx in np_idx {
                let idx = idx as isize;
                if idx == c.anchor0 || idx == c.anchor1 {
                    continue;
                }
                if c.token_str[idx as usize] == tokens[idx as usize] {
                    sim_tokens += 1;
                }
                remaining -= 1;
                if sim_tokens + remaining < needed {
                    break;
                }
            }
            if sim_tokens as isize > max_score
                || (sim_tokens as isize == max_score && param_count as isize > max_param_count)
            {
                max_score = sim_tokens as isize;
                max_param_count = param_count as isize;
                max_cluster = Some(c);
            }
        }
        if max_score >= needed as isize {
            max_cluster
        } else {
            None
        }
    }
    // ------------------------------------------------------------------
    // Training
    // ------------------------------------------------------------------
    pub fn add_log_message(&mut self, content: &str) {
        if content.len() > self.cfg.max_bytes {
            return;
        }
        let tokens = if self.cfg.extra_delimiters.is_empty() {
            let mut scratch = self.scratch_tok.borrow_mut();
            scratch.clear();
            let count = tokenize_whitespace_count(content, &mut scratch, self.cfg.max_tokens);
            if count == 0 || count > self.cfg.max_tokens {
                return;
            }
            scratch.clone()
        } else {
            let t = tokenize(content, &self.cfg.extra_delimiters, self.cfg.max_tokens);
            if t.is_empty() || t.len() > self.cfg.max_tokens {
                return;
            }
            t
        };
        let tc = tokens.len();
        if tc >= self.root_by_len.len() {
            // First log with this token count: create cluster + tree path.
            let id = self.next_cluster;
            self.next_cluster += 1;
            let token_ids: Vec<u64> = tokens.iter().map(|t| self.intern_token(t)).collect();
            let cl = Box::new(Cluster::new(id, tokens, token_ids, self.param_id));
            if id >= self.clusters.len() {
                self.clusters.resize_with(id + 1, || None);
            }
            self.clusters[id] = Some(cl);
            self.add_seq_to_prefix_tree(id);
            return;
        }
        if let Some(c) = self.tree_search_train(&tokens) {
            let cid = c.id;
            let mut changed = false;
            {
                let cluster = self.clusters[cid]
                    .as_mut()
                    .expect("cluster was just found via tree search");
                for (i, tok) in tokens.iter().enumerate().take(cluster.token_str.len()) {
                    if cluster.token_ids[i] == self.param_id {
                        continue;
                    }
                    if cluster.token_str[i] != *tok {
                        cluster.token_ids[i] = self.param_id;
                        cluster.token_str[i] = self.cfg.param_string.clone();
                        cluster.param_count += 1;
                        changed = true;
                    }
                }
                if changed {
                    cluster.rebuild_non_param_idx(self.param_id);
                }
                cluster.size += 1;
            }
            return;
        }
        // No match — create new cluster
        if self.cfg.max_clusters > 0 && self.next_cluster > self.cfg.max_clusters {
            return;
        }
        let mut token_ids = Vec::new();
        self.intern_token_ids(&tokens, &mut token_ids);
        let id = self.next_cluster;
        self.next_cluster += 1;
        let cl = Box::new(Cluster::new(id, tokens, token_ids, self.param_id));
        if id >= self.clusters.len() {
            self.clusters.resize_with(id + 1, || None);
        }
        self.clusters[id] = Some(cl);
        self.add_seq_to_prefix_tree(id);
    }
    fn add_seq_to_prefix_tree(&mut self, cluster_id: usize) {
        let cluster = self.clusters[cluster_id]
            .as_ref()
            .expect("cluster exists by construction");
        let tc = cluster.token_ids.len();
        if tc >= self.root_by_len.len() {
            self.root_by_len.resize_with(tc + 1, || None);
        }
        if self.root_by_len[tc].is_none() {
            self.root_by_len[tc] = Some(Box::new(Node::new()));
        }
        let root = self.root_by_len[tc]
            .as_mut()
            .expect("root was just created if missing");
        if tc == 0 {
            root.cluster_ids.push(cluster_id);
            return;
        }
        let mut cur_ptr: *mut Node = &mut **root;
        let mut cur_depth = 1;
        for (i, &token_id) in cluster.token_ids.iter().enumerate() {
            if cur_depth >= self.cfg.depth - 2 || cur_depth >= tc {
                // SAFETY: cur_ptr is derived from a mutable borrow of a Box<Node>
                // stored in root_by_len, which remains alive for this loop.
                unsafe { (*cur_ptr).cluster_ids.push(cluster_id) };
                break;
            }
            let next_ptr = unsafe {
                // SAFETY: Same as above — cur_ptr points into root_by_len which
                // outlives this loop, and children entries are also Box<Node>
                // allocations owned by the tree.
                if let Some(n) = (*cur_ptr).children.get(&token_id) {
                    // Exact child exists.
                    &mut **(n as *const Box<Node> as *mut Box<Node>)
                } else if self.cfg.parametrize_numeric_tokens && has_numbers(&cluster.token_str[i])
                {
                    // Numeric token: always route to wildcard.
                    let entry = (*cur_ptr)
                        .children
                        .entry(self.param_id)
                        .or_insert_with(|| Box::new(Node::new()));
                    &mut **entry
                } else {
                    // Non-numeric token not yet in tree.
                    let specific_count = (*cur_ptr).children.len();
                    let has_wild = (*cur_ptr).children.contains_key(&self.param_id);
                    let available = self.cfg.max_children - 1;
                    if specific_count < available
                        || (!has_wild && specific_count < self.cfg.max_children - 1)
                    {
                        let entry = (*cur_ptr)
                            .children
                            .entry(token_id)
                            .or_insert_with(|| Box::new(Node::new()));
                        &mut **entry
                    } else {
                        let entry = (*cur_ptr)
                            .children
                            .entry(self.param_id)
                            .or_insert_with(|| Box::new(Node::new()));
                        &mut **entry
                    }
                }
            };
            cur_ptr = next_ptr;
            cur_depth += 1;
        }
    }
    fn sync_templates_from_clusters(&mut self) {
        let mut out: Vec<Template> = Vec::with_capacity(self.clusters.len().saturating_sub(1));
        for id in 1..self.clusters.len() {
            let c = match self.clusters[id].as_ref() {
                Some(c) => c,
                None => continue,
            };
            let token_count = c.token_ids.len();
            let mut params = vec![false; token_count];
            let mut dense = Vec::with_capacity(token_count - c.param_count);
            for (i, &tid) in c.token_ids.iter().enumerate() {
                if tid == self.param_id {
                    params[i] = true;
                } else {
                    dense.push(c.token_str[i].clone());
                }
            }
            out.push(Template {
                id: c.id,
                tokens: dense,
                params,
                token_count,
                count: c.size,
            });
        }
        out.sort_by(|a, b| b.count.cmp(&a.count));
        self.templates = out;
    }
}
// ---------------------------------------------------------------------------
// Training API
// ---------------------------------------------------------------------------
/// Train a matcher with default config.
pub fn train(samples: &[String], cfg: Config) -> Result<Matcher, String> {
    train_with_config(samples, cfg)
}
/// Train a matcher with custom config.
pub fn train_with_config(samples: &[String], cfg: Config) -> Result<Matcher, String> {
    let norm = cfg.normalize()?;
    let mut m = Matcher::new(norm);
    for sample in samples {
        m.add_log_message(sample);
    }
    m.sync_templates_from_clusters();
    m.rebuild_match_needed();
    m.prefilter_buckets = prefilter::rebuild_match_prefilter(&m.clusters, m.param_id);
    m.freeze_dict();
    Ok(m)
}
/// Rebuild a matcher from pre-existing templates.
pub fn matcher_from_templates(cfg: Config, templates: &[Template]) -> Result<Matcher, String> {
    let norm = cfg.normalize()?;
    let mut m = Matcher::new(norm);
    if templates.is_empty() {
        m.freeze_dict();
        return Ok(m);
    }
    let mut sorted: Vec<_> = templates.to_vec();
    sorted.sort_by_key(|t| t.id);
    let mut seen = std::collections::HashSet::new();
    let mut max_id = 0;
    for t in &sorted {
        if t.id == 0 {
            return Err(format!("template id must be > 0, got {}", t.id));
        }
        if !seen.insert(t.id) {
            return Err(format!("duplicate template id {}", t.id));
        }
        if t.count == 0 {
            return Err(format!("template {} count must be > 0", t.id));
        }
        if t.id > max_id {
            max_id = t.id;
        }
    }
    m.clusters.resize_with(max_id + 1, || None);
    m.next_cluster = max_id + 1;
    for t in &sorted {
        let mut full = vec![String::new(); t.token_count];
        let mut dense_idx = 0;
        for (i, slot) in full.iter_mut().enumerate().take(t.token_count) {
            if t.params.get(i).copied().unwrap_or(false) {
                *slot = m.cfg.param_string.clone();
            } else {
                *slot = t.tokens[dense_idx].clone();
                dense_idx += 1;
            }
        }
        let mut token_ids = Vec::new();
        m.intern_token_ids(&full, &mut token_ids);
        let cl = Box::new(Cluster::new(t.id, full, token_ids, m.param_id));
        m.clusters[t.id] = Some(cl);
    }
    for id in 1..m.clusters.len() {
        if m.clusters[id].is_some() {
            m.add_seq_to_prefix_tree(id);
        }
    }
    m.sync_templates_from_clusters();
    m.rebuild_match_needed();
    m.prefilter_buckets = prefilter::rebuild_match_prefilter(&m.clusters, m.param_id);
    m.freeze_dict();
    Ok(m)
}
// ---------------------------------------------------------------------------
// Tokenization helpers
// ---------------------------------------------------------------------------
/// Single-pass tokenizer that splits on individual space characters.
/// Does **not** trim and **does** preserve empty tokens, matching Go's
/// `tokenizeWhitespaceCount` so that consecutive spaces increase the token
/// count (and therefore change the tree bucket).
fn tokenize_whitespace_count(content: &str, dst: &mut Vec<String>, max_tokens: usize) -> usize {
    if content.is_empty() || max_tokens == 0 {
        return 0;
    }
    dst.clear();
    let bytes = content.as_bytes();
    let mut start = 0;
    let mut count = 1;
    for i in 0..bytes.len() {
        if bytes[i] != b' ' {
            continue;
        }
        dst.push(unsafe { std::str::from_utf8_unchecked(&bytes[start..i]) }.to_string());
        start = i + 1;
        if count >= max_tokens {
            return count + 1; // signal overflow
        }
        count += 1;
    }
    // SAFETY: same as above — sub-slice of a valid UTF-8 string.
    dst.push(unsafe { std::str::from_utf8_unchecked(&bytes[start..]) }.to_string());
    count
}

/// Tokenizer used when `extra_delimiters` are configured. Matches Go's
/// `tokenize`: trim, replace delimiters with spaces, then collapse runs of
/// spaces and skip empty fields (like `strings.Fields`).
fn tokenize(content: &str, extra_delimiters: &[String], max_tokens: usize) -> Vec<String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let mut s = trimmed.to_string();
    for d in extra_delimiters {
        s = s.replace(d, " ");
    }
    s.split(' ')
        .filter(|t| !t.is_empty())
        .take(max_tokens)
        .map(|t| t.to_string())
        .collect()
}
fn has_numbers(s: &str) -> bool {
    s.bytes().any(|b| b.is_ascii_digit())
}

// ---------------------------------------------------------------------------
// RenderPlan
// ---------------------------------------------------------------------------

/// Precomputed recipe for rendering a template with supplied parameter values.
#[derive(Debug, Clone)]
pub struct RenderPlan {
    head: Vec<u8>,
    segments: Vec<RenderSegment>,
    max_size: usize,
}

#[derive(Debug, Clone)]
struct RenderSegment {
    arg_idx: usize,
    tail: Vec<u8>,
}

impl RenderPlan {
    /// Build a render plan for `t`.
    ///
    /// `max_arg_len` is optional. When provided, it is called once for each
    /// parameter position and included in `max_size`.
    pub fn new(t: &Template, max_arg_len: Option<&dyn Fn(usize) -> usize>) -> Self {
        let mut head: Vec<u8> = Vec::new();
        let mut segments: Vec<RenderSegment> = Vec::new();
        let mut arg_idx = 0usize;
        let mut tok_idx = 0usize;
        let mut cur: Vec<u8> = Vec::new();

        for i in 0..t.token_count {
            if i > 0 {
                cur.push(b' ');
            }
            if t.params.get(i).copied().unwrap_or(false) {
                if let Some(last) = segments.last_mut() {
                    last.tail = cur;
                } else {
                    head = cur;
                }
                segments.push(RenderSegment {
                    arg_idx,
                    tail: Vec::new(),
                });
                cur = Vec::new();
                arg_idx += 1;
            } else {
                cur.extend_from_slice(t.tokens[tok_idx].as_bytes());
                tok_idx += 1;
            }
        }
        if let Some(last) = segments.last_mut() {
            last.tail = cur;
        } else {
            head = cur;
        }

        let mut max_size = head.len();
        for seg in &segments {
            max_size += seg.tail.len();
            if let Some(f) = max_arg_len {
                max_size += f(seg.arg_idx);
            }
        }
        Self {
            head,
            segments,
            max_size,
        }
    }

    /// Upper-bound size calculated by `new`.
    pub fn max_size(&self) -> usize {
        self.max_size
    }

    /// Append the rendered template to `dst`.
    ///
    /// `args` provides parameter values by position. Missing positions render
    /// as empty strings.
    pub fn append(&self, dst: &mut Vec<u8>, args: Option<&[&str]>) {
        dst.extend_from_slice(&self.head);
        for seg in &self.segments {
            if let Some(a) = args
                && let Some(s) = a.get(seg.arg_idx)
            {
                dst.extend_from_slice(s.as_bytes());
            }
            dst.extend_from_slice(&seg.tail);
        }
    }
}

// ---------------------------------------------------------------------------
// StrideSample
// ---------------------------------------------------------------------------

/// Deterministically sample `frac * len(lines)` lines as fixed-size blocks at
/// regular strides with random jitter inside each stride window.
///
/// Uses a seeded rng derived from the input length so the result is
/// deterministic — same input produces the same sample across runs.
pub fn stride_sample(lines: &[String], frac: f64, block_size: usize) -> Vec<String> {
    let total = lines.len();
    let sample_n = (total as f64 * frac) as usize;
    if sample_n == 0 || total == 0 {
        return lines.to_vec();
    }
    let num_blocks = (sample_n / block_size).max(1);
    let stride = (total / num_blocks).max(block_size);
    let mut rng = SmallRng::seed_from_u64(total as u64);
    let mut out: Vec<String> = Vec::with_capacity(sample_n);
    let mut start = 0usize;
    while start < total && out.len() < sample_n {
        let max_offset = stride.saturating_sub(block_size).max(1);
        let offset = start + (rng.next_u32() as usize % max_offset);
        if offset >= total {
            break;
        }
        let end = (offset + block_size).min(total);
        out.extend(lines[offset..end].iter().cloned());
        start += stride;
    }
    out
}

/// Tiny deterministic PRNG (xorshift64*) so `stride_sample` does not need
/// an external `rand` dependency.
struct SmallRng(u64);

impl SmallRng {
    fn seed_from_u64(seed: u64) -> Self {
        let mut s = seed;
        // Mix the seed with a few rounds of xorshift so small seeds don't
        // produce trivial sequences.
        for _ in 0..4 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
        }
        Self(s.max(1))
    }
    fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 << 12;
        self.0 ^= self.0 >> 25;
        self.0 ^= self.0 << 27;
        self.0 = self.0.wrapping_mul(0x2545_f491_4f6c_dd1d);
        (self.0 >> 32) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    /// Expand a dense Template into its full token sequence, inserting
    /// `param_str` at each position marked in `params`.
    fn render_template_placeholders(t: &Template, param_str: &str) -> String {
        let mut out: Vec<String> = Vec::with_capacity(t.token_count);
        let mut dense_idx = 0;
        for i in 0..t.token_count {
            if t.params.get(i).copied().unwrap_or(false) {
                out.push(param_str.to_string());
            } else {
                out.push(t.tokens[dense_idx].clone());
                dense_idx += 1;
            }
        }
        out.join(" ")
    }
    // -----------------------------------------------------------------
    // Reference behaviour: scenarios ported from logpai/Drain3 test_drain.py
    // -----------------------------------------------------------------
    #[test]
    fn logpai_sshd_scenario() {
        let samples: Vec<String> = vec![
            "Dec 10 07:07:38 LabSZ sshd[24206]: input_userauth_request: invalid user test9 [preauth]".into(),
            "Dec 10 07:08:28 LabSZ sshd[24208]: input_userauth_request: invalid user webmaster [preauth]".into(),
            "Dec 10 09:12:32 LabSZ sshd[24490]: Failed password for invalid user ftpuser from 0.0.0.0 port 62891 ssh2".into(),
            "Dec 10 09:12:35 LabSZ sshd[24492]: Failed password for invalid user pi from 0.0.0.0 port 49289 ssh2".into(),
            "Dec 10 09:12:44 LabSZ sshd[24501]: Failed password for invalid user ftpuser from 0.0.0.0 port 60836 ssh2".into(),
            "Dec 10 07:28:03 LabSZ sshd[24245]: input_userauth_request: invalid user pgadmin [preauth]".into(),
        ];
        let cfg = Config {
            similarity_threshold: 0.4, // logpai default
            ..Config::default()
        };
        let m = train_with_config(&samples, cfg.clone()).unwrap();
        let mut want: HashMap<String, usize> = HashMap::new();
        want.insert(
            "Dec 10 <*> LabSZ <*> input_userauth_request: invalid user <*> [preauth]".into(),
            3,
        );
        want.insert(
            "Dec 10 <*> LabSZ <*> Failed password for invalid user <*> from 0.0.0.0 port <*> ssh2"
                .into(),
            3,
        );
        let mut got: HashMap<String, usize> = HashMap::new();
        let mut total = 0;
        for tmpl in m.templates() {
            let key = render_template_placeholders(&tmpl, &cfg.param_string);
            *got.entry(key).or_insert(0) += tmpl.count;
            total += tmpl.count;
        }
        assert_eq!(got, want, "templates mismatch");
        assert_eq!(total, samples.len(), "total count mismatch");
    }
    #[test]
    fn logpai_sshd_scenario_high_sim() {
        let samples: Vec<String> = vec![
            "Dec 10 07:07:38 LabSZ sshd[24206]: input_userauth_request: invalid user test9 [preauth]".into(),
            "Dec 10 07:08:28 LabSZ sshd[24208]: input_userauth_request: invalid user webmaster [preauth]".into(),
            "Dec 10 09:12:32 LabSZ sshd[24490]: Failed password for invalid user ftpuser from 0.0.0.0 port 62891 ssh2".into(),
            "Dec 10 09:12:35 LabSZ sshd[24492]: Failed password for invalid user pi from 0.0.0.0 port 49289 ssh2".into(),
            "Dec 10 09:12:44 LabSZ sshd[24501]: Failed password for invalid user ftpuser from 0.0.0.0 port 60836 ssh2".into(),
            "Dec 10 07:28:03 LabSZ sshd[24245]: input_userauth_request: invalid user pgadmin [preauth]".into(),
        ];
        let cfg = Config {
            similarity_threshold: 0.75,
            ..Config::default()
        };
        let m = train_with_config(&samples, cfg.clone()).unwrap();
        let mut want: HashMap<String, usize> = HashMap::new();
        want.insert(samples[0].clone(), 1);
        want.insert(samples[1].clone(), 1);
        want.insert(
            "Dec 10 <*> LabSZ <*> Failed password for invalid user <*> from 0.0.0.0 port <*> ssh2"
                .into(),
            3,
        );
        want.insert(samples[5].clone(), 1);
        let mut got: HashMap<String, usize> = HashMap::new();
        let mut total = 0;
        for tmpl in m.templates() {
            let key = render_template_placeholders(&tmpl, &cfg.param_string);
            *got.entry(key).or_insert(0) += tmpl.count;
            total += tmpl.count;
        }
        assert_eq!(got, want, "templates mismatch");
        assert_eq!(total, samples.len(), "total count mismatch");
    }
    #[test]
    fn logpai_short_message() {
        let m = train(
            &["hello".into(), "hello".into(), "otherword".into()],
            Config::default(),
        )
        .unwrap();
        let mut got: HashMap<String, usize> = HashMap::new();
        for tmpl in m.templates() {
            let key = render_template_placeholders(&tmpl, "<*>");
            *got.entry(key).or_insert(0) += tmpl.count;
        }
        let mut want: HashMap<String, usize> = HashMap::new();
        want.insert("hello".into(), 2);
        want.insert("otherword".into(), 1);
        assert_eq!(got, want, "templates mismatch");
    }
    #[test]
    fn logpai_match_only() {
        let m = train(
            &[
                "aa aa aa".into(),
                "aa aa bb".into(),
                "aa aa cc".into(),
                "xx yy zz".into(),
            ],
            Config::default(),
        )
        .unwrap();
        let cases: Vec<(&str, usize)> = vec![
            ("aa aa tt", 1), // wildcard absorbs tt
            ("xx yy zz", 2), // exact
            ("xx yy rr", 0), // literal mismatch
            ("nothing", 0),  // unknown token count
        ];
        for (line, want) in cases {
            let id = m.match_id(line);
            if want == 0 {
                assert!(
                    id.is_none(),
                    "Match({line:?}): got id={id:?}, want no match"
                );
            } else {
                assert_eq!(
                    id,
                    Some(want),
                    "Match({line:?}): got id={id:?}, want id={want}"
                );
            }
        }
    }
    // -----------------------------------------------------------------
    // Properties
    // -----------------------------------------------------------------
    #[test]
    fn deterministic_templates() {
        let samples: Vec<String> = vec![
            "svc 1 INFO user 10".into(),
            "svc 2 INFO user 20".into(),
            "svc 3 ERROR user 30".into(),
            "svc 4 ERROR user 40".into(),
        ];
        let m1 = train(&samples, Config::default()).unwrap();
        let m2 = train(&samples, Config::default()).unwrap();
        assert_eq!(
            m1.templates(),
            m2.templates(),
            "templates are not deterministic"
        );
    }
    #[test]
    fn train_handles_empty_input() {
        let m = train(&[], Config::default()).unwrap();
        assert!(m.templates().is_empty(), "expected no templates");
        assert!(m.match_id("anything").is_none(), "expected no match");
    }
    #[test]
    fn zero_thresholds_are_valid() {
        let cfg = Config {
            similarity_threshold: 0.0,
            match_threshold: 0.0,
            ..Config::default()
        };
        let m = train_with_config(&["A B C".into(), "A B D".into()], cfg).unwrap();
        // MatchThreshold=0.0 accepts any tree-routable candidate.
        assert!(
            m.match_id("A X Y").is_some(),
            "expected match with 0.0 match threshold"
        );
        // SimilarityThreshold=0.0 merges aggressively: one template.
        assert_eq!(
            m.templates().len(),
            1,
            "expected 1 template with 0.0 similarity"
        );
    }
    #[test]
    fn max_clusters() {
        let lines: Vec<String> = vec![
            "alpha X Y".into(),
            "bravo X Y".into(),
            "charlie X Y".into(),
            "delta X Y".into(),
            "echo X Y".into(),
        ];
        let cfg = Config {
            max_clusters: 2,
            ..Config::default()
        };
        let capped = train_with_config(&lines, cfg.clone()).unwrap();
        assert!(
            capped.templates().len() <= 2,
            "expected at most 2 templates, got {}",
            capped.templates().len()
        );
        let cfg = Config {
            max_clusters: 0,
            ..Config::default()
        };
        let full = train_with_config(&lines, cfg).unwrap();
        assert!(
            full.templates().len() > capped.templates().len(),
            "expected uncapped training to produce more templates: uncapped={} capped={}",
            full.templates().len(),
            capped.templates().len()
        );
    }
    // -----------------------------------------------------------------
    // Config validation
    // -----------------------------------------------------------------
    #[test]
    fn train_with_config_validation() {
        let cfg = Config {
            depth: 2,
            ..Config::default()
        };
        assert!(
            train_with_config(&["a b c".into()], cfg).is_err(),
            "expected error for invalid depth"
        );
    }
    #[test]
    fn zero_value_config_is_rejected() {
        let zero_cfg = Config {
            depth: 0,
            similarity_threshold: 0.0,
            match_threshold: 0.0,
            max_children: 0,
            max_tokens: 0,
            max_bytes: 0,
            max_clusters: 0,
            param_string: String::new(),
            parametrize_numeric_tokens: false,
            extra_delimiters: Vec::new(),
            enable_match_prefilter: false,
        };
        assert!(
            train_with_config(&["a b c".into()], zero_cfg).is_err(),
            "expected error for zero-value Config"
        );
    }
    // -----------------------------------------------------------------
    // Features
    // -----------------------------------------------------------------
    #[test]
    fn extra_delimiters() {
        let cfg = Config {
            extra_delimiters: vec!["=".into()],
            ..Config::default()
        };
        let m = train_with_config(&["k=v a=1".into(), "k=v a=2".into()], cfg).unwrap();
        let (id, args, ok) = m.match_line("k=v a=7");
        assert!(ok, "expected match");
        assert_eq!(id, 1, "expected template id 1, got {id}");
        assert_eq!(args, vec!["7"], "unexpected args: {args:?}");
    }
    #[test]
    fn match_into() {
        let samples: Vec<String> = vec![
            "service 1 level INFO user 10 action 5".into(),
            "service 2 level INFO user 20 action 5".into(),
            "service 3 level INFO user 30 action 5".into(),
        ];
        let m = train(&samples, Config::default()).unwrap();
        let line = "service 99 level INFO user 777 action 5";
        let (id_a, args_a, ok_a) = m.match_line(line);
        let mut scratch: Vec<String> = Vec::with_capacity(8);
        let (id_b, ok_b) = m.match_into(line, &mut scratch);
        assert_eq!(id_a, id_b, "MatchInto id mismatch");
        assert_eq!(ok_a, ok_b, "MatchInto ok mismatch");
        assert_eq!(args_a, scratch, "MatchInto args mismatch");
        assert!(!scratch.is_empty(), "expected extracted params");
        scratch.clear();
        let (_, ok_miss) = m.match_into("short unmatched", &mut scratch);
        assert!(!ok_miss, "expected no match");
        assert!(
            scratch.is_empty(),
            "expected empty args on miss, got {scratch:?}"
        );
    }
    #[test]
    fn config_and_templates_are_copied() {
        let cfg = Config {
            extra_delimiters: vec!["=".into()],
            ..Config::default()
        };
        let m = train_with_config(&["k=v a=1".into(), "k=v a=2".into()], cfg).unwrap();
        let mut read_cfg = m.config();
        read_cfg.extra_delimiters[0] = ":".into();
        assert_eq!(
            m.config().extra_delimiters[0],
            "=",
            "config getter leaked mutable slice"
        );
        let mut templates = m.templates();
        templates[0].tokens[0] = "mutated".into();
        assert_ne!(
            templates,
            m.templates(),
            "templates getter leaked mutable data"
        );
    }
}
