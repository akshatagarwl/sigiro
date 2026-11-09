//! Self-profiling over OTLP.
//!
//! With `SIGIRO_SELF_PROFILING=true`, sigiro samples its own CPU (pprof-rs, 100
//! Hz) and, when jemalloc heap profiling is enabled, its in-use heap, and every
//! 10 seconds sends both as an OTLP `ExportProfilesServiceRequest` to:
//!
//!   1. `OTEL_EXPORTER_OTLP_PROFILES_ENDPOINT`, used as-is, when set;
//!   2. else `OTEL_EXPORTER_OTLP_ENDPOINT` + `/v1development/profiles`;
//!   3. else this server's own OTLP/HTTP listener on 127.0.0.1.
//!
//! Requests carry `Authorization: Bearer $SIGIRO_OTEL_AUTH_KEY` when that key
//! is set, like the rest of the self-instrumentation (see `src/otel.rs`).
//! Profiles are tagged `service.name=$OTEL_SERVICE_NAME` (default `sigiro`).
//!
//! Heap profiles need jemalloc's sampler, which only starts with the process:
//! run with `MALLOC_CONF=prof:true,prof_active:true,lg_prof_sample:19` (on
//! macOS `_RJEM_MALLOC_CONF`). Without it only CPU is sent.

use std::collections::HashMap;
use std::io::BufReader;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use sigiro_segments::profile_export::{
    ExportFrame, ExportProfile, ExportRequest, ExportSample, encode_profiles_request,
};
use tracing_attributes::instrument;

/// How often a profile is collected and sent.
const WINDOW: Duration = Duration::from_secs(10);
/// CPU sampling frequency (Hz).
const CPU_HZ: i32 = 100;
const CPU_PERIOD_NANOS: i64 = 1_000_000_000 / CPU_HZ as i64;
/// pprof-rs only accumulates; each window reports the growth since the last
/// one. Its collector spills evicted stacks to a temp file and a report
/// resolves every stack seen so far, so it is replaced every 30 windows (5
/// minutes) to bound both. Replacing it every window would allocate its ~80
/// MiB table every 10 seconds.
const CPU_WINDOWS_PER_GUARD: u32 = 30;
/// Leaf frames from the sampler or allocator themselves, not the program,
/// matched after leading underscores (macOS adds them to C symbols).
const SAMPLER_FRAME_PREFIXES: &[&str] = &[
    "backtrace::",
    "pprof::",
    "<pprof::",
    "rjem_",
    "je_",
    "prof_",
    "malloc",
    "mallocx",
    "rallocx",
    "do_rallocx",
    "calloc",
    "rust_alloc",
    "rust_realloc",
    "rdl_",
    "tikv_jemallocator::",
    "<tikv_jemallocator::",
];
/// Inlined standard-library allocation frames, which carry only a short name
/// (compared without generic arguments).
const ALLOCATOR_FRAMES: &[&str] = &[
    "alloc",
    "alloc_zeroed",
    "alloc_impl",
    "alloc_impl_runtime",
    "allocate",
    "allocate_in",
    "try_allocate_in",
    "realloc",
    "grow_impl",
    "grow_one",
    "grow_amortized",
    "finish_grow",
    "box_new_uninit",
];

/// Running self-profiler. Dropping it stops profiling.
pub struct ProfilingHandle {
    _stop: mpsc::Sender<()>,
}

