use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, UpDownCounter};
use parking_lot::Mutex;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const WORKLOAD_CLASS_COUNT: usize = 4;

/// Queries one tenant may have admitted at once.
///
/// A dashboard opens its panels together, so a workspace needs a few queries
/// in flight; capping them per tenant keeps one workspace from filling the
/// process-wide query capacity while another waits. Each project's DuckDB
/// reader pool holds at least this many connections, so an admitted query
/// never queues again for a reader.
pub const QUERIES_PER_TENANT: usize = 4;

/// How long a query waits for a slot before it is rejected with 429.
///
/// A burst (a dashboard loading every panel) queues for a moment instead of
/// failing; only sustained overload, where slots stay busy this long, is
/// rejected.
const QUERY_ADMISSION_WAIT: Duration = Duration::from_secs(5);

/// Process-wide query capacity for `cores` available CPUs: at least one
/// tenant's worth so a single workspace's panels run side by side, at most 8.
/// DuckDB runs each query on its own reader connection, and its memory limit
/// and the service's cgroup bound what those queries use together.
fn query_capacity(cores: usize) -> usize {
    cores.clamp(QUERIES_PER_TENANT, 8)
}

/// A bounded class of expensive work.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WorkloadClass {
    Query,
    Transform,
    ProjectInit,
    Maintenance,
}

impl WorkloadClass {
    const fn index(self) -> usize {
        match self {
            Self::Query => 0,
            Self::Transform => 1,
            Self::ProjectInit => 2,
            Self::Maintenance => 3,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Transform => "transform",
            Self::ProjectInit => "project_init",
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
            Self::Maintenance => "maintenance",
        };
        formatter.write_str(name)
    }
}

/// Which bound rejected a workload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionScope {
    /// The class's process-wide capacity.
    Process,
    /// The per-tenant query capacity ([`QUERIES_PER_TENANT`]).
    Tenant,
}

impl fmt::Display for AdmissionScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Process => "capacity",
            Self::Tenant => "per-tenant capacity",
        })
    }
}

/// The reason a workload could not be admitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{workload} workload admission rejected: {scope} {capacity} is in use")]
pub struct AdmissionError {
    /// The class whose bound was reached.
    pub workload: WorkloadClass,
    /// Whether the process-wide or the per-tenant bound was reached.
    pub scope: AdmissionScope,
    /// The maximum number of concurrent operations under that bound.
    pub capacity: usize,
}

/// A permit that keeps one unit of a workload class admitted.
///
/// Dropping the permit releases the unit back to its class automatically.
#[derive(Debug)]
pub struct WorkloadPermit {
    workload: WorkloadClass,
    _class_permit: OwnedSemaphorePermit,
    _tenant_slot: Option<TenantSlot>,
    in_use_metric: UpDownCounter<i64>,
}

/// One tenant's claim on its query semaphore.
///
/// Dropping it releases the permit and, when nothing else holds or waits on
/// the semaphore, removes the tenant's entry so the map only holds tenants
/// with queries in flight.
#[derive(Debug)]
struct TenantSlot {
    state: Arc<WorkloadState>,
    tenant: String,
    semaphore: Arc<Semaphore>,
    permit: Option<OwnedSemaphorePermit>,
}

impl Drop for TenantSlot {
    fn drop(&mut self) {
        drop(self.permit.take());
        let mut tenants = self.state.tenants.lock();
        // The map and this slot are the only owners: no admitted query (each
        // permit owns a clone) and no waiter (each `acquire_owned` future owns
        // one) is left. New claims clone under this same lock, so the count
        // cannot grow while it is held.
        if Arc::strong_count(&self.semaphore) == 2 {
            tenants.remove(&self.tenant);
        }
    }
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
    tenants: Mutex<HashMap<String, Arc<Semaphore>>>,
    rejected: [AtomicUsize; WORKLOAD_CLASS_COUNT],
    capacities: [usize; WORKLOAD_CLASS_COUNT],
    in_use_metric: UpDownCounter<i64>,
    rejected_metric: Counter<u64>,
}

