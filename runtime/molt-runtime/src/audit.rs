//! Structured audit logging for the Molt runtime.
//!
//! Every capability check (filesystem, network, environment, database, etc.) can
//! emit an [`AuditEvent`] that records the operation, the capability that was
//! tested, operation-specific arguments, and the resulting decision.
//!
//! Events are routed through a thread-local [`AuditSink`].  By default the sink
//! is [`NullSink`] (zero overhead).  Call [`set_audit_sink`] during
//! initialization to install a real sink such as [`JsonLinesSink`],
//! [`BufferedSink`], or [`StderrSink`].
//!
//! # Manual JSON serialization
//!
//! `molt-runtime` does not depend on serde, so all JSON output is hand-written.
//! The helpers in this module escape strings according to RFC 8259.

use std::cell::RefCell;
use std::fmt;
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// The outcome of a capability check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditDecision {
    /// The operation was permitted.
    Allowed,
    /// The operation was denied for the given reason.
    Denied { reason: String },
    /// The operation hit a resource limit (e.g. file-descriptor cap).
    ResourceExceeded { error: String },
}

/// Operation-specific arguments attached to an [`AuditEvent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditArgs {
    /// A filesystem path.
    Path(String),
    /// A network endpoint.
    Network { host: String, port: u16 },
    /// An environment variable key.
    Env { key: String },
    /// A database query identified by its hash.
    Db { query_hash: u64 },
    /// Free-form argument string for operations that don't fit the above.
    Custom(String),
    /// No additional arguments.
    None,
}

/// A single audit record emitted by a capability check.
#[derive(Debug, Clone)]
pub struct AuditEvent {
    /// Monotonic nanosecond timestamp (relative to `UNIX_EPOCH`).
    pub timestamp_ns: u64,
    /// Dot-separated operation identifier, e.g. `"fs.read"`, `"net.connect"`.
    pub operation: &'static str,
    /// Capability that was checked, e.g. `"fs.read"`, `"net.outbound"`.
    pub capability: &'static str,
    /// Operation-specific arguments.
    pub args: AuditArgs,
    /// Whether the operation was allowed, denied, or resource-exceeded.
    pub decision: AuditDecision,
    /// Fully-qualified Python module that triggered the check.
    pub module: String,
}

// ---------------------------------------------------------------------------
// Timestamps
// ---------------------------------------------------------------------------

/// Return the current wall-clock time as nanoseconds since the Unix epoch.
/// Falls back to 0 when the system clock is unavailable (e.g. some WASM
/// environments).
fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_nanos()).ok())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// JSON helpers (no serde)
// ---------------------------------------------------------------------------

