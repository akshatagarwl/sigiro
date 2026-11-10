//! Box-level disk-capacity gate as an explicit state machine.
//!
//! The storage-maintenance task samples `SIGIRO_DATA_DIR` and feeds each
//! sample to [`DiskCapacityState::on_sample`]; the self-heal retention run it
//! spawns reports back through [`HealGuard`]. Every transition is computed by
//! the pure [`step`] and logged once. The ingest hot path only loads one
//! `AtomicU8` ([`DiskCapacityState::is_over_cap`]).

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// Why a tripped gate cannot reopen on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StuckReason {
    /// `SIGIRO_RETENTION_DAYS=0`: nothing will ever delete data. Free disk,
    /// raise `SIGIRO_MAX_DISK_BYTES`, or enable retention.
    RetentionOff,
    /// A retention run finished and the next sample was no smaller: everything
    /// left is inside the retention window.
    HealFreedNothing,
    /// The retention run errored or panicked; see the log line before this one.
    HealFailed,
}

/// Where the gate is. Ingest is rejected in `Tripped`, `Healing`, and `Stuck`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum DiskGateState {
    /// `SIGIRO_MAX_DISK_BYTES=0`.
    Disabled,
    /// Under the cap; ingest accepted.
    Open,
    /// Over the cap after a retention run that freed space; the next sample
    /// decides between another run and reopening.
    Tripped,
    /// Over the cap with a self-heal retention run in flight.
    Healing,
    /// Over the cap and no run can help; only falling usage reopens it.
    Stuck(StuckReason),
}

impl DiskGateState {
    pub fn rejects_ingest(self) -> bool {
        matches!(self, Self::Tripped | Self::Healing | Self::Stuck(_))
    }

    const fn encode(self) -> u8 {
        match self {
            Self::Disabled => 0,
            Self::Open => 1,
            Self::Tripped => 2,
            Self::Healing => 3,
            Self::Stuck(StuckReason::RetentionOff) => 4,
            Self::Stuck(StuckReason::HealFreedNothing) => 5,
            Self::Stuck(StuckReason::HealFailed) => 6,
        }
    }

