use std::fs::File;
use std::io::{self, IsTerminal};
use std::path::Path;
use std::sync::Once;

use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::prelude::*;

use crate::error::HostError;

static PANIC_HOOK: Once = Once::new();

/// The guard must outlive logging so queued records can flush.
pub fn init_logger(path: impl AsRef<Path>) -> Result<WorkerGuard, HostError> {
    let guard = init_subscriber(path)?;
    install_panic_hook();
    Ok(guard)
}

/// The log of the wgpu renderer mode: the tracing lines (the startup generation check of the
/// distant land) go to this program's standard output, which `--log` sends into the helper's
/// own log, so that a wgpu start writes one log of this program. No panic hook: the crate of
/// the renderer reports a panic in that log.
pub fn init_stdout_logger() -> Result<(), HostError> {
    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_writer(io::stdout)
        .with_ansi(false)
        .with_target(false)
        .with_span_events(FmtSpan::CLOSE);
    tracing_subscriber::registry()
        .with(level_filter())
        .with(stdout_layer)
        .with(distantland::TraceReportLayer::with_handle(distantland::trace_report_handle()))
        .try_init()
        .map_err(|e| HostError::init(format!("Failed to initialize logger: {e:?}")))
}

/// INFO and above, unless RUST_LOG says otherwise.
fn level_filter() -> EnvFilter {
    EnvFilter::builder()
        .with_default_directive(Level::INFO.into())
        .with_regex(false)
        .from_env_lossy()
}

fn init_subscriber(path: impl AsRef<Path>) -> Result<WorkerGuard, HostError> {
    // The log of the run before stays as `<path>.1`: a player who starts the game again after
    // a crash still has it.
    nirender_wgpu::rotate_log(path.as_ref());
    let file = File::create(path).map_err(HostError::io)?;

    // No buffer between the worker and the file: the last lines before an abort are written
    // as they come (the abort of a panic skips the guard, which would flush a buffer).
    let (writer, guard) = tracing_appender::non_blocking(file);

    let filter = level_filter();

    let file_layer = tracing_subscriber::fmt::layer()
        .pretty()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(false)
        .with_file(false)
        .with_line_number(false)
        .with_span_events(FmtSpan::CLOSE);

    let stderr_layer = io::stderr().is_terminal().then(|| {
        // The IPC host normally launches headlessly from the 32-bit runtime.
        tracing_subscriber::fmt::layer()
            .pretty()
            .with_writer(io::stderr)
            .with_target(false)
            .with_file(false)
            .with_line_number(false)
            .with_span_events(FmtSpan::CLOSE)
    });

    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer);

    let registry = registry.with(distantland::TraceReportLayer::with_handle(distantland::trace_report_handle()));

    if let Err(e) = registry.try_init() {
        return Err(HostError::init(format!("Failed to initialize logger: {e:?}")));
    }

    Ok(guard)
}

fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let payload = info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
                .unwrap_or("<non-string panic payload>");
            let location = info
                .location()
                .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
                .unwrap_or_else(|| "<unknown>".to_string());

            tracing::error!(panic_message = payload, panic_location = location, "numidium panicked");
        }));
    });
}