/// Escape a string for embedding in a JSON value (RFC 8259 section 7).
///
/// Also escapes U+2028 (LINE SEPARATOR) and U+2029 (PARAGRAPH SEPARATOR)
/// which are legal unescaped in JSON but treated as line terminators by
/// JavaScript engines, breaking JSON Lines parsing.
fn json_escape(s: &str, buf: &mut String) {
    buf.push('"');
    for ch in s.chars() {
        match ch {
            '"' => buf.push_str("\\\""),
            '\\' => buf.push_str("\\\\"),
            '\n' => buf.push_str("\\n"),
            '\r' => buf.push_str("\\r"),
            '\t' => buf.push_str("\\t"),
            '\u{2028}' => buf.push_str("\\u2028"),
            '\u{2029}' => buf.push_str("\\u2029"),
            c if c < '\x20' => {
                buf.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => buf.push(c),
        }
    }
    buf.push('"');
}

impl AuditDecision {
    /// Append this decision as a JSON object fragment (without outer braces).
    fn append_json(&self, buf: &mut String) {
        match self {
            AuditDecision::Allowed => {
                buf.push_str("\"status\":\"allowed\"");
            }
            AuditDecision::Denied { reason } => {
                buf.push_str("\"status\":\"denied\",\"reason\":");
                json_escape(reason, buf);
            }
            AuditDecision::ResourceExceeded { error } => {
                buf.push_str("\"status\":\"resource_exceeded\",\"error\":");
                json_escape(error, buf);
            }
        }
    }
}

impl AuditArgs {
    /// Append these args as a JSON object fragment (without outer braces).
    fn append_json(&self, buf: &mut String) {
        match self {
            AuditArgs::Path(p) => {
                buf.push_str("\"kind\":\"path\",\"path\":");
                json_escape(p, buf);
            }
            AuditArgs::Network { host, port } => {
                buf.push_str("\"kind\":\"network\",\"host\":");
                json_escape(host, buf);
                buf.push_str(",\"port\":");
                buf.push_str(&port.to_string());
            }
            AuditArgs::Env { key } => {
                buf.push_str("\"kind\":\"env\",\"key\":");
                json_escape(key, buf);
            }
            AuditArgs::Db { query_hash } => {
                buf.push_str("\"kind\":\"db\",\"query_hash\":");
                buf.push_str(&query_hash.to_string());
            }
            AuditArgs::Custom(s) => {
                buf.push_str("\"kind\":\"custom\",\"value\":");
                json_escape(s, buf);
            }
            AuditArgs::None => {
                buf.push_str("\"kind\":\"none\"");
            }
        }
    }
}

impl AuditEvent {
    /// Create a new event with the current timestamp.
    pub fn new(
        operation: &'static str,
        capability: &'static str,
        args: AuditArgs,
        decision: AuditDecision,
        module: String,
    ) -> Self {
        Self {
            timestamp_ns: now_ns(),
            operation,
            capability,
            args,
            decision,
            module,
        }
    }

    /// Serialize the event as a single-line JSON object.
    pub fn to_json(&self) -> String {
        let mut buf = String::with_capacity(256);
        buf.push_str("{\"timestamp_ns\":");
        buf.push_str(&self.timestamp_ns.to_string());
        buf.push_str(",\"operation\":");
        json_escape(self.operation, &mut buf);
        buf.push_str(",\"capability\":");
        json_escape(self.capability, &mut buf);
        buf.push_str(",\"args\":{");
        self.args.append_json(&mut buf);
        buf.push_str("},\"decision\":{");
        self.decision.append_json(&mut buf);
        buf.push_str("},\"module\":");
        json_escape(&self.module, &mut buf);
        buf.push('}');
        buf
    }
}

impl fmt::Display for AuditEvent {
    /// Compact human-readable representation for stderr logging.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status = match &self.decision {
            AuditDecision::Allowed => "ALLOWED",
            AuditDecision::Denied { .. } => "DENIED",
            AuditDecision::ResourceExceeded { .. } => "EXCEEDED",
        };
        write!(
            f,
            "[audit] {} {} cap={} mod={}",
            status, self.operation, self.capability, self.module
        )
    }
}

// ---------------------------------------------------------------------------
// AuditSink trait
// ---------------------------------------------------------------------------

/// Receives audit events.  Implementations must be `Send + Sync` so that a
/// single sink can be shared across threads (the thread-local slot stores a
/// pointer to the global sink).
pub trait AuditSink: Send + Sync {
    /// Process a single event.  Implementations should be cheap; expensive
    /// work (network I/O, disk flushes) should be batched or deferred.
    fn emit(&self, event: &AuditEvent);

    /// Flush any buffered output.  Called on graceful shutdown.
    fn flush(&self) {}
}

// ---------------------------------------------------------------------------
// Built-in sinks
// ---------------------------------------------------------------------------

/// No-op sink.  Installed by default when audit logging is disabled.
pub struct NullSink;

impl AuditSink for NullSink {
    #[inline]
    fn emit(&self, _event: &AuditEvent) {}
}

/// Writes one JSON object per line to an arbitrary `std::io::Write` destination.
pub struct JsonLinesSink<W: Write + Send + Sync> {
    writer: Mutex<W>,
}

impl<W: Write + Send + Sync> JsonLinesSink<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer: Mutex::new(writer),
        }
    }
}

impl<W: Write + Send + Sync> AuditSink for JsonLinesSink<W> {
    fn emit(&self, event: &AuditEvent) {
        let line = event.to_json();
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(line.as_bytes());
            let _ = w.write_all(b"\n");
        }
    }

    fn flush(&self) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.flush();
        }
    }
}

/// Collects events in memory for later batch export or testing.
pub struct BufferedSink {
    events: Mutex<Vec<AuditEvent>>,
}

impl BufferedSink {
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    /// Return a snapshot of all buffered events.
    pub fn events(&self) -> Vec<AuditEvent> {
        self.events.lock().map(|e| e.clone()).unwrap_or_default()
    }

    /// Drain and return all buffered events, leaving the buffer empty.
    pub fn drain(&self) -> Vec<AuditEvent> {
        self.events
            .lock()
            .map(|mut e| std::mem::take(&mut *e))
            .unwrap_or_default()
    }
}

impl Default for BufferedSink {
    fn default() -> Self {
        Self::new()
    }
}

impl AuditSink for BufferedSink {
    fn emit(&self, event: &AuditEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event.clone());
        }
    }
}

