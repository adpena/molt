# PR: Cloudflare demo hardening, conformance suite, and protobuf integration

## Summary

Monty-inspired resource controls, audit logging, quality harness, formal verification scaffolds, and integration modules for the Python-at-the-edge platform (Molt + Monty + Buffa). This sprint focused on hardening the Cloudflare Worker deployment path, establishing a conformance baseline against CPython, and wiring protobuf encoding into the runtime.

**69 commits** | **58 files changed** | **+3,946 / -8,109 lines** (net reduction from removing stale vendor/cranelift-frontend)

## What Changed

### Security Infrastructure
- ResourceTracker trait with pluggable LimitedTracker (memory, time, allocation, recursion limits)
- Pre-emptive DoS guards on 5 operations (pow, lshift, repeat, bigint mul, str.replace)
- Uncatchable resource exceptions via dual WASM exception tags
- Structured audit logging (AuditSink trait, 4 sinks, JSON Lines)
- IO mode toggle (Real/Virtual/Callback) for sandbox switching
- Capability manifest v2.0 (TOML/YAML/JSON) with resource limits and audit config
- `--type-gate` CLI flag for typed capability enforcement
- `--audit-log` and `--io-mode` CLI flags wired to build and run subcommands
- SSRF prevention hardening in capability gates
- SHA-256 integrity pin on `molt_runtime.wasm` for supply chain verification

### WASM Codegen & Backend
- `resource_check_time` emitted at loop backedges (`MOLT_WASM_RESOURCE_CHECKS=1`)
- `resource_check_op_size` before pow/lshift operations
- `resource_on_allocate`/`free` wired into alloc/dealloc paths
- All resource violations throw uncatchable tag 1
- Trampoline targets preferred for WASM indirect calls
- Fixed `br_if` lowering for split-runtime imports
- Fixed `call_guarded` missing closure env arg (type mismatch)
- Fixed class dispatch, attr IC separation, type indexing
- Removed broken `_fixup_func_type_indices` from WASM post-link
- Hardened split-runtime Cloudflare Worker pipeline
- Hardened backend cache keys with fresh WASM validation

### TIR & Compiler Fixes
- Fixed or/and operators returning wrong value after TIR linearization
- Fixed infinite loop regression: TIR linearizes loops but Cranelift created unused loop blocks
- Enabled TIR optimization for functions with loops
- Fixed `dec_ref` emission at loop backedge for reassigned variables
- Fixed float coercion guard on arithmetic int fast paths
- TIR verifier expanded (+138 lines)

### Formal Verification
- Lean 4 formalizations: TIR syntax (+60 lines), types (+77 lines), backend semantics
- Luau and Rust backend emission/semantics specifications
- Formalization audit document with certification status
- Checkpoint of formal and TIR verification work

### Protobuf Integration (molt-runtime-protobuf)
- Schema-driven protobuf encode/decode (`encode_message`, `decode_message`)
- AuditEvent protobuf schema with encode/decode convenience functions
- Varint codec, field encoding primitives
- `@molt.proto` decorator with CPython-fallback encode/decode (277-line Python module)

### Quality Harness
- `molt harness quick|standard|deep` with 16 layers
- Baseline ratchet system (quality only moves forward)
- Dynamic workspace crate discovery (resilient to churn)
- 12/16 layers implemented (fuzz, conformance, bench, size + quick + standard)
- Harness quick profile fully green (4/4 PASS)

### Conformance Suite
- Monty conformance adapter for Molt differential testing
- Batch Molt conformance runner with warmup optimization
- Conformance analysis document with categorized failure patterns
- CPython baseline: 385/409 (94%)

### Runtime Fixes
- Fixed metaclass pointer on dynamically-created exception class objects
- Cleared stale pending exception after itertools class base setup
- Hardened ColdHeaderSlab against out-of-bounds free from stale references
- Fixed six compilation: stdlib sys bootstrap payload use-after-free
- Fixed module cache deletion race in exception handler
- Fixed manifest one-shot race and dangling `BUILTINS_MODULE_PTR`
- NaN-boxing fuzz target sign-extension bug fixed

### Tooling & Deployment
- Post-deploy live endpoint sweep tool for Cloudflare Workers
- CI workflow additions: explicit test steps for Phase 1.4 crates and Python modules
- Removed stale `vendor/cranelift-frontend-0.130.0` (-7,821 lines)
- WASM binary excluded from repo via `.gitignore`

### New Files
| File | Purpose |
|------|---------|
| `runtime/molt-runtime-protobuf/src/{encode,decode,audit_event}.rs` | Protobuf codec + audit event schema |
| `runtime/molt-runtime/tests/resource_enforcement.rs` | Resource enforcement integration tests |
| `runtime/molt-snapshot/benches/format_comparison.rs` | Snapshot format benchmark |
| `src/molt/proto.py` | `@molt.proto` decorator |
| `tests/harness/{adapt_monty_tests,run_molt_conformance}.py` | Conformance adapter and runner |
| `tests/harness/conformance_analysis.md` | Failure pattern analysis |
| `tests/test_capability_manifest_e2e.py` | Capability manifest integration tests |
| `tests/test_manifest_cli_integration.py` | CLI manifest integration tests |
| `tests/test_proto.py` | Proto decorator tests |
| `tools/cloudflare_demo_deploy_verify.py` | Post-deploy verification tool |
| `docs/spec/areas/formal/FORMALIZATION_AUDIT_2026-03-29.md` | Formal verification audit |

## Test Plan

- [ ] `molt harness quick` passes 4/4 layers (compile, lint, unit-rust, unit-python)
- [ ] Rust tests pass across all workspace crates (`cargo test --workspace`)
- [ ] Python manifest tests pass (`pytest tests/`)
- [ ] 6 proto decorator tests pass (`pytest tests/test_proto.py`)
- [ ] 5 capability manifest e2e tests pass (`pytest tests/test_capability_manifest_e2e.py`)
- [ ] Resource enforcement tests pass (`cargo test -p molt-runtime --test resource_enforcement`)
- [ ] WASM compilation tests pass (`cargo test -p molt-backend --test wasm_compilation`)
- [ ] 385/409 Monty CPython conformance (94% baseline)
- [ ] `cargo clippy -p molt-backend -- -D warnings` clean
- [ ] No WASM binary checked into repo

## Breaking Changes

None. All new functionality is additive or behind feature flags / environment variables.

## Related

- Cloudflare follow-on work now rolls into `docs/superpowers/plans/2026-03-29-consolidated-monty-buffa-and-waves.md` after the 2026-03-30 plan audit.
- Design spec: `docs/superpowers/specs/2026-03-28-cloudflare-demo-hardening-design.md`

---

Generated with [Claude Code](https://claude.com/claude-code)
