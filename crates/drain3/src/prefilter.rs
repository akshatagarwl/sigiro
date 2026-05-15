use std::collections::HashMap;

use crate::Cluster;

#[derive(Debug, Default, Clone)]
pub struct PrefilterBucket {
    pub any: Vec<usize>,
    pub first_keys: Vec<u64>,
    pub first_vals: Vec<Vec<usize>>,
    pub last_keys: Vec<u64>,
    pub last_vals: Vec<Vec<usize>>,
    pub fl_keys: Vec<u64>,
    pub fl_vals: Vec<Vec<usize>>,
}

pub fn rebuild_match_prefilter(
    clusters: &[Option<Box<Cluster>>],
    param_id: u64,
) -> Vec<PrefilterBucket> {
    let mut any_by_tc: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut first_by_tc: HashMap<usize, HashMap<u64, Vec<usize>>> = HashMap::new();
    let mut last_by_tc: HashMap<usize, HashMap<u64, Vec<usize>>> = HashMap::new();
    let mut fl_by_tc: HashMap<usize, HashMap<u64, Vec<usize>>> = HashMap::new();
    let mut max_len = 0usize;

    for (id, cluster) in clusters.iter().enumerate().skip(1) {
        let cluster = match cluster.as_ref() {
            Some(c) => c,
            None => continue,
        };

        let token_count = cluster.token_ids.len();
        if token_count > max_len {
            max_len = token_count;
        }
        if token_count == 0 {
            any_by_tc.entry(0).or_default().push(id);
            continue;
        }

        let first_id = cluster.token_ids[0];
        let last_id = cluster.token_ids[token_count - 1];
        let first_is_param = first_id == param_id;
        let last_is_param = last_id == param_id;

        match (first_is_param, last_is_param) {
            (true, true) => {
                any_by_tc.entry(token_count).or_default().push(id);
            }
            (false, true) => {
                first_by_tc
                    .entry(token_count)
                    .or_default()
                    .entry(first_id)
                    .or_default()
                    .push(id);
            }
            (true, false) => {
                last_by_tc
                    .entry(token_count)
                    .or_default()
                    .entry(last_id)
                    .or_default()
                    .push(id);
            }
            (false, false) => {
                let combined = (first_id << 32) | (last_id & 0xFFFFFFFF);
                fl_by_tc
                    .entry(token_count)
                    .or_default()
                    .entry(combined)
                    .or_default()
                    .push(id);
            }
        }
    }

    let mut buckets = vec![PrefilterBucket::default(); max_len + 1];
    for (tc, ids) in any_by_tc {
        if tc < buckets.len() {
            buckets[tc].any = ids;
        }
    }
    for (tc, mm) in first_by_tc {
        if tc < buckets.len() {
            let (keys, vals) = sorted_u64_keys(mm);
            buckets[tc].first_keys = keys;
            buckets[tc].first_vals = vals;
        }
    }
    for (tc, mm) in last_by_tc {
        if tc < buckets.len() {
            let (keys, vals) = sorted_u64_keys(mm);
            buckets[tc].last_keys = keys;
            buckets[tc].last_vals = vals;
        }
    }
    for (tc, mm) in fl_by_tc {
        if tc < buckets.len() {
            let (keys, vals) = sorted_u64_keys(mm);
            buckets[tc].fl_keys = keys;
            buckets[tc].fl_vals = vals;
        }
    }

    buckets
}

pub fn prefilter_candidates_compact<'a>(
    buckets: &'a [PrefilterBucket],
    dict_ids: &HashMap<String, u64>,
    tokens: &[String],
    dst: &'a mut Vec<usize>,
) -> Option<&'a [usize]> {
    let tc = tokens.len();
    let b = buckets.get(tc)?;
    let any = &b.any[..];

    let mut first = &[][..];
    let mut last = &[][..];
    let mut first_last = &[][..];

    if tc > 0 {
        let first_id = dict_ids.get(&tokens[0]).copied().unwrap_or(0);
        let last_id = dict_ids.get(&tokens[tc - 1]).copied().unwrap_or(0);
        let first_known = first_id != 0;
        let last_known = last_id != 0;

        if first_known {
            first = search_sorted_u64(&b.first_keys, &b.first_vals, first_id);
        }
        if last_known {
            last = search_sorted_u64(&b.last_keys, &b.last_vals, last_id);
        }
        if first_known && last_known {
            let combined = (first_id << 32) | (last_id & 0xFFFFFFFF);
            first_last = search_sorted_u64(&b.fl_keys, &b.fl_vals, combined);
        }
    }

    merge_prefilter_groups(any, first, last, first_last, dst)
}

fn merge_prefilter_groups<'a>(
    any: &'a [usize],
    first: &'a [usize],
    last: &'a [usize],
    first_last: &'a [usize],
    dst: &'a mut Vec<usize>,
) -> Option<&'a [usize]> {
    let mut non_empty = 0usize;
    let mut single: Option<&'a [usize]> = None;
    let mut total = 0usize;

    for group in [any, first, last, first_last] {
        if group.is_empty() {
            continue;
        }
        non_empty += 1;
        single = Some(group);
        total += group.len();
    }

    if non_empty == 0 {
        return None;
    }
    if non_empty == 1 {
        return single;
    }

    dst.clear();
    dst.reserve(total);
    dst.extend_from_slice(any);
    dst.extend_from_slice(first);
    dst.extend_from_slice(last);
    dst.extend_from_slice(first_last);
    Some(dst)
}

fn search_sorted_u64<'a>(keys: &'a [u64], vals: &'a [Vec<usize>], target: u64) -> &'a [usize] {
    match keys.binary_search(&target) {
        Ok(i) => &vals[i],
        Err(_) => &[],
    }
}

fn sorted_u64_keys(m: HashMap<u64, Vec<usize>>) -> (Vec<u64>, Vec<Vec<usize>>) {
    let mut items: Vec<(u64, Vec<usize>)> = m.into_iter().collect();
    items.sort_unstable_by_key(|(k, _)| *k);
    let mut keys = Vec::with_capacity(items.len());
    let mut vals = Vec::with_capacity(items.len());
    for (k, v) in items {
        keys.push(k);
        vals.push(v);
    }
    (keys, vals)
}