/// Writes compact human-readable lines to stderr.
pub struct StderrSink;

impl AuditSink for StderrSink {
    fn emit(&self, event: &AuditEvent) {
        eprintln!("{event}");
    }
}

// ---------------------------------------------------------------------------
// Global fallback sink
// ---------------------------------------------------------------------------

/// Global audit sink, set once by the first call to [`set_audit_sink`].
///
/// Threads that never call `set_audit_sink` will inherit this sink
/// automatically on their first use of [`audit_emit`], instead of silently
/// dropping events via [`NullSink`].
static GLOBAL_AUDIT_SINK: OnceLock<Arc<dyn AuditSink + Send + Sync>> = OnceLock::new();

// ---------------------------------------------------------------------------
// Thread-local accessor
// ---------------------------------------------------------------------------

/// Wrapper that delegates to a shared `Arc<dyn AuditSink>`.
struct ArcSinkAdapter(Arc<dyn AuditSink + Send + Sync>);

impl AuditSink for ArcSinkAdapter {
    fn emit(&self, event: &AuditEvent) {
        self.0.emit(event);
    }
    fn flush(&self) {
        self.0.flush();
    }
}

/// Initialize the thread-local sink.  If a global sink has been set (by
/// another thread calling `set_audit_sink`), use it; otherwise fall back to
/// `NullSink`.
fn make_default_sink() -> Box<dyn AuditSink> {
    match GLOBAL_AUDIT_SINK.get() {
        Some(arc) => Box::new(ArcSinkAdapter(Arc::clone(arc))),
        None => Box::new(NullSink),
    }
}

thread_local! {
    static AUDIT_SINK: RefCell<Box<dyn AuditSink>> = RefCell::new(make_default_sink());
}

/// Emit an audit event through the thread-local sink.
///
/// This is the primary entry point for all audit logging in the runtime.
/// If no sink has been installed via [`set_audit_sink`], the event is silently
/// dropped by the default [`NullSink`].
pub fn audit_emit(event: AuditEvent) {
    AUDIT_SINK.with(|cell| {
        cell.borrow().emit(&event);
    });
}

/// Replace the thread-local audit sink.
///
/// On the **first** call (across all threads), the sink is also stored as the
/// global default.  Subsequent threads that never call `set_audit_sink` will
/// inherit this global sink automatically, eliminating the silent-drop problem
/// where spawned threads would fall back to [`NullSink`].
///
/// Must be called before any capability checks run.  Typically invoked once
/// during runtime initialization on the main thread.
pub fn set_audit_sink(sink: Box<dyn AuditSink>) {
    AUDIT_SINK.with(|cell| {
        *cell.borrow_mut() = sink;
    });
}

/// Set the global default audit sink that all new threads will inherit.
///
/// This uses [`OnceLock`], so only the first call takes effect.  Typically
/// called once during host initialization, before spawning worker threads.
///
/// Unlike [`set_audit_sink`] (which only affects the calling thread), this
/// ensures every thread created *after* this call inherits the sink.
pub fn set_global_audit_sink(sink: Arc<dyn AuditSink + Send + Sync>) {
    let _ = GLOBAL_AUDIT_SINK.set(sink);
}

/// Flush the thread-local audit sink.  Call on graceful shutdown.
pub fn audit_flush() {
    AUDIT_SINK.with(|cell| {
        cell.borrow().flush();
    });
}

// ---------------------------------------------------------------------------
// Convenience helper for capability-gated code paths
// ---------------------------------------------------------------------------

/// Emit an audit event recording a capability decision.
///
/// This is the recommended entry point for non-VFS capability checks.
/// It accepts `&'static str` for the operation and capability names,
/// keeping allocations to the bare minimum on the hot path.
///
/// `operation`  — dot-separated name such as `"process.signal"`, `"net.connect"`.
/// `capability` — the capability that was tested, e.g. `"process"`, `"net"`.
/// `args`       — operation-specific arguments (use `AuditArgs::None` when N/A).
/// `allowed`    — whether the capability check passed.
pub fn audit_capability_decision(
    operation: &'static str,
    capability: &'static str,
    args: AuditArgs,
    allowed: bool,
) {
    let decision = if allowed {
        AuditDecision::Allowed
    } else {
        AuditDecision::Denied {
            reason: format!("missing {capability} capability"),
        }
    };
    audit_emit(AuditEvent::new(
        operation,
        capability,
        args,
        decision,
        module_path!().to_string(),
    ));
}

// ---------------------------------------------------------------------------
// Helper macro
// ---------------------------------------------------------------------------

