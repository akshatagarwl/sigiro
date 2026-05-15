/// Fast read-only token-to-ID lookup table.
///
/// Uses a sorted flat array with binary search.  Faster than `HashMap` for
/// read-only access because there is no hashing overhead and the layout is
/// cache-friendly.
#[derive(Debug, Clone)]
pub struct FrozenDict {
    keys: Vec<String>,
    vals: Vec<u64>,
}

impl FrozenDict {
    /// Build a frozen dictionary from (key, value) pairs.
    pub fn new(mut entries: Vec<(String, u64)>) -> Self {
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut keys = Vec::with_capacity(entries.len());
        let mut vals = Vec::with_capacity(entries.len());
        for (k, v) in entries {
            keys.push(k);
            vals.push(v);
        }
        Self { keys, vals }
    }

    /// Lookup a key.  Returns `Some(id)` if found, `None` otherwise.
    pub fn lookup(&self, key: &str) -> Option<u64> {
        match self.keys.binary_search_by(|probe| probe.as_str().cmp(key)) {
            Ok(idx) => Some(self.vals[idx]),
            Err(_) => None,
        }
    }
}
