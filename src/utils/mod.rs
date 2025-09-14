//! Utilities: logging (dynamic level), minimal JSON string helpers, ANSI color (respects NO_COLOR),
//! progress tracking, monotonic timing, simple error context trait.
//!
//! Key items:
//!   init_logging / derive_level
//!   output::* (json_escape etc.)
//!   monotonic_ms
//!   Progress / ProgressSnapshot

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Logging helpers.
pub mod logging {
    use super::*;

    #[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
    pub enum LogLevel {
        Error = 0,
        Info = 1,
        Debug = 2,
        Trace = 3,
    }

    impl LogLevel {
        pub fn as_str(&self) -> &'static str {
            match self {
                LogLevel::Error => "ERROR",
                LogLevel::Info => "INFO",
                LogLevel::Debug => "DEBUG",
                LogLevel::Trace => "TRACE",
            }
        }
    }

    static GLOBAL_LEVEL: OnceLock<AtomicU8> = OnceLock::new();

    fn inner_cell() -> &'static AtomicU8 {
        GLOBAL_LEVEL.get_or_init(|| AtomicU8::new(LogLevel::Info as u8))
    }

    pub fn init_logging(level: LogLevel) {
        set_log_level(level);
    }

    pub fn set_log_level(level: LogLevel) {
        inner_cell().store(level as u8, Ordering::Relaxed);
    }

    pub fn current_log_level() -> LogLevel {
        match inner_cell().load(Ordering::Relaxed) {
            0 => LogLevel::Error,
            1 => LogLevel::Info,
            2 => LogLevel::Debug,
            _ => LogLevel::Trace,
        }
    }

    pub fn derive_level(verbose: u8, quiet: bool) -> LogLevel {
        if quiet {
            return LogLevel::Error;
        }
        match verbose {
            0 => LogLevel::Info,
            1 => LogLevel::Debug,
            _ => LogLevel::Trace,
        }
    }

    fn timestamp() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    }

    fn should_emit(level: LogLevel) -> bool {
        level <= current_log_level()
    }

    pub fn log(level: LogLevel, msg: impl AsRef<str>) {
        if should_emit(level) {
            println!("[{}][{}] {}", level.as_str(), timestamp(), msg.as_ref());
        }
    }

    pub fn error(msg: impl AsRef<str>) {
        log(LogLevel::Error, msg);
    }
    pub fn info(msg: impl AsRef<str>) {
        log(LogLevel::Info, msg);
    }
    pub fn debug(msg: impl AsRef<str>) {
        log(LogLevel::Debug, msg);
    }
    pub fn trace(msg: impl AsRef<str>) {
        log(LogLevel::Trace, msg);
    }

    #[macro_export]
    macro_rules! log_error {
        ($($t:tt)*) => { $crate::utils::logging::error(format!($($t)*)) };
    }
    #[macro_export]
    macro_rules! log_info {
        ($($t:tt)*) => { $crate::utils::logging::info(format!($($t)*)) };
    }
    #[macro_export]
    macro_rules! log_debug {
        ($($t:tt)*) => { $crate::utils::logging::debug(format!($($t)*)) };
    }
    #[macro_export]
    macro_rules! log_trace {
        ($($t:tt)*) => { $crate::utils::logging::trace(format!($($t)*)) };
    }
}

pub use logging::{derive_level, init_logging};

/// Output related helpers (simple JSON/ANSI formatting w/o extra deps).
pub mod output {
    /// Escape a string minimally for JSON string context.
    pub fn json_escape(input: &str) -> String {
        let mut out = String::with_capacity(input.len() + 2);
        for c in input.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out
    }

    /// Wrap a key and raw value into JSON key-value (value must already be JSON safe).
    pub fn json_kv_raw(key: &str, raw_value: &str) -> String {
        format!("\"{}\":{}", json_escape(key), raw_value)
    }

    /// Wrap a key and string value into JSON key-value.
    pub fn json_kv(key: &str, value: &str) -> String {
        format!("\"{}\":\"{}\"", json_escape(key), json_escape(value))
    }

    /// Turn Option<&str> into JSON raw value.
    pub fn json_opt_str(v: Option<&str>) -> String {
        match v {
            Some(s) => format!("\"{}\"", json_escape(s)),
            None => "null".to_string(),
        }
    }

    /// Simple join helper for JSON objects.
    pub fn json_obj(fields: &[String]) -> String {
        format!("{{{}}}", fields.join(","))
    }

    /// Simple ansi color wrapper (disable via NO_COLOR).
    pub fn color(c: Color, text: impl AsRef<str>) -> String {
        if std::env::var_os("NO_COLOR").is_some() {
            return text.as_ref().to_string();
        }
        format!("{}{}{}", c.as_code(), text.as_ref(), "\x1b[0m")
    }

    #[derive(Copy, Clone)]
    pub enum Color {
        Red,
        Green,
        Yellow,
        Blue,
        Magenta,
        Cyan,
        Bold,
    }
    impl Color {
        fn as_code(&self) -> &'static str {
            match self {
                Color::Red => "\x1b[31m",
                Color::Green => "\x1b[32m",
                Color::Yellow => "\x1b[33m",
                Color::Blue => "\x1b[34m",
                Color::Magenta => "\x1b[35m",
                Color::Cyan => "\x1b[36m",
                Color::Bold => "\x1b[1m",
            }
        }
    }
}

/// Generic error enrichment helper (lightweight inline alternative to anyhow::Context).
pub trait ContextExt<T> {
    fn ctx(self, msg: &'static str) -> anyhow::Result<T>;
}

impl<T, E: std::error::Error + Send + Sync + 'static> ContextExt<T> for Result<T, E> {
    fn ctx(self, msg: &'static str) -> anyhow::Result<T> {
        self.map_err(|e| anyhow::anyhow!("{}: {}", msg, e))
    }
}

/// Simple time utility: monotonic milliseconds (NOT wall clock).
pub fn monotonic_ms() -> u128 {
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    let base = START.get_or_init(Instant::now);
    base.elapsed().as_millis()
}

/// Lightweight progress indicator state.
pub struct Progress {
    total: Option<u64>,
    current: u64,
    started: std::time::Instant,
}

impl Progress {
    pub fn new(total: Option<u64>) -> Self {
        Self {
            total,
            current: 0,
            started: std::time::Instant::now(),
        }
    }
    pub fn inc(&mut self, delta: u64) {
        self.current += delta;
    }
    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            current: self.current,
            total: self.total,
            elapsed_ms: self.started.elapsed().as_millis(),
        }
    }
}

pub struct ProgressSnapshot {
    pub current: u64,
    pub total: Option<u64>,
    pub elapsed_ms: u128,
}

impl ProgressSnapshot {
    pub fn rate_per_sec(&self) -> f64 {
        if self.elapsed_ms == 0 {
            return 0.0;
        }
        (self.current as f64) / (self.elapsed_ms as f64 / 1000.0)
    }
}

// End of utils module.
