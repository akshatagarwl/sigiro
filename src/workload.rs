use std::fmt;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, UpDownCounter};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const WORKLOAD_CLASS_COUNT: usize = 5;

/// A bounded class of expensive work.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WorkloadClass {
    Query,
    Transform,
    ProjectInit,
    Anomaly,
    Maintenance,
}

impl WorkloadClass {
    const fn index(self) -> usize {
        match self {
            Self::Query => 0,
            Self::Transform => 1,
            Self::ProjectInit => 2,
            Self::Anomaly => 3,
            Self::Maintenance => 4,
        }
    }

    const fn uses_global_cpu_budget(self) -> bool {
        matches!(self, Self::Query | Self::Transform | Self::Anomaly)
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Transform => "transform",
            Self::ProjectInit => "project_init",
            Self::Anomaly => "anomaly",
            Self::Maintenance => "maintenance",
        }
    }
}

impl fmt::Display for WorkloadClass {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Query => "query",
            Self::Transform => "transform",
            Self::ProjectInit => "project init",
            Self::Anomaly => "anomaly",
            Self::Maintenance => "maintenance",
        };
        formatter.write_str(name)
    }
}

/// The reason a workload could not start immediately.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{workload} workload admission rejected: capacity {capacity} is in use")]
pub struct AdmissionError {
    /// The class whose bound was reached.
    pub workload: WorkloadClass,
    /// The maximum number of concurrent operations in the class.
    pub capacity: usize,
}

/// A permit that keeps one unit of a workload class admitted.
///
/// Dropping the permit releases the unit back to its class automatically.
#[derive(Debug)]
pub struct WorkloadPermit {
    workload: WorkloadClass,
    _class_permit: OwnedSemaphorePermit,
    _global_permit: Option<OwnedSemaphorePermit>,
    in_use_metric: UpDownCounter<i64>,
}

impl WorkloadPermit {
    /// Returns the class held by this permit.
    pub const fn workload(&self) -> WorkloadClass {
        self.workload
    }
}

impl Drop for WorkloadPermit {
    fn drop(&mut self) {
        self.in_use_metric
            .add(-1, &[KeyValue::new("class", self.workload.name())]);
    }
}

#[derive(Debug)]
struct WorkloadState {
    semaphores: [Arc<Semaphore>; WORKLOAD_CLASS_COUNT],
    global: Arc<Semaphore>,
    global_capacity: usize,
    rejected: [AtomicUsize; WORKLOAD_CLASS_COUNT],
    capacities: [usize; WORKLOAD_CLASS_COUNT],
    in_use_metric: UpDownCounter<i64>,
    rejected_metric: Counter<u64>,
}

/// A reusable, non-queuing concurrency budget for expensive workloads.
///
/// Each class has an independent semaphore. Admission uses `try_acquire_owned`
/// so callers receive an error immediately rather than adding more work to a
/// queue that can amplify runtime contention.
#[derive(Clone, Debug)]
pub struct WorkloadBudget {
    state: Arc<WorkloadState>,
}

/// Current admission counters for all workload classes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkloadSnapshot {
    stats: [WorkloadStats; WORKLOAD_CLASS_COUNT],
}

/// Capacity and counters for one workload class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkloadStats {
    pub capacity: usize,
    pub in_use: usize,
    pub rejected: usize,
}

impl WorkloadBudget {
    /// Creates a budget sized once from this process's available parallelism.
    ///
    /// The operating system should always provide this value; an unavailable
    /// value is treated as one core so admission remains bounded.
    pub fn new() -> Self {
        let cores = std::thread::available_parallelism()
            .unwrap_or(NonZeroUsize::MIN)
            .get();
        let capacities = [
            cores.min(8),
            cores,
            (cores / 2).clamp(1, 4),
            (cores / 4).clamp(1, 2),
            1,
        ];

        Self::with_capacities_and_global(capacities, cores)
    }