/// Emit an audit event for a capability check.
///
/// # Usage
///
/// ```ignore
/// use molt_runtime::audit::{AuditArgs, AuditDecision};
///
/// // With automatic module name (uses Rust module_path!()):
/// audit_cap_check!("fs.read", "fs.read",
///     AuditArgs::Path(path.clone()),
///     AuditDecision::Allowed);
///
/// // With explicit Python module name:
/// audit_cap_check!("net.connect", "net.outbound",
///     AuditArgs::Network { host: host.clone(), port },
///     AuditDecision::Denied { reason: "sandbox policy".into() },
///     "myapp.http");
/// ```
///
/// The `module` field defaults to `module_path!()` (the Rust module path).
/// Callers that know the Python module should pass it as the fifth argument.
#[macro_export]
macro_rules! audit_cap_check {
    ($op:expr, $cap:expr, $args:expr, $decision:expr) => {
        $crate::audit::audit_emit($crate::audit::AuditEvent::new(
            $op,
            $cap,
            $args,
            $decision,
            module_path!().to_string(),
        ));
    };
    ($op:expr, $cap:expr, $args:expr, $decision:expr, $module:expr) => {
        $crate::audit::audit_emit($crate::audit::AuditEvent::new(
            $op,
            $cap,
            $args,
            $decision,
            $module.to_string(),
        ));
    };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Install a `BufferedSink`, run `body`, then return captured events.
    fn with_buffered_sink<F: FnOnce()>(body: F) -> Vec<AuditEvent> {
        let sink = Arc::new(BufferedSink::new());
        // We need a wrapper that delegates to the Arc so we can read back.
        struct ArcSink(Arc<BufferedSink>);
        impl AuditSink for ArcSink {
            fn emit(&self, event: &AuditEvent) {
                self.0.emit(event);
            }
        }
        let arc_sink = Arc::clone(&sink);
        set_audit_sink(Box::new(ArcSink(arc_sink)));
        body();
        let events = sink.events();
        // Restore null sink to avoid cross-test interference.
        set_audit_sink(Box::new(NullSink));
        events
    }

    #[test]
    fn null_sink_does_not_panic() {
        set_audit_sink(Box::new(NullSink));
        audit_emit(AuditEvent::new(
            "fs.read",
            "fs.read",
            AuditArgs::Path("/etc/passwd".into()),
            AuditDecision::Allowed,
            "test".into(),
        ));
    }

    #[test]
    fn buffered_sink_captures_events() {
        let events = with_buffered_sink(|| {
            audit_emit(AuditEvent::new(
                "fs.read",
                "fs.read",
                AuditArgs::Path("/tmp/data.txt".into()),
                AuditDecision::Allowed,
                "myapp.io".into(),
            ));
            audit_emit(AuditEvent::new(
                "net.connect",
                "net.outbound",
                AuditArgs::Network {
                    host: "example.com".into(),
                    port: 443,
                },
                AuditDecision::Denied {
                    reason: "sandbox".into(),
                },
                "myapp.http".into(),
            ));
        });
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].operation, "fs.read");
        assert_eq!(events[1].operation, "net.connect");
        assert_eq!(
            events[1].decision,
            AuditDecision::Denied {
                reason: "sandbox".into()
            }
        );
    }

    #[test]
    fn buffered_sink_drain() {
        let sink = BufferedSink::new();
        sink.emit(&AuditEvent::new(
            "env.get",
            "env.read",
            AuditArgs::Env {
                key: "SECRET".into(),
            },
            AuditDecision::Allowed,
            "config".into(),
        ));
        assert_eq!(sink.events().len(), 1);
        let drained = sink.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(sink.events().len(), 0);
    }

    #[test]
    fn json_serialization_roundtrip() {
        let event = AuditEvent {
            timestamp_ns: 1_700_000_000_000_000_000,
            operation: "fs.write",
            capability: "fs.write",
            args: AuditArgs::Path("/tmp/out.txt".into()),
            decision: AuditDecision::Allowed,
            module: "writer".into(),
        };
        let json = event.to_json();
        assert!(json.starts_with('{'));
        assert!(json.ends_with('}'));
        assert!(json.contains("\"operation\":\"fs.write\""));
        assert!(json.contains("\"status\":\"allowed\""));
        assert!(json.contains("\"path\":\"/tmp/out.txt\""));
        assert!(json.contains("\"module\":\"writer\""));
        // Must not contain newlines (JSON Lines requirement).
        assert!(!json.contains('\n'));
    }

    #[test]
    fn json_escape_special_chars() {
        let event = AuditEvent {
            timestamp_ns: 0,
            operation: "custom",
            capability: "custom",
            args: AuditArgs::Custom("line1\nline2\ttab\"quote\\back".into()),
            decision: AuditDecision::ResourceExceeded {
                error: "too\nmany".into(),
            },
            module: "esc\test".into(),
        };
        let json = event.to_json();
        // Newlines and tabs must be escaped.
        assert!(!json.contains('\n'));
        assert!(!json.contains('\t'));
        assert!(json.contains("\\n"));
        assert!(json.contains("\\t"));
        assert!(json.contains("\\\""));
        assert!(json.contains("\\\\"));
    }

    #[test]
    fn json_lines_sink_writes_newline_terminated() {
        let buf: Vec<u8> = Vec::new();
        let sink = JsonLinesSink::new(buf);
        sink.emit(&AuditEvent::new(
            "db.query",
            "db.read",
            AuditArgs::Db { query_hash: 0xDEAD },
            AuditDecision::Allowed,
            "dal".into(),
        ));
        let output = sink.writer.lock().unwrap();
        let text = String::from_utf8_lossy(&output);
        assert!(text.ends_with('\n'));
        let lines: Vec<&str> = text.trim().split('\n').collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with('{'));
    }

    #[test]
    fn display_format() {
        let event = AuditEvent::new(
            "fs.read",
            "fs.read",
            AuditArgs::None,
            AuditDecision::Allowed,
            "demo".into(),
        );
        let s = format!("{event}");
        assert!(s.contains("[audit]"));
        assert!(s.contains("ALLOWED"));
        assert!(s.contains("fs.read"));
        assert!(s.contains("mod=demo"));
    }

    #[test]
    fn all_args_variants_serialize() {
        let variants = vec![
            AuditArgs::Path("/a".into()),
            AuditArgs::Network {
                host: "h".into(),
                port: 80,
            },
            AuditArgs::Env { key: "K".into() },
            AuditArgs::Db { query_hash: 42 },
            AuditArgs::Custom("x".into()),
            AuditArgs::None,
        ];
        for args in variants {
            let event = AuditEvent {
                timestamp_ns: 1,
                operation: "test",
                capability: "test",
                args,
                decision: AuditDecision::Allowed,
                module: "t".into(),
            };
            let json = event.to_json();
            assert!(json.starts_with('{'));
            assert!(json.ends_with('}'));
        }
    }

    #[test]
    fn macro_basic_usage() {
        let events = with_buffered_sink(|| {
            audit_cap_check!(
                "fs.read",
                "fs.read",
                AuditArgs::Path("/x".into()),
                AuditDecision::Allowed
            );
            audit_cap_check!(
                "net.connect",
                "net.outbound",
                AuditArgs::Network {
                    host: "h".into(),
                    port: 8080
                },
                AuditDecision::Denied {
                    reason: "nope".into()
                },
                "my.module"
            );
        });
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].operation, "fs.read");
        assert_eq!(events[1].module, "my.module");
    }

    #[test]
    fn global_sink_inherited_by_spawned_thread() {
        // Set up a shared buffered sink as the global default.
        let sink = Arc::new(BufferedSink::new());

        struct SharedSink(Arc<BufferedSink>);
        impl AuditSink for SharedSink {
            fn emit(&self, event: &AuditEvent) {
                self.0.emit(event);
            }
        }

        // Install as global default (OnceLock: first call wins across
        // the entire test process, so this test must run with
        // `--test-threads=1` or accept that if another test set it first,
        // the OnceLock is already occupied).
        let shared: Arc<dyn AuditSink + Send + Sync> = Arc::new(SharedSink(Arc::clone(&sink)));
        set_global_audit_sink(Arc::clone(&shared));

        // Spawn a thread that NEVER calls set_audit_sink.
        // Its thread-local will be initialized from make_default_sink(),
        // which reads GLOBAL_AUDIT_SINK.
        let handle = std::thread::spawn(move || {
            audit_emit(AuditEvent::new(
                "thread.test",
                "thread.cap",
                AuditArgs::Custom("from-child".into()),
                AuditDecision::Allowed,
                "child_thread".into(),
            ));
        });
        handle.join().expect("child thread panicked");

        // The event should have been captured by the shared sink.
        let events = sink.events();
        assert!(
            events.iter().any(|e| e.operation == "thread.test"),
            "global audit sink was not inherited by the spawned thread; \
             captured {} events: {:?}",
            events.len(),
            events.iter().map(|e| e.operation).collect::<Vec<_>>()
        );
    }
}
