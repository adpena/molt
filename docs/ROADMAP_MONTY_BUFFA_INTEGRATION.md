# Molt Integration Roadmap: Monty + Buffa + Edge Platform

**Status:** Active
**Created:** 2026-03-28
**Last Updated:** 2026-03-28
**Tracking:** Linear workspace (see issue link below)

---

## Vision

Molt, Monty, and Buffa together form a **complete Python-at-the-edge platform**:

- **Monty** (pydantic/monty) — secure bytecode interpreter, <1us startup, snapshot/resume
- **Molt** — AOT compiler, 10-100x faster, WASM/native, 327 stdlib modules
- **Buffa** (anthropics/buffa) — pure-Rust protobuf, zero-copy views, no_std+alloc

The tiered execution model (V8-style) uses Monty for cold/one-shot code and Molt for
hot paths, with buffa providing native-speed wire serialization. The shared contract
surface (capability manifest, resource tracker, audit sink, type stubs) ensures
seamless tier-up and identical sandbox enforcement across both runtimes.

---

## Phase 0: Foundation (COMPLETE)

Sprint 2026-03-28. All items verified: 4 crates compile, 40 Rust + 69 Python tests pass.

### Security Infrastructure
- [x] `ResourceTracker` trait + `LimitedTracker` (memory, time, alloc, recursion limits)
- [x] Pre-emptive DoS guards on 5 operations (pow, lshift, repeat, bigint mul, str.replace)
- [x] Uncatchable resource exceptions (dual WASM exception tags)
- [x] Structured audit logging (`AuditSink` trait, 4 sinks, JSON Lines)
- [x] IO mode toggle (Real/Virtual/Callback)
- [x] Capability manifest v2.0 (TOML/YAML/JSON parser, 69 unit tests)

### WASM Codegen Activation
- [x] `resource_check_time` at loop backedges (3 handler paths)
- [x] `resource_check_op_size` before pow/lshift
- [x] `resource_on_allocate`/`resource_on_free` registered in import registry
- [x] All resource violations throw uncatchable tag 1

### Performance Optimizations
- [x] `HEADER_FLAG_CONTAINS_REFS` (bit 19) — O(1) skip for primitive containers
- [x] ASCII character interning — 128 immortal single-char strings
- [x] `refcount_opt` module with heap-ref utilities

### New Crates and Modules
- [x] `molt-snapshot` — WASM execution state serialization (SHA-256 integrity)
- [x] `molt-embed` — embeddable compilation SDK
- [x] `molt repl` — interactive REPL with readline/history
- [x] `capability_manifest.py` — TOML/YAML/JSON manifest parser
- [x] 3 fuzz targets (NaN-boxing, WASM types, TIR passes)
- [x] Compile-fail tests + refcount verification mode

### Documentation
- [x] `RESOURCE_CONTROLS.md` — full ResourceTracker reference
- [x] `AUDIT_LOGGING.md` — full AuditSink reference with JSON schema
- [x] `CAPABILITIES.md` — updated with resource limits, IO mode, audit
- [x] Tiered execution vision document (575 lines)

---

## Phase 1: Wire and Ship (Target: Week of 2026-03-31)

### 1.1 Commit and PR
- [ ] Stage all Phase 0 changes
- [ ] PR: "feat: Monty-inspired resource controls, audit logging, DoS guards, sandboxing"
- [ ] CI green on all new tests

### 1.2 Wire Manifest to Build Pipeline
- [ ] `--capability-manifest` flag feeds `ResourceLimits` to backend daemon
- [ ] `--audit-log` flag installs `JsonLinesSink` at runtime init
- [ ] `--io-mode` flag sets `IoMode` atomic before execution
- [ ] `--type-gate` rejects untyped code in capability-touching paths
- [ ] End-to-end test: build with manifest, verify limits enforced

### 1.3 Wire Allocation Tracking
- [ ] Insert `resource_on_allocate` call in WASM host `alloc` handler
- [ ] Insert `resource_on_free` call in WASM host `dealloc` handler
- [ ] Test: WASM module hitting `max_memory` limit terminates cleanly

### 1.4 CI Integration
- [ ] Add `cargo test -p molt-snapshot` to CI
- [ ] Add `cargo test -p molt-embed` to CI
- [ ] Add `python3 -m molt.capability_manifest` to CI
- [ ] Add `cargo test --features refcount_verify` to CI
- [ ] Add fuzz corpus seeds for continuous fuzzing

---

## Phase 2: Correctness and Hardening (Target: 2026-04-07)

### 2.1 Monty Conformance Suite
- [ ] Download Monty's ~250 `.py` test files from `crates/monty/test_cases/`
- [ ] Add as `tests/monty_compat/` directory
- [ ] Run through Molt's differential testing pipeline
- [ ] Track pass rate (target: >95% on shared subset)

### 2.2 Fuzz Campaign
- [ ] `cargo +nightly fuzz run fuzz_nan_boxing` — 24 hours minimum
- [ ] `cargo +nightly fuzz run fuzz_tir_passes` — 24 hours minimum
- [ ] `cargo +nightly fuzz run fuzz_wasm_type_section` — 24 hours minimum
- [ ] Triage and fix all crashes (P0 priority)

### 2.3 Optimization Measurement
- [ ] Benchmark ASCII interning impact on string-heavy workloads
- [ ] Benchmark contains_refs impact on numeric/data workloads
- [ ] Benchmark resource check overhead (with/without MOLT_WASM_RESOURCE_CHECKS)
- [ ] Publish results in `docs/benchmarks/`