    fn with_capacities_and_global(
        capacities: [usize; WORKLOAD_CLASS_COUNT],
        global_capacity: usize,
    ) -> Self {
        let semaphores = capacities.map(|capacity| Arc::new(Semaphore::new(capacity)));
        let meter = opentelemetry::global::meter("sigiro.workload");
        Self {
            state: Arc::new(WorkloadState {
                semaphores,
                global: Arc::new(Semaphore::new(global_capacity)),
                global_capacity,
                rejected: std::array::from_fn(|_| AtomicUsize::new(0)),
                capacities,
                in_use_metric: meter.i64_up_down_counter("sigiro.workload.in_use").build(),
                rejected_metric: meter.u64_counter("sigiro.workload.rejected").build(),
            }),
        }
    }

    /// Attempts to admit one operation without waiting.
    pub fn try_acquire(&self, workload: WorkloadClass) -> Result<WorkloadPermit, AdmissionError> {
        let index = workload.index();
        let class_permit = Arc::clone(&self.state.semaphores[index])
            .try_acquire_owned()
            .map_err(|_| {
                self.reject(workload);
                AdmissionError {
                    workload,
                    capacity: self.state.capacities[index],
                }
            })?;
        let global_permit = if workload.uses_global_cpu_budget() {
            Some(
                Arc::clone(&self.state.global)
                    .try_acquire_owned()
                    .map_err(|_| {
                        self.reject(workload);
                        AdmissionError {
                            workload,
                            capacity: self.state.global_capacity,
                        }
                    })?,
            )
        } else {
            None
        };

        self.state
            .in_use_metric
            .add(1, &[KeyValue::new("class", workload.name())]);
        Ok(WorkloadPermit {
            workload,
            _class_permit: class_permit,
            _global_permit: global_permit,
            in_use_metric: self.state.in_use_metric.clone(),
        })
    }

    fn reject(&self, workload: WorkloadClass) {
        let index = workload.index();
        self.state.rejected[index].fetch_add(1, Ordering::Relaxed);
        self.state
            .rejected_metric
            .add(1, &[KeyValue::new("class", workload.name())]);
    }

    /// Returns the process-wide cap shared by all expensive workload classes.
    pub fn global_capacity(&self) -> usize {
        self.state.global_capacity
    }

    /// Returns the configured capacity for one workload class.
    pub fn capacity(&self, workload: WorkloadClass) -> usize {
        self.state.capacities[workload.index()]
    }

    /// Returns the number of currently admitted operations in one class.
    pub fn in_use(&self, workload: WorkloadClass) -> usize {
        self.capacity(workload) - self.state.semaphores[workload.index()].available_permits()
    }

    /// Returns the number of immediate admission rejections for one class.
    pub fn rejected(&self, workload: WorkloadClass) -> usize {
        self.state.rejected[workload.index()].load(Ordering::Relaxed)
    }

    /// Reads capacity and counters for every class without allocating.
    pub fn snapshot(&self) -> WorkloadSnapshot {
        WorkloadSnapshot {
            stats: std::array::from_fn(|index| {
                let workload = match index {
                    0 => WorkloadClass::Query,
                    1 => WorkloadClass::Transform,
                    2 => WorkloadClass::ProjectInit,
                    3 => WorkloadClass::Anomaly,
                    4 => WorkloadClass::Maintenance,
                    _ => unreachable!("workload class index is bounded by the class count"),
                };
                WorkloadStats {
                    capacity: self.capacity(workload),
                    in_use: self.in_use(workload),
                    rejected: self.rejected(workload),
                }
            }),
        }
    }

    /// Returns the snapshot entry for one class.
    pub fn stats(&self, workload: WorkloadClass) -> WorkloadStats {
        self.snapshot().stats[workload.index()]
    }
}

impl Default for WorkloadBudget {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkloadSnapshot {
    /// Returns the snapshot entry for one class.
    pub fn stats(self, workload: WorkloadClass) -> WorkloadStats {
        self.stats[workload.index()]
    }
}