/// Start self-profiling if `SIGIRO_SELF_PROFILING` is enabled. Returns `None`
/// when it is not.
#[instrument(skip_all)]
pub fn start_profiling() -> Result<Option<ProfilingHandle>> {
    let enabled = crate::config::SELF_PROFILING
        .value()
        .map_err(anyhow::Error::from)?;
    if !matches!(enabled.trim(), "true" | "1") {
        return Ok(None);
    }
    let endpoint = profiles_endpoint(
        &crate::config::OTEL_EXPORTER_OTLP_PROFILES_ENDPOINT
            .value()
            .unwrap_or_default(),
        &crate::config::OTEL_EXPORTER_OTLP_ENDPOINT
            .value()
            .unwrap_or_default(),
        crate::config::OTLP_HTTP_PORT
            .value()
            .map_err(anyhow::Error::from)?,
    );
    let service_name = crate::config::OTEL_SERVICE_NAME
        .value()
        .map_err(anyhow::Error::from)?;
    let auth_key = crate::otel::otel_auth_key_from_env()?;
    let authorization = auth_key
        .as_ref()
        .map(|key| format!("Bearer {}", key.as_ref()));
    let cpu = CpuSampler::new()?;
    let heap_enabled = heap_profiling_active();

    tracing::info!(
        endpoint = %endpoint,
        service = %service_name,
        cpu_hz = CPU_HZ,
        heap = heap_enabled,
        auth_configured = auth_key.is_some(),
        "self-profiling enabled"
    );
    if !heap_enabled {
        tracing::info!(
            "self-profiling sends CPU only; start with MALLOC_CONF=prof:true,prof_active:true,lg_prof_sample:19 to add heap"
        );
    }

    let (stop, stopped) = mpsc::channel();
    let exporter = Exporter {
        endpoint,
        authorization,
        resource: vec![
            ("service.name".to_string(), service_name),
            (
                "service.version".to_string(),
                env!("CARGO_PKG_VERSION").to_string(),
            ),
            ("process.pid".to_string(), std::process::id().to_string()),
        ],
    };
    std::thread::Builder::new()
        .name("self-profiler".into())
        .spawn(move || run(cpu, heap_enabled, &exporter, &stopped))
        .context("failed to start the self-profiling thread")?;
    Ok(Some(ProfilingHandle { _stop: stop }))
}

/// Where profiles go: the profiles endpoint as given, else the OTLP base
/// endpoint's profiles path, else this server's own OTLP/HTTP listener.
fn profiles_endpoint(profiles: &str, base: &str, otlp_http_port: u16) -> String {
    let profiles = profiles.trim();
    if !profiles.is_empty() {
        return profiles.to_string();
    }
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        format!("http://127.0.0.1:{otlp_http_port}/v1development/profiles")
    } else {
        format!("{base}/v1development/profiles")
    }
}

struct Exporter {
    endpoint: String,
    authorization: Option<String>,
    resource: Vec<(String, String)>,
}

fn run(mut cpu: CpuSampler, heap: bool, exporter: &Exporter, stopped: &mpsc::Receiver<()>) {
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(%error, "self-profiling stopped: cannot build the HTTP client");
            return;
        }
    };
    let mut window_start = SystemTime::now();
    let mut failures = 0u32;
    loop {
        match stopped.recv_timeout(WINDOW) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
        }
        let now = SystemTime::now();
        let start_nanos = unix_nanos(window_start);
        let duration_nanos = u64::try_from(
            now.duration_since(window_start)
                .unwrap_or_default()
                .as_nanos(),
        )
        .unwrap_or(u64::MAX);
        window_start = now;

        let mut profiles = Vec::with_capacity(2);
        match cpu.collect() {
            Ok(samples) if !samples.is_empty() => profiles.push(ExportProfile {
                sample_type: ("cpu".into(), "nanoseconds".into()),
                period_type: ("cpu".into(), "nanoseconds".into()),
                period: CPU_PERIOD_NANOS,
                time_unix_nano: start_nanos,
                duration_nano: duration_nanos,
                samples,
            }),
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "self-profiling: CPU profile failed"),
        }
        if heap {
            match heap_samples() {
                Ok(samples) if !samples.is_empty() => profiles.push(ExportProfile {
                    sample_type: ("inuse_space".into(), "bytes".into()),
                    period_type: ("space".into(), "bytes".into()),
                    period: 0,
                    time_unix_nano: unix_nanos(now),
                    duration_nano: 0,
                    samples,
                }),
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "self-profiling: heap profile failed"),
            }
        }
        if profiles.is_empty() {
            continue;
        }
        let body = encode_profiles_request(&ExportRequest {
            resource: exporter.resource.clone(),
            scope_name: "sigiro".into(),
            scope_version: env!("CARGO_PKG_VERSION").into(),
            profiles,
        });
        let mut request = client
            .post(&exporter.endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/x-protobuf")
            .body(body);
        if let Some(authorization) = &exporter.authorization {
            request = request.header(reqwest::header::AUTHORIZATION, authorization);
        }
        match request
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
        {
            Ok(_) => failures = 0,
            Err(error) => {
                failures = failures.saturating_add(1);
                // The first failure, then one in 30 (every 5 minutes), so a
                // missing endpoint does not flood the log.
                if failures % 30 == 1 {
                    tracing::warn!(%error, endpoint = %exporter.endpoint, failures, "self-profiling: export failed");
                }
            }
        }
    }
}

fn unix_nanos(time: SystemTime) -> u64 {
    u64::try_from(
        time.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    )
    .unwrap_or(u64::MAX)
}