### 2.4 Review Findings Backlog
- [ ] Audit `slice_contains_heap_refs` wiring into dec_ref fast path
- [ ] Add safety multiplier to LeftShift estimate (match Pow's 4x)
- [ ] Document `with_tracker` non-reentrancy in RESOURCE_CONTROLS.md

---

## Phase 3: Buffa Integration (Target: 2026-04-14)

### 3.1 molt-runtime-protobuf Crate
- [ ] Create `runtime/molt-runtime-protobuf/` depending on `buffa`
- [ ] Expose `protobuf.encode()` / `protobuf.decode()` as Python builtins
- [ ] Zero-copy `MessageView` for incoming protobuf data
- [ ] Feature-gated behind `stdlib_protobuf`
- [ ] Tests against buffa's conformance suite

### 3.2 Audit Event Schema
- [ ] Define `AuditEvent.proto` for structured audit logging
- [ ] Binary protobuf encoding option for compact event storage
- [ ] JSON encoding via buffa's JSON codec for human-readable output
- [ ] Benchmark: protobuf vs hand-rolled JSON Lines

### 3.3 Snapshot Serialization Upgrade
- [ ] Evaluate buffa binary encoding for `ExecutionSnapshot`
- [ ] Compare: current hand-rolled vs buffa vs postcard
- [ ] Adopt winner based on size + speed benchmarks

---

## Phase 4: Monty C API Bridge (Target: 2026-04-21)

### 4.1 molt-ffi Crate
- [ ] Create `runtime/molt-ffi/` with `extern "C"` wrappers
- [ ] Expose all 327 stdlib intrinsics via stable C API
- [ ] Generate header file (`molt.h`) for C/C++ consumers
- [ ] Documentation: function signatures, ownership semantics, error handling

### 4.2 Monty Integration
- [ ] Create example: Monty host resolving `OsCall` via `molt-ffi`
- [ ] Test: Monty interprets Python, calls Molt's stdlib for json/math/regex
- [ ] Measure: latency of cross-runtime stdlib calls
- [ ] Propose upstream PR to pydantic/monty

---

## Phase 5: Cloudflare Workers Production (Target: 2026-04-28)

### 5.1 Real Deployment
- [ ] Deploy Worker with resource limits + audit logging
- [ ] Verify: time limits kill runaway loops
- [ ] Verify: memory limits prevent OOM
- [ ] Verify: audit events reach log drain
- [ ] Verify: IO mode virtual correctly sandboxes

### 5.2 gRPC at the Edge
- [ ] Compile Python gRPC service to WASM with buffa wire format
- [ ] Deploy on Cloudflare Workers
- [ ] Benchmark: latency, throughput, memory usage
- [ ] Compare with Go/Rust native implementations

---

## Phase 6: Tiered Execution (Target: 2026-05 onwards)

### 6.1 Monty as Optional WASM Host Dependency
- [ ] Feature-flag `monty` in `molt-wasm-host`
- [ ] Cold imports interpreted by Monty
- [ ] Call counter instrumentation in Molt-compiled code

### 6.2 Background Compilation
- [ ] Tier-up coordinator monitors per-function call counts
- [ ] Background thread compiles hot functions via Molt
- [ ] Content-addressed cache for compiled artifacts

### 6.3 Atomic Swap
- [ ] Replace interpreted entry point with compiled function pointer
- [ ] Zero-downtime transition (no request stalls)
- [ ] Deoptimization path if type assumptions change

### 6.4 PydanticAI Code Mode
- [ ] Monty+Molt powers LLM Python execution
- [ ] Shared capability manifest for AI agent sandboxing
- [ ] Snapshot/resume for long-running agent workflows

---

## Success Criteria

| Metric | Target |
|--------|--------|
| CPython parity (Monty test suite) | >95% pass rate |
| Resource enforcement | 100% — no bypass possible |
| Audit logging overhead (NullSink) | <0.1% |
| Resource check overhead (hot loop) | <1% |
| ASCII interning allocation reduction | >30% on string workloads |
| contains_refs cleanup speedup | >10% on numeric workloads |
| Protobuf encode/decode | Within 2x of native buffa |
| Cloudflare Worker cold start | <50ms with resource limits |
| Tiered execution tier-up | <100ms compile latency |

---

## Key Files Reference

| Concern | Primary File |
|---------|-------------|
| Resource tracking | `runtime/molt-runtime/src/resource.rs` |
| Audit logging | `runtime/molt-runtime/src/audit.rs` |
| DoS guards | `runtime/molt-runtime/src/object/ops_arith.rs` |
| Capability manifest | `src/molt/capability_manifest.py` |
| WASM exception tags | `runtime/molt-backend/src/wasm.rs` |
| WASM resource imports | `runtime/molt-backend/src/wasm_imports.rs` |
| IO mode toggle | `runtime/molt-runtime/src/vfs/caps.rs` |
| Snapshot/resume | `runtime/molt-snapshot/src/lib.rs` |
| Embed SDK | `runtime/molt-embed/src/lib.rs` |
| REPL | `src/molt/repl.py` |
| contains_refs | `runtime/molt-runtime/src/object/refcount_opt.rs` |
| ASCII interning | `runtime/molt-runtime/src/object/builders.rs` |
| Fuzz targets | `runtime/molt-backend/fuzz/fuzz_targets/` |
| Refcount verify | `runtime/molt-runtime/src/refcount_verify.rs` |
| Design spec | `docs/superpowers/specs/2026-03-28-monty-integration-resource-controls-design.md` |
| Tiered vision | `docs/superpowers/specs/2026-03-28-tiered-execution-vision.md` |