    const fn decode(value: u8) -> Self {
        match value {
            0 => Self::Disabled,
            1 => Self::Open,
            2 => Self::Tripped,
            3 => Self::Healing,
            4 => Self::Stuck(StuckReason::RetentionOff),
            5 => Self::Stuck(StuckReason::HealFreedNothing),
            _ => Self::Stuck(StuckReason::HealFailed),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskGateEvent {
    /// `reliable` is false when the walk hit an unreadable entry, so `bytes`
    /// is an under-count: it may trip the gate but never clear it.
    Sample {
        bytes: u64,
        reliable: bool,
    },
    HealFinished {
        ok: bool,
    },
}

/// Facts [`step`] needs besides the state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskGateContext {
    pub cap: u64,
    pub retention_on: bool,
    /// Bytes sampled when the last self-heal run started.
    pub heal_baseline_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskGateEffect {
    /// Start one retention run now; record `bytes` as its baseline.
    SpawnHeal { baseline_bytes: u64 },
}

/// The whole gate. Trips at `bytes >= cap`, clears at `bytes <= 90% of cap`
/// on a reliable sample (hysteresis), and heals while runs make progress.
pub fn step(
    state: DiskGateState,
    event: DiskGateEvent,
    context: DiskGateContext,
) -> (DiskGateState, Option<DiskGateEffect>) {
    use DiskGateEvent::{HealFinished, Sample};
    use DiskGateState::{Disabled, Healing, Open, Stuck, Tripped};
    if context.cap == 0 {
        return (Disabled, None);
    }
    // u128 keeps 90% exact (no `cap/10*9` truncation that breaks small caps,
    // e.g. cap=9 would clear only at 0 bytes) while never overflowing u64.
    let clear_threshold = ((u128::from(context.cap) * 9) / 10) as u64;
    let heal = |bytes| {
        if context.retention_on {
            (
                Healing,
                Some(DiskGateEffect::SpawnHeal {
                    baseline_bytes: bytes,
                }),
            )
        } else {
            (Stuck(StuckReason::RetentionOff), None)
        }
    };
    match (state, event) {
        (Disabled, Sample { bytes, .. }) | (Open, Sample { bytes, .. }) if bytes >= context.cap => {
            heal(bytes)
        }
        (Disabled, Sample { .. }) => (Open, None),
        (
            Tripped | Healing | Stuck(_),
            Sample {
                bytes,
                reliable: true,
            },
        ) if bytes <= clear_threshold => (Open, None),
        (Tripped, Sample { bytes, .. }) if bytes < context.heal_baseline_bytes => heal(bytes),
        (Tripped, Sample { .. }) => (Stuck(StuckReason::HealFreedNothing), None),
        (Healing, HealFinished { ok: true }) => (Tripped, None),
        (Healing, HealFinished { ok: false }) => (Stuck(StuckReason::HealFailed), None),
        (state, Sample { .. } | HealFinished { .. }) => (state, None),
    }
}

/// Box-level disk-capacity gate, shared (`Arc`) between the ingest router —
/// which calls [`Self::is_over_cap`] at the top of `send` — and the
/// storage-maintenance task. `cap == 0` disables the gate entirely.
#[derive(Debug)]
pub struct DiskCapacityState {
    /// `SIGIRO_MAX_DISK_BYTES`. 0 disables the gate.
    pub(crate) cap: u64,
    /// [`DiskGateState`], encoded.
    state: AtomicU8,
    /// Most recent sampled on-disk byte size of `SIGIRO_DATA_DIR`.
    sampled_bytes: AtomicU64,
    heal_baseline_bytes: AtomicU64,
    /// Serializes load → step → store so a heal finishing during a sample
    /// cannot lose either transition. Never taken on the ingest path.
    transition: Mutex<()>,
}

/// What `/health` reports while the gate rejects ingest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DiskGateHealth {
    #[serde(flatten)]
    pub state: DiskGateState,
    pub sampled_bytes: u64,
    pub cap_bytes: u64,
}

impl DiskCapacityState {
    pub fn new(cap: u64) -> Arc<Self> {
        let initial = if cap == 0 {
            DiskGateState::Disabled
        } else {
            DiskGateState::Open
        };
        Arc::new(Self {
            cap,
            state: AtomicU8::new(initial.encode()),
            sampled_bytes: AtomicU64::new(0),
            heal_baseline_bytes: AtomicU64::new(0),
            transition: Mutex::new(()),
        })
    }

    /// A disabled (`cap == 0`) gate — the no-op default for constructors.
    pub(crate) fn disabled() -> Arc<Self> {
        Self::new(0)
    }

    pub fn state(&self) -> DiskGateState {
        DiskGateState::decode(self.state.load(Ordering::Acquire))
    }

    /// True when ingest must be rejected. One atomic load: cheap enough for
    /// the ingest hot path.
    pub fn is_over_cap(&self) -> bool {
        self.state().rejects_ingest()
    }

    /// `Some` while the gate rejects ingest, for `/health`.
    pub fn health(&self) -> Option<DiskGateHealth> {
        let state = self.state();
        state.rejects_ingest().then(|| DiskGateHealth {
            state,
            sampled_bytes: self.sampled_bytes.load(Ordering::Relaxed),
            cap_bytes: self.cap,
        })
    }

    /// Fold a fresh sample into the gate. Returns the effect the caller must
    /// run (a self-heal retention pass), if any.
    pub fn on_sample(
        &self,
        bytes: u64,
        reliable: bool,
        retention_on: bool,
    ) -> Option<DiskGateEffect> {
        self.sampled_bytes.store(bytes, Ordering::Relaxed);
        self.apply(DiskGateEvent::Sample { bytes, reliable }, retention_on)
    }

    fn apply(&self, event: DiskGateEvent, retention_on: bool) -> Option<DiskGateEffect> {
        let _serialized = self.transition.lock();
        let from = self.state();
        let context = DiskGateContext {
            cap: self.cap,
            retention_on,
            heal_baseline_bytes: self.heal_baseline_bytes.load(Ordering::Relaxed),
        };
        let (to, effect) = step(from, event, context);
        if let Some(DiskGateEffect::SpawnHeal { baseline_bytes }) = effect {
            self.heal_baseline_bytes
                .store(baseline_bytes, Ordering::Relaxed);
        }
        if to != from {
            self.state.store(to.encode(), Ordering::Release);
            let sampled_bytes = self.sampled_bytes.load(Ordering::Relaxed);
            if to.rejects_ingest() {
                tracing::warn!(
                    ?from,
                    ?to,
                    sampled_bytes,
                    cap = self.cap,
                    "disk gate: REJECTING ingest"
                );
            } else {
                tracing::info!(
                    ?from,
                    ?to,
                    sampled_bytes,
                    cap = self.cap,
                    "disk gate: accepting ingest"
                );
            }
        }
        effect
    }

    /// Track one self-heal run. Dropping the guard without [`HealGuard::finish`]
    /// (error or panic) reports a failed run.
    pub fn start_heal(self: &Arc<Self>) -> HealGuard {
        HealGuard {
            disk: Arc::clone(self),
            reported: false,
        }
    }
}

/// Reports a self-heal run's end to the gate exactly once, on every exit
/// path — normal completion, error, or panic-unwind — so `Healing` can never
/// outlive its run.
pub struct HealGuard {
    disk: Arc<DiskCapacityState>,
    reported: bool,
}

impl HealGuard {
    pub fn finish(mut self, ok: bool) {
        self.report(ok);
    }

    fn report(&mut self, ok: bool) {
        if !self.reported {
            self.reported = true;
            // Retention is on whenever a heal ran; a finish never trips.
            self.disk.apply(DiskGateEvent::HealFinished { ok }, true);
        }
    }
}

impl Drop for HealGuard {
    fn drop(&mut self) {
        self.report(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use DiskGateEvent::{HealFinished, Sample};
    use DiskGateState::{Disabled, Healing, Open, Stuck, Tripped};
    use StuckReason::{HealFailed, HealFreedNothing, RetentionOff};

    const CAP: u64 = 100;

    fn ctx(retention_on: bool, heal_baseline_bytes: u64) -> DiskGateContext {
        DiskGateContext {
            cap: CAP,
            retention_on,
            heal_baseline_bytes,
        }
    }
    const fn sample(bytes: u64) -> DiskGateEvent {
        Sample {
            bytes,
            reliable: true,
        }
    }
    const fn under_count(bytes: u64) -> DiskGateEvent {
        Sample {
            bytes,
            reliable: false,
        }
    }
    const fn spawn(baseline_bytes: u64) -> Option<DiskGateEffect> {
        Some(DiskGateEffect::SpawnHeal { baseline_bytes })
    }

    #[test]
    fn every_disk_gate_transition_follows_the_table() {
        let cases: &[(
            DiskGateState,
            DiskGateEvent,
            DiskGateContext,
            DiskGateState,
            Option<DiskGateEffect>,
        )] = &[
            (Open, sample(50), ctx(true, 0), Open, None),
            (Open, sample(99), ctx(true, 0), Open, None),
            (Open, sample(100), ctx(true, 0), Healing, spawn(100)),
            (Open, under_count(120), ctx(true, 0), Healing, spawn(120)),
            (Open, sample(100), ctx(false, 0), Stuck(RetentionOff), None),
            (Open, HealFinished { ok: true }, ctx(true, 0), Open, None),
            (Healing, sample(120), ctx(true, 120), Healing, None),
            (Healing, sample(90), ctx(true, 120), Open, None),
            (Healing, under_count(10), ctx(true, 120), Healing, None),
            (
                Healing,
                HealFinished { ok: true },
                ctx(true, 120),
                Tripped,
                None,
            ),
            (
                Healing,
                HealFinished { ok: false },
                ctx(true, 120),
                Stuck(HealFailed),
                None,
            ),
            (Tripped, sample(90), ctx(true, 120), Open, None),
            (Tripped, sample(91), ctx(true, 120), Healing, spawn(91)),
            (Tripped, sample(110), ctx(true, 120), Healing, spawn(110)),
            (
                Tripped,
                sample(120),
                ctx(true, 120),
                Stuck(HealFreedNothing),
                None,
            ),
            (
                Tripped,
                sample(130),
                ctx(true, 120),
                Stuck(HealFreedNothing),
                None,
            ),
            (Tripped, under_count(50), ctx(true, 120), Healing, spawn(50)),
            (
                Tripped,
                HealFinished { ok: true },
                ctx(true, 120),
                Tripped,
                None,
            ),
            (
                Stuck(RetentionOff),
                sample(150),
                ctx(false, 0),
                Stuck(RetentionOff),
                None,
            ),
            (
                Stuck(RetentionOff),
                under_count(10),
                ctx(false, 0),
                Stuck(RetentionOff),
                None,
            ),
            (Stuck(RetentionOff), sample(90), ctx(false, 0), Open, None),
            (
                Stuck(HealFreedNothing),
                sample(95),
                ctx(true, 120),
                Stuck(HealFreedNothing),
                None,
            ),
            (
                Stuck(HealFreedNothing),
                sample(80),
                ctx(true, 120),
                Open,
                None,
            ),
            (Stuck(HealFailed), sample(80), ctx(true, 120), Open, None),
            (
                Stuck(HealFailed),
                HealFinished { ok: true },
                ctx(true, 120),
                Stuck(HealFailed),
                None,
            ),
            (Disabled, sample(10), ctx(true, 0), Open, None),
        ];
        for (from, event, context, to, effect) in cases {
            assert_eq!(
                step(*from, *event, *context),
                (*to, *effect),
                "{from:?} + {event:?} ({context:?})"
            );
        }
        let off = DiskGateContext {
            cap: 0,
            ..ctx(true, 0)
        };
        for state in [Open, Healing, Tripped, Stuck(RetentionOff)] {
            assert_eq!(step(state, sample(u64::MAX), off), (Disabled, None));
        }
    }

    #[test]
    fn small_caps_clear_at_exactly_ninety_percent() {
        let context = DiskGateContext {
            cap: 9,
            retention_on: false,
            heal_baseline_bytes: 0,
        };
        assert_eq!(step(Stuck(RetentionOff), sample(8), context).0, Open);
        assert_eq!(
            step(Stuck(RetentionOff), sample(9), context).0,
            Stuck(RetentionOff)
        );
    }

    #[test]
    fn every_state_round_trips_through_the_atomic() {
        for state in [
            Disabled,
            Open,
            Tripped,
            Healing,
            Stuck(RetentionOff),
            Stuck(HealFreedNothing),
            Stuck(HealFailed),
        ] {
            assert_eq!(DiskGateState::decode(state.encode()), state);
        }
    }

    #[test]
    fn a_heal_that_freed_nothing_ends_stuck_and_health_says_why() {
        let disk = DiskCapacityState::new(CAP);
        assert!(!disk.is_over_cap());
        assert_eq!(disk.health(), None);
        let Some(DiskGateEffect::SpawnHeal { .. }) = disk.on_sample(120, true, true) else {
            panic!("an over-cap sample must start a heal");
        };
        assert!(disk.is_over_cap());
        disk.start_heal().finish(true);
        assert_eq!(disk.state(), Tripped);
        assert_eq!(disk.on_sample(120, true, true), None);
        assert_eq!(disk.state(), Stuck(HealFreedNothing));
        let health = disk.health().unwrap();
        assert_eq!(
            serde_json::to_value(&health).unwrap(),
            serde_json::json!({"state": "stuck", "reason": "heal_freed_nothing", "sampled_bytes": 120, "cap_bytes": 100})
        );
        assert_eq!(disk.on_sample(85, true, true), None);
        assert_eq!(disk.state(), Open);
        assert_eq!(disk.health(), None);
    }

    #[test]
    fn a_panicking_heal_cannot_leave_the_gate_healing() {
        let disk = DiskCapacityState::new(CAP);
        disk.on_sample(150, true, true);
        assert_eq!(disk.state(), Healing);
        let guard = disk.start_heal();
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = guard;
            panic!("retention run panicked");
        }));
        assert!(unwound.is_err());
        assert_eq!(disk.state(), Stuck(HealFailed));
    }

    #[test]
    fn a_heal_that_frees_space_retries_until_usage_reopens_ingest() {
        let disk = DiskCapacityState::new(CAP);
        assert!(disk.on_sample(150, true, true).is_some());
        disk.start_heal().finish(true);
        assert_eq!(
            disk.on_sample(130, true, true),
            spawn(130),
            "progress earns another run"
        );
        disk.start_heal().finish(true);
        assert_eq!(disk.on_sample(70, true, true), None);
        assert_eq!(disk.state(), Open);
    }
}
