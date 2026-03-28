# Molt Audit Logging

This document describes the structured audit logging system for capability-gated
operations in the Molt runtime.

## Overview

Every capability check (filesystem, network, environment, database) can emit a
structured `AuditEvent` that records:

- The operation performed (e.g., `fs.read`, `net.connect`)
- The capability that was tested
- Operation-specific arguments (path, host/port, env key, query hash)
- The resulting decision (allowed, denied, resource exceeded)
- The Python module that triggered the check
- A nanosecond-precision timestamp

By default, audit logging is disabled via `NullSink` (zero runtime overhead).
Enable it through the capability manifest or CLI flags.

## AuditSink Trait

```rust
pub trait AuditSink: Send + Sync {
    fn emit(&self, event: &AuditEvent);
    fn flush(&self) {}
}
```

Sinks must be `Send + Sync` for cross-thread sharing. The thread-local sink is
set during initialization via `set_audit_sink`. Implementations should keep
`emit` cheap; expensive work should be batched or deferred.

## Built-in Sinks

| Sink | Description | Use Case |
| --- | --- | --- |
| `NullSink` | No-op, zero overhead | Default when audit is disabled |
| `StderrSink` | Compact human-readable lines to stderr | Local debugging |
| `JsonLinesSink<W>` | One JSON object per line to any `Write` | Log aggregation, compliance |
| `BufferedSink` | Collects events in memory | Testing, batch export |

### NullSink

Installed by default. All calls are inlined to no-ops by the compiler.

### StderrSink

Writes compact lines in the format:

```
[audit] ALLOWED fs.read cap=fs.read mod=myapp.io
[audit] DENIED net.connect cap=net.outbound mod=myapp.http
```

### JsonLinesSink

Writes one JSON object per line to any `std::io::Write` destination:

```json
{"timestamp_ns":1711612800000000000,"operation":"fs.read","capability":"fs.read","args":{"kind":"path","path":"/tmp/data.txt"},"decision":{"status":"allowed"},"module":"myapp.io"}
```

### BufferedSink

Stores events in a `Mutex<Vec<AuditEvent>>` for programmatic access:

```rust
let sink = BufferedSink::new();
// ... run code ...
let events = sink.events();  // snapshot
let events = sink.drain();   // drain and clear
```

## Event Format (JSON Schema)

Each JSON Lines event has the following structure:

| Field | Type | Description |
| --- | --- | --- |
| `timestamp_ns` | `u64` | Nanoseconds since Unix epoch |
| `operation` | `string` | Dot-separated operation (e.g., `fs.read`, `net.connect`, `env.get`) |
| `capability` | `string` | Capability that was checked (e.g., `fs.read`, `net.outbound`) |
| `args` | `object` | Operation-specific arguments (see below) |
| `decision` | `object` | Outcome of the check (see below) |
| `module` | `string` | Python module that triggered the check |

### Args Variants

| `kind` | Additional Fields | Example |
| --- | --- | --- |
| `path` | `path: string` | `{"kind":"path","path":"/tmp/data.txt"}` |
| `network` | `host: string, port: u16` | `{"kind":"network","host":"example.com","port":443}` |
| `env` | `key: string` | `{"kind":"env","key":"SECRET_KEY"}` |
| `db` | `query_hash: u64` | `{"kind":"db","query_hash":57005}` |
| `custom` | `value: string` | `{"kind":"custom","value":"..."}` |
| `none` | *(none)* | `{"kind":"none"}` |

### Decision Variants

| `status` | Additional Fields | Meaning |
| --- | --- | --- |
| `allowed` | *(none)* | Operation was permitted |
| `denied` | `reason: string` | Operation was blocked by policy |
| `resource_exceeded` | `error: string` | Resource limit hit |

## Integration with Capability System

Audit events are emitted at the same callsites as capability checks. Use the
`audit_cap_check!` macro for ergonomic integration:

```rust
use molt_runtime::audit::{AuditArgs, AuditDecision};

// With automatic module name (uses Rust module_path!()):
audit_cap_check!("fs.read", "fs.read",
    AuditArgs::Path(path.clone()),
    AuditDecision::Allowed);

// With explicit Python module name:
audit_cap_check!("net.connect", "net.outbound",
    AuditArgs::Network { host: host.clone(), port },
    AuditDecision::Denied { reason: "sandbox policy".into() },
    "myapp.http");
```

## Manifest Configuration

Enable audit logging in `molt.capabilities.toml`:

```toml
[audit]
enabled = true
sink = "jsonl"      # "null" | "stderr" | "jsonl" | "buffered"
output = "stderr"   # Output destination for jsonl sink
```

Or via CLI:

```bash
molt run --audit --audit-sink jsonl worker.py
```

## Example: Enabling Audit Logging for Compliance

For a SOC 2-compliant deployment, enable full audit logging with JSON Lines
output routed to a log collector:

```toml
[manifest]
version = "2.0"
description = "Compliance-audited production deployment"

[capabilities]
allow = ["net", "db.read", "db.write", "env.read"]

[audit]
enabled = true
sink = "jsonl"
output = "stderr"
```

Build and deploy:

```bash
molt build --capability-manifest molt.capabilities.toml app.py
```

At runtime, every capability-gated operation emits a JSON line to stderr. Route
stderr to your log aggregator (Datadog, Splunk, CloudWatch) for retention and
alerting.

Sample output:

```json
{"timestamp_ns":1711612800000000000,"operation":"db.query","capability":"db.read","args":{"kind":"db","query_hash":57005},"decision":{"status":"allowed"},"module":"app.dal"}
{"timestamp_ns":1711612800100000000,"operation":"net.connect","capability":"net","args":{"kind":"network","host":"api.example.com","port":443},"decision":{"status":"allowed"},"module":"app.http"}
{"timestamp_ns":1711612800200000000,"operation":"env.get","capability":"env.read","args":{"kind":"env","key":"DB_URL"},"decision":{"status":"allowed"},"module":"app.config"}
```

## Manual JSON Serialization

`molt-runtime` does not depend on serde. All JSON output is hand-written with
RFC 8259-compliant string escaping, including U+2028/U+2029 escaping for
compatibility with JSON Lines parsers.

## Source Files

- Trait + sinks + macro: `runtime/molt-runtime/src/audit.rs`
- Capability integration: `runtime/molt-runtime/src/vfs/caps.rs`
- Manifest configuration: `molt.capabilities.toml`