/// A reusable concurrency budget for expensive workloads.
///
/// Each class has an independent semaphore. Ingest transforms, project init
/// and maintenance use [`WorkloadBudget::try_acquire`], which fails
/// immediately rather than queueing work that can amplify runtime contention.
/// Queries use [`WorkloadBudget::acquire_query`]: at most
/// [`QUERIES_PER_TENANT`] per tenant, and a short bounded wait for a slot so
/// a burst of panels queues briefly instead of failing.
///
/// Queries and transforms used to share one global budget of `cores`. On a
/// one-CPU host that admitted a single operation at a time, so a dashboard's
/// second query, or an ingest batch arriving during a query, was rejected.
/// The classes are now bounded separately; the host's CPU quota and DuckDB's
/// own limits bound what they use together.
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
        let capacities = [query_capacity(cores), cores, (cores / 2).clamp(1, 4), 1];

        Self::with_capacities(capacities)
    }

    fn with_capacities(capacities: [usize; WORKLOAD_CLASS_COUNT]) -> Self {
        let semaphores = capacities.map(|capacity| Arc::new(Semaphore::new(capacity)));
        let meter = opentelemetry::global::meter("sigiro.workload");
        Self {
            state: Arc::new(WorkloadState {
                semaphores,
                tenants: Mutex::new(HashMap::new()),
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
            .map_err(|_| self.reject(workload, AdmissionScope::Process))?;
        Ok(self.admit(workload, class_permit, None))
    }

    /// Admits one query for `tenant`, waiting up to a few seconds for a slot.
    ///
    /// The tenant's own slot ([`QUERIES_PER_TENANT`]) is taken first, so a
    /// tenant's extra queries wait behind its own and never queue ahead of
    /// another tenant's for the process-wide slots.
    pub async fn acquire_query(&self, tenant: &str) -> Result<WorkloadPermit, AdmissionError> {
        self.acquire_query_within(tenant, QUERY_ADMISSION_WAIT)
            .await
    }

    async fn acquire_query_within(
        &self,
        tenant: &str,
        wait: Duration,
    ) -> Result<WorkloadPermit, AdmissionError> {
        let workload = WorkloadClass::Query;
        let deadline = tokio::time::Instant::now() + wait;
        let mut slot = self.tenant_slot(tenant);
        let tenant_permit =
            tokio::time::timeout_at(deadline, Arc::clone(&slot.semaphore).acquire_owned())
                .await
                .ok()
                .and_then(Result::ok)
                .ok_or_else(|| self.reject(workload, AdmissionScope::Tenant))?;
        slot.permit = Some(tenant_permit);
        let class_permit = tokio::time::timeout_at(
            deadline,
            Arc::clone(&self.state.semaphores[workload.index()]).acquire_owned(),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or_else(|| self.reject(workload, AdmissionScope::Process))?;
        Ok(self.admit(workload, class_permit, Some(slot)))
    }

    /// Claims `tenant`'s query semaphore, creating it on first use.
    fn tenant_slot(&self, tenant: &str) -> TenantSlot {
        let semaphore = Arc::clone(
            self.state
                .tenants
                .lock()
                .entry(tenant.to_owned())
                .or_insert_with(|| Arc::new(Semaphore::new(QUERIES_PER_TENANT))),
        );
        TenantSlot {
            state: Arc::clone(&self.state),
            tenant: tenant.to_owned(),
            semaphore,
            permit: None,
        }
    }

    fn admit(
        &self,
        workload: WorkloadClass,
        class_permit: OwnedSemaphorePermit,
        tenant_slot: Option<TenantSlot>,
    ) -> WorkloadPermit {
        self.state
            .in_use_metric
            .add(1, &[KeyValue::new("class", workload.name())]);
        WorkloadPermit {
            workload,
            _class_permit: class_permit,
            _tenant_slot: tenant_slot,
            in_use_metric: self.state.in_use_metric.clone(),
        }
    }

    fn reject(&self, workload: WorkloadClass, scope: AdmissionScope) -> AdmissionError {
        let index = workload.index();
        self.state.rejected[index].fetch_add(1, Ordering::Relaxed);
        self.state
            .rejected_metric
            .add(1, &[KeyValue::new("class", workload.name())]);
        AdmissionError {
            workload,
            scope,
            capacity: match scope {
                AdmissionScope::Process => self.state.capacities[index],
                AdmissionScope::Tenant => QUERIES_PER_TENANT,
            },
        }
    }

    /// Returns the configured capacity for one workload class.
    pub fn capacity(&self, workload: WorkloadClass) -> usize {
        self.state.capacities[workload.index()]
    }

    /// Returns the number of currently admitted operations in one class.
    pub fn in_use(&self, workload: WorkloadClass) -> usize {
        self.capacity(workload) - self.state.semaphores[workload.index()].available_permits()
    }

    /// Returns the number of admission rejections for one class.
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
                    3 => WorkloadClass::Maintenance,
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

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: Duration = Duration::from_millis(50);

    fn budget(query_capacity: usize) -> WorkloadBudget {
        WorkloadBudget::with_capacities([query_capacity, 1, 1, 1])
    }

    async fn fill(budget: &WorkloadBudget, tenant: &str, count: usize) -> Vec<WorkloadPermit> {
        let mut permits = Vec::new();
        for _ in 0..count {
            permits.push(
                budget
                    .acquire_query_within(tenant, SHORT)
                    .await
                    .expect("slot is free"),
            );
        }
        permits
    }

    #[test]
    fn query_capacity_admits_one_tenant_and_stays_bounded() {
        assert_eq!(query_capacity(1), QUERIES_PER_TENANT);
        assert_eq!(query_capacity(2), QUERIES_PER_TENANT);
        assert_eq!(query_capacity(6), 6);
        assert_eq!(query_capacity(64), 8);
    }

    #[tokio::test]
    async fn tenant_runs_four_queries_and_the_fifth_is_rejected_after_waiting() {
        let budget = budget(8);
        let _held = fill(&budget, "a", QUERIES_PER_TENANT).await;

        let started = tokio::time::Instant::now();
        let error = budget
            .acquire_query_within("a", SHORT)
            .await
            .expect_err("tenant is at its cap");
        assert!(started.elapsed() >= SHORT);
        assert_eq!(
            error,
            AdmissionError {
                workload: WorkloadClass::Query,
                scope: AdmissionScope::Tenant,
                capacity: QUERIES_PER_TENANT,
            }
        );
        assert_eq!(budget.rejected(WorkloadClass::Query), 1);

        // Another tenant is not held back by the first one's cap.
        budget
            .acquire_query_within("b", SHORT)
            .await
            .expect("other tenant has its own slots");
    }

    #[tokio::test]
    async fn queued_query_starts_when_a_slot_frees() {
        let budget = budget(8);
        let mut held = fill(&budget, "a", QUERIES_PER_TENANT).await;

        let waiter = {
            let budget = budget.clone();
            tokio::spawn(async move {
                budget
                    .acquire_query_within("a", Duration::from_secs(5))
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished(), "fifth query waits for a slot");
        drop(held.pop());

        let permit = waiter.await.expect("waiter task").expect("slot freed");
        assert_eq!(permit.workload(), WorkloadClass::Query);
        assert_eq!(budget.rejected(WorkloadClass::Query), 0);
    }

    #[tokio::test]
    async fn process_capacity_bounds_queries_across_tenants() {
        let budget = budget(2);
        let _a = fill(&budget, "a", 1).await;
        let _b = fill(&budget, "b", 1).await;

        let error = budget
            .acquire_query_within("c", SHORT)
            .await
            .expect_err("process capacity is in use");
        assert_eq!(error.scope, AdmissionScope::Process);
        assert_eq!(error.capacity, 2);
        assert_eq!(budget.in_use(WorkloadClass::Query), 2);
    }

    #[tokio::test]
    async fn queries_do_not_take_ingest_capacity() {
        let budget = budget(8);
        let _queries = fill(&budget, "a", QUERIES_PER_TENANT).await;

        budget
            .try_acquire(WorkloadClass::Transform)
            .expect("transform is bounded separately from queries");
    }

    #[tokio::test]
    async fn tenant_entries_are_dropped_once_idle() {
        let budget = budget(8);
        let held = fill(&budget, "a", QUERIES_PER_TENANT).await;
        budget
            .acquire_query_within("a", SHORT)
            .await
            .expect_err("tenant is at its cap");
        assert_eq!(budget.state.tenants.lock().len(), 1);

        drop(held);
        assert!(budget.state.tenants.lock().is_empty());
        assert_eq!(budget.in_use(WorkloadClass::Query), 0);
    }
}
