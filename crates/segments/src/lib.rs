use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Signal {
    Spans,
    Logs,
    Metrics,
    Profiles,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub key: String,
    pub signal: Signal,
    pub min_time_us: i64,
    pub max_time_us: i64,
    pub rows: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    KeyInUse(String),
    NotPending(String),
    NotLive(String),
    SignalMismatch(String),
}

#[derive(Debug, Default, Clone)]
pub struct Manifest {
    generation: u64,
    pending: BTreeMap<String, Signal>,
    live: BTreeMap<String, Segment>,
}

impl Manifest {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn register(&mut self, key: &str, signal: Signal) -> Result<(), ManifestError> {
        if self.pending.contains_key(key) || self.live.contains_key(key) {
            return Err(ManifestError::KeyInUse(key.into()));
        }
        self.pending.insert(key.into(), signal);
        self.generation += 1;
        Ok(())
    }

    pub fn commit(&mut self, segment: Segment) -> Result<(), ManifestError> {
        self.take_pending(&segment)?;
        self.live.insert(segment.key.clone(), segment);
        self.generation += 1;
        Ok(())
    }

    pub fn replace(&mut self, inputs: &[&str], output: Segment) -> Result<(), ManifestError> {
        for key in inputs {
            match self.live.get(*key) {
                None => return Err(ManifestError::NotLive((*key).into())),
                Some(input) if input.signal != output.signal => {
                    return Err(ManifestError::SignalMismatch((*key).into()));
                }
                Some(_) => {}
            }
        }
        self.take_pending(&output)?;
        for key in inputs {
            self.live.remove(*key);
        }
        self.live.insert(output.key.clone(), output);
        self.generation += 1;
        Ok(())
    }

    pub fn abandon(&mut self, key: &str) {
        if self.pending.remove(key).is_some() {
            self.generation += 1;
        }
    }

    pub fn expire(&mut self, signal: Signal, cutoff_us: i64) -> Vec<String> {
        let expired: Vec<String> = self
            .live
            .values()
            .filter(|s| s.signal == signal && s.max_time_us < cutoff_us)
            .map(|s| s.key.clone())
            .collect();
        if !expired.is_empty() {
            for key in &expired {
                self.live.remove(key);
            }
            self.generation += 1;
        }
        expired
    }

    pub fn live(&self, signal: Signal, from_us: i64, to_us: i64) -> Vec<&Segment> {
        self.live
            .values()
            .filter(|s| s.signal == signal && s.min_time_us <= to_us && s.max_time_us >= from_us)
            .collect()
    }

    pub fn orphans<'a>(&self, stored: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
        let known: BTreeSet<&str> = self
            .pending
            .keys()
            .chain(self.live.keys())
            .map(String::as_str)
            .collect();
        stored.filter(|key| !known.contains(key)).collect()
    }

    fn take_pending(&mut self, segment: &Segment) -> Result<(), ManifestError> {
        match self.pending.get(&segment.key) {
            None => Err(ManifestError::NotPending(segment.key.clone())),
            Some(signal) if *signal != segment.signal => {
                Err(ManifestError::SignalMismatch(segment.key.clone()))
            }
            Some(_) => {
                self.pending.remove(&segment.key);
                Ok(())
            }
        }
    }
}
