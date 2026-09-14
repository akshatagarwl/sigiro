use opentelemetry::metrics::Gauge;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_metrics::{RuntimeMetrics, RuntimeMonitor};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

/// Stops the runtime sampler before the OpenTelemetry provider is shut down.
pub struct RuntimeMetricsGuard {
    task: JoinHandle<()>,
}

impl Drop for RuntimeMetricsGuard {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Instruments {
    workers: Gauge<u64>,
    live_tasks: Gauge<u64>,
    global_queue_depth: Gauge<u64>,
    worker_busy_ratio: Gauge<f64>,
    worker_busy_seconds: Gauge<f64>,
    worker_max_busy_seconds: Gauge<f64>,
    worker_min_busy_seconds: Gauge<f64>,
    worker_parks: Gauge<u64>,
}

impl Instruments {
    fn new() -> Self {
        let meter = opentelemetry::global::meter("sigiro.runtime");
        Self {
            workers: meter.u64_gauge("sigiro.runtime.workers").build(),
            live_tasks: meter.u64_gauge("sigiro.runtime.live_tasks").build(),
            global_queue_depth: meter.u64_gauge("sigiro.runtime.global_queue_depth").build(),
            worker_busy_ratio: meter.f64_gauge("sigiro.runtime.worker_busy_ratio").build(),
            worker_busy_seconds: meter
                .f64_gauge("sigiro.runtime.worker_busy_seconds")
                .build(),
            worker_max_busy_seconds: meter
                .f64_gauge("sigiro.runtime.worker_max_busy_seconds")
                .build(),
            worker_min_busy_seconds: meter
                .f64_gauge("sigiro.runtime.worker_min_busy_seconds")
                .build(),
            worker_parks: meter.u64_gauge("sigiro.runtime.worker_parks").build(),
        }
    }

    fn record(&self, sample: &RuntimeMetrics) {
        let workers = sample.workers_count as u64;
        let available_worker_seconds = sample.elapsed.as_secs_f64() * workers as f64;
        let busy_ratio = if available_worker_seconds > 0.0 {
            (sample.total_busy_duration.as_secs_f64() / available_worker_seconds).clamp(0.0, 1.0)
        } else {
            0.0
        };

        self.workers.record(workers, &[]);
        self.live_tasks.record(sample.live_tasks_count as u64, &[]);
        self.global_queue_depth
            .record(sample.global_queue_depth as u64, &[]);
        self.worker_busy_ratio.record(busy_ratio, &[]);
        self.worker_busy_seconds
            .record(sample.total_busy_duration.as_secs_f64(), &[]);
        self.worker_max_busy_seconds
            .record(sample.max_busy_duration.as_secs_f64(), &[]);
        self.worker_min_busy_seconds
            .record(sample.min_busy_duration.as_secs_f64(), &[]);
        self.worker_parks.record(sample.total_park_count, &[]);
    }
}

/// Samples stable Tokio runtime metrics. Task scheduling delay and blocking-pool
/// depth require Tokio's unstable metrics API; workload-specific admission
/// metrics cover those queues without making the production build depend on it.
pub fn spawn() -> RuntimeMetricsGuard {
    let monitor = RuntimeMonitor::new(&tokio::runtime::Handle::current());
    let task = tokio::spawn(async move {
        let instruments = Instruments::new();
        let mut intervals = monitor.intervals();
        let _baseline = intervals.next();
        let mut ticker = tokio::time::interval(SAMPLE_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;

        loop {
            ticker.tick().await;
            let Some(sample) = intervals.next() else {
                tracing::error!("Tokio runtime metrics interval ended unexpectedly");
                return;
            };
            instruments.record(&sample);
        }
    });
    RuntimeMetricsGuard { task }
}