/// Drop the sampler's or allocator's own frames from the leaf end.
fn trim_sampler_frames(frames: &mut Vec<ExportFrame>) {
    let keep_from = frames
        .iter()
        .position(|frame| {
            let name = frame.function.trim_start_matches('_');
            let short = name.split('<').next().unwrap_or(name);
            !(ALLOCATOR_FRAMES.contains(&short)
                || SAMPLER_FRAME_PREFIXES
                    .iter()
                    .any(|prefix| name.starts_with(prefix)))
        })
        .unwrap_or(frames.len());
    frames.drain(..keep_from);
}

fn export_frame(symbol: &pprof::Symbol) -> ExportFrame {
    ExportFrame {
        function: symbol.name(),
        file: symbol
            .filename
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        line: i64::from(symbol.lineno.unwrap_or(0)),
    }
}

type StackKey = (String, Vec<ExportFrame>);

struct CpuSampler {
    guard: Option<pprof::ProfilerGuard<'static>>,
    previous: HashMap<StackKey, isize>,
    windows: u32,
}

impl CpuSampler {
    fn new() -> Result<Self> {
        Ok(Self {
            guard: Some(new_guard()?),
            previous: HashMap::new(),
            windows: 0,
        })
    }

    /// CPU samples since the previous call.
    fn collect(&mut self) -> Result<Vec<ExportSample>> {
        let report = match &self.guard {
            Some(guard) => guard.report().build().context("pprof report")?,
            None => {
                self.guard = Some(new_guard()?);
                return Ok(Vec::new());
            }
        };
        let mut current: HashMap<StackKey, isize> = HashMap::with_capacity(report.data.len());
        for (frames, count) in &report.data {
            // Leaf first; within a frame, inlined functions come first.
            let mut stack: Vec<ExportFrame> =
                frames.frames.iter().flatten().map(export_frame).collect();
            trim_sampler_frames(&mut stack);
            if stack.is_empty() {
                continue;
            }
            *current
                .entry((frames.thread_name.clone(), stack))
                .or_default() += *count;
        }
        let samples = current
            .iter()
            .filter_map(|(key, &count)| {
                let delta = count - self.previous.get(key).copied().unwrap_or(0);
                (delta > 0).then(|| ExportSample {
                    frames: key.1.clone(),
                    value: i64::try_from(delta)
                        .unwrap_or(i64::MAX)
                        .saturating_mul(CPU_PERIOD_NANOS),
                    attributes: thread_attributes(&key.0),
                })
            })
            .collect();
        self.previous = current;
        self.windows += 1;
        if self.windows >= CPU_WINDOWS_PER_GUARD {
            // Only one profiler may run, so the old guard goes first.
            self.guard = None;
            self.previous.clear();
            self.windows = 0;
            self.guard = Some(new_guard()?);
        }
        Ok(samples)
    }
}

/// The thread name as a sample attribute. An unnamed thread reports its
/// numeric id instead, which is left out.
fn thread_attributes(thread: &str) -> Vec<(String, String)> {
    if thread.is_empty() || thread.bytes().all(|b| b.is_ascii_digit()) {
        Vec::new()
    } else {
        vec![("thread.name".to_string(), thread.to_string())]
    }
}

fn new_guard() -> Result<pprof::ProfilerGuard<'static>> {
    pprof::ProfilerGuardBuilder::default()
        .frequency(CPU_HZ)
        .blocklist(&["libc", "libgcc", "pthread", "vdso"])
        .build()
        .context("failed to start the CPU profiler")
}

/// Whether jemalloc's heap sampler is on. Called from async startup, so it
/// must not block on the (tokio) lock; nothing else holds it yet.
fn heap_profiling_active() -> bool {
    jemalloc_pprof::PROF_CTL
        .as_ref()
        .is_some_and(|ctl| ctl.try_lock().is_ok_and(|ctl| ctl.activated()))
}

