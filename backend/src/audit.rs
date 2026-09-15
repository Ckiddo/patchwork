//! Allowlisted telemetry: no URI, headers, body, credentials or raw errors.
use crate::config::LogLevel;
use std::sync::{Mutex, OnceLock};
use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Default)]
struct PoolTimings {
    seconds: Vec<f64>,
    discarded: u64,
}
static POOL_TIMINGS: OnceLock<Mutex<PoolTimings>> = OnceLock::new();
struct PoolAcquireLayer;
impl<S: tracing::Subscriber> Layer<S> for PoolAcquireLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() != "sqlx::pool::acquire" {
            return;
        }
        #[derive(Default)]
        struct DurationVisitor(Option<f64>);
        impl tracing::field::Visit for DurationVisitor {
            fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
            fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
                // This spelling is the duration field emitted by SQLx 0.8.6.
                if field.name() == "aquired_after_secs" && value.is_finite() && value >= 0.0 {
                    self.0 = Some(value);
                }
            }
        }
        let mut visitor = DurationVisitor::default();
        event.record(&mut visitor);
        if let Some(seconds) = visitor.0 {
            let mut timings = POOL_TIMINGS
                .get_or_init(Mutex::default)
                .lock()
                .expect("pool timings");
            if timings.seconds.len() < 100_000 {
                timings.seconds.push(seconds);
            } else {
                timings.discarded += 1;
            }
        }
    }
}

pub fn flush_pool_timings() {
    let Some(timings) = POOL_TIMINGS.get() else {
        return;
    };
    let mut timings = timings.lock().expect("pool timings");
    if timings.seconds.is_empty() {
        return;
    }
    timings.seconds.sort_by(f64::total_cmp);
    let count = timings.seconds.len();
    let p95 = timings.seconds[(count * 95).div_ceil(100) - 1] * 1000.0;
    let max = timings.seconds[count - 1] * 1000.0;
    tracing::info!(target: "patchwork_audit", event = "pool_acquire_summary", samples = count, p95_ms = p95, max_ms = max, discarded = timings.discarded);
    timings.seconds.clear();
}

pub fn init(level: &LogLevel) {
    let directive = match level {
        LogLevel::Error => "off,patchwork_audit=error",
        LogLevel::Warn => "off,patchwork_audit=warn,sqlx::pool::acquire=warn",
        // SQLx 0.8.6's pool-acquire target contains durations only. Statement and
        // connection-error targets remain disabled, including during load tests.
        LogLevel::Info => "off,patchwork_audit=info,sqlx::pool::acquire=info",
    };
    tracing_subscriber::registry()
        .with(EnvFilter::new(directive))
        .with(PoolAcquireLayer)
        .with(
            fmt::layer()
                .json()
                .with_current_span(false)
                .with_span_list(false)
                // Avoid synchronous console/file I/O for every pool checkout. Keep only
                // bounded numeric samples in memory and emit one summary on clean exit.
                .with_filter(tracing_subscriber::filter::filter_fn(|m| {
                    m.target() != "sqlx::pool::acquire"
                })),
        )
        .init();
}

pub fn http_response(status: u16) {
    tracing::info!(target: "patchwork_audit", event = "http_response", status);
}
