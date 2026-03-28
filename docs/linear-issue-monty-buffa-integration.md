# Linear Issue: Monty + Buffa Integration Roadmap

**Title:** Epic: Python Edge Platform — Monty + Buffa + Molt Integration

**Priority:** High

**Labels:** `epic`, `security`, `performance`, `architecture`, `integration`

**Estimate:** 6 phases, ~8 weeks

---

## Description

### Context

Deep analysis of pydantic/monty (secure Python interpreter, 6.5K stars) and
anthropics/buffa (pure-Rust protobuf, 471 stars) revealed transformative
integration opportunities with Molt. Together the three projects form a
V8-style tiered execution platform for Python at the edge.

### Objective

Build the most extremely complete, correct, performant, and optimized
Python edge platform possible — combining Monty's instant startup and
sandbox execution with Molt's AOT compilation and 327 stdlib modules,
using Buffa for native-speed wire serialization.

### Design Documents

- `docs/superpowers/specs/2026-03-28-monty-integration-resource-controls-design.md`
- `docs/superpowers/specs/2026-03-28-tiered-execution-vision.md`
- `docs/ROADMAP_MONTY_BUFFA_INTEGRATION.md` (full phase breakdown)

---

## Sub-Issues

### Phase 0: Foundation [DONE]

- [x] MOL-XX: ResourceTracker trait + LimitedTracker
- [x] MOL-XX: Pre-emptive DoS guards (pow, lshift, repeat, bigint mul, str.replace)
- [x] MOL-XX: Uncatchable resource exceptions (dual WASM exception tags)
- [x] MOL-XX: Structured audit logging (AuditSink, 4 sinks, JSON Lines)
- [x] MOL-XX: IO mode toggle (Real/Virtual/Callback)
- [x] MOL-XX: Capability manifest v2.0 (TOML/YAML/JSON)
- [x] MOL-XX: WASM codegen resource_check_time at loop backedges
- [x] MOL-XX: WASM codegen resource_check_op_size before pow/lshift
- [x] MOL-XX: contains_refs container optimization
- [x] MOL-XX: ASCII character interning (128 immortal strings)
- [x] MOL-XX: molt-snapshot crate (execution state serialization)
- [x] MOL-XX: molt-embed SDK crate
- [x] MOL-XX: molt repl command
- [x] MOL-XX: Fuzz targets (NaN-boxing, WASM types, TIR passes)
- [x] MOL-XX: Compile-fail tests + refcount verification mode
- [x] MOL-XX: Documentation (RESOURCE_CONTROLS, AUDIT_LOGGING, CAPABILITIES)

### Phase 1: Wire and Ship

- [ ] MOL-XX: Wire --capability-manifest to build pipeline
- [ ] MOL-XX: Wire resource_on_allocate into WASM host alloc path
- [ ] MOL-XX: CI integration for all new crates and tests
- [ ] MOL-XX: End-to-end resource limit enforcement test

### Phase 2: Correctness and Hardening

- [ ] MOL-XX: Monty conformance suite (250 .py test files)
- [ ] MOL-XX: 24-hour fuzz campaign (3 targets)
- [ ] MOL-XX: Optimization measurement and benchmarks
- [ ] MOL-XX: Review findings backlog cleanup

### Phase 3: Buffa Integration

- [ ] MOL-XX: molt-runtime-protobuf crate (buffa-backed)
- [ ] MOL-XX: AuditEvent.proto schema
- [ ] MOL-XX: Snapshot serialization format evaluation

### Phase 4: Monty C API Bridge

- [ ] MOL-XX: molt-ffi crate (327 stdlib extern "C" wrappers)
- [ ] MOL-XX: Monty host integration example
- [ ] MOL-XX: Upstream PR to pydantic/monty

### Phase 5: Cloudflare Workers Production

- [ ] MOL-XX: Real deployment with resource limits + audit
- [ ] MOL-XX: gRPC at the edge (Python → WASM → buffa wire)

### Phase 6: Tiered Execution

- [ ] MOL-XX: Monty as optional WASM host dependency
- [ ] MOL-XX: Background compilation + atomic swap
- [ ] MOL-XX: PydanticAI code mode integration

---

## Success Criteria

| Metric | Target |
|--------|--------|
| CPython parity (Monty suite) | >95% |
| Resource enforcement | 100% — no bypass |
| Audit overhead (NullSink) | <0.1% |
| Resource check overhead | <1% |
| Protobuf perf | Within 2x native buffa |
| Worker cold start | <50ms with limits |
| Tier-up latency | <100ms |

---

## References

- pydantic/monty: https://github.com/pydantic/monty
- anthropics/buffa: https://github.com/anthropics/buffa
- Molt tiered execution vision: `docs/superpowers/specs/2026-03-28-tiered-execution-vision.md`
- Full roadmap: `docs/ROADMAP_MONTY_BUFFA_INTEGRATION.md`