/// The sampled in-use heap, by allocating stack.
fn heap_samples() -> Result<Vec<ExportSample>> {
    let Some(ctl) = jemalloc_pprof::PROF_CTL.as_ref() else {
        return Ok(Vec::new());
    };
    let dump = ctl.blocking_lock().dump().context("jemalloc heap dump")?;
    let profile = pprof_util::parse_jeheap(BufReader::new(dump), None)?;
    let mut symbols: HashMap<usize, Vec<ExportFrame>> = HashMap::new();
    let mut stacks: HashMap<Vec<ExportFrame>, f64> = HashMap::new();
    for (stack, _) in &profile.stacks {
        // parse_jeheap returns callers first; the addresses are return
        // addresses, so the call is the instruction before each one.
        let mut frames = Vec::with_capacity(stack.addrs.len());
        for &addr in stack.addrs.iter().rev() {
            frames.extend(
                symbols
                    .entry(addr)
                    .or_insert_with(|| resolve_address(addr.saturating_sub(1)))
                    .iter()
                    .cloned(),
            );
        }
        trim_sampler_frames(&mut frames);
        if !frames.is_empty() {
            *stacks.entry(frames).or_default() += stack.weight;
        }
    }
    Ok(stacks
        .into_iter()
        .filter_map(|(frames, bytes)| {
            #[expect(clippy::cast_possible_truncation, reason = "heap sizes fit in i64")]
            let value = bytes.round() as i64;
            (value > 0).then(|| ExportSample {
                frames,
                value,
                attributes: Vec::new(),
            })
        })
        .collect())
}

/// Symbolize one code address: inlined functions first, then their caller.
fn resolve_address(addr: usize) -> Vec<ExportFrame> {
    let mut frames = Vec::new();
    backtrace::resolve(addr as *mut std::ffi::c_void, |symbol| {
        frames.push(export_frame(&pprof::Symbol {
            name: symbol.name().map(|name| name.as_bytes().to_vec()),
            addr: symbol.addr(),
            lineno: symbol.lineno(),
            filename: symbol.filename().map(std::path::Path::to_path_buf),
        }));
    });
    if frames.is_empty() {
        frames.push(ExportFrame {
            function: format!("0x{addr:x}"),
            file: String::new(),
            line: 0,
        });
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(function: &str) -> ExportFrame {
        ExportFrame {
            function: function.into(),
            file: String::new(),
            line: 0,
        }
    }

    #[test]
    fn the_profiles_endpoint_prefers_the_specific_then_the_base_then_itself() {
        assert_eq!(
            profiles_endpoint(" https://p.example/x ", "https://o.example", 4318),
            "https://p.example/x"
        );
        assert_eq!(
            profiles_endpoint("", "https://o.example/", 4318),
            "https://o.example/v1development/profiles"
        );
        assert_eq!(
            profiles_endpoint("", "", 14318),
            "http://127.0.0.1:14318/v1development/profiles"
        );
    }

    #[test]
    fn sampler_and_allocator_frames_are_trimmed_from_the_leaf() {
        let mut frames = vec![
            frame("backtrace::backtrace::trace_unsynchronized"),
            frame("pprof::profiler::perf_signal_handler"),
            frame("__rjem_je_prof_backtrace"),
            frame("__rjem_je_malloc_default"),
            frame("alloc"),
            frame("__rust_alloc"),
            frame("allocate"),
            frame("_do_rallocx"),
            frame("finish_grow<alloc::alloc::Global>"),
            frame("sigiro::query::run"),
            frame("je_malloc_default"),
            frame("main"),
        ];
        trim_sampler_frames(&mut frames);
        assert_eq!(
            frames,
            [
                frame("sigiro::query::run"),
                frame("je_malloc_default"),
                frame("main")
            ]
        );
    }

    #[test]
    fn numeric_thread_names_are_not_labels() {
        assert!(thread_attributes("6159593472").is_empty());
        assert!(thread_attributes("").is_empty());
        assert_eq!(
            thread_attributes("tokio-runtime-worker"),
            [(
                "thread.name".to_string(),
                "tokio-runtime-worker".to_string()
            )]
        );
    }

    /// The real samplers, end to end: CPU burned in this test shows up in the
    /// next window's samples, as nanoseconds at the sampling period.
    #[test]
    fn cpu_samples_cover_the_work_done_in_the_window() {
        let mut sampler = CpuSampler::new().unwrap();
        let started = std::time::Instant::now();
        let mut x = 1u64;
        while started.elapsed() < Duration::from_millis(500) {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        }
        std::hint::black_box(x);
        let samples = sampler.collect().unwrap();
        let total: i64 = samples.iter().map(|s| s.value).sum();
        assert!(total > 0, "no CPU samples");
        assert!(samples.iter().all(|s| s.value % CPU_PERIOD_NANOS == 0));
        assert!(samples.iter().all(|s| !s.frames.is_empty()));
        // A second window reports only new samples, not the first window again.
        let again: i64 = sampler.collect().unwrap().iter().map(|s| s.value).sum();
        assert!(
            again < total,
            "second window repeated the first: {again} >= {total}"
        );
    }
}
