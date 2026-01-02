# Molt Roadmap: The Evolution of Python

Molt compiles a verified subset of Python into extremely fast, single-file native binaries and WASM. This document tracks our progress from research prototype to production-grade systems runtime.

**Ultimate Goal:** A Go-like developer experience for Python, producing binaries that rival C/Rust in performance and safety, suitable for high-concurrency web services, databases, and data pipelines.

**Source of truth:** This file is the canonical status tracker. For near-term sequencing, see `ROADMAP_90_DAYS.md`. For historical milestone framing, see `docs/spec/0006-roadmap.md`.

---

## 🚀 Milestone Status

| Feature | Status | Date Completed | Notes |
| :--- | :--- | :--- | :--- |
| **0. Technical Specification** | ✅ Done | 2026-01-02 | Defined IR stack, tiers, and security model. |
| **1. NaN-boxed Object Model** | ✅ Done | 2026-01-02 | Efficient 64-bit tagged pointer representation. |
| **2. Tier 0 Structification** | ✅ Done | 2026-01-02 | Fixed-offset attribute access for typed classes. |
| **3. AOT Backend (Native)** | ✅ Done | 2026-01-02 | Cranelift-based machine code generation. |
| **4. AOT Backend (WASM)** | ✅ Done | 2026-01-02 | Direct WebAssembly bytecode generation. |
| **5. Tier 1 Guards** | ✅ Done | 2026-01-02 | Runtime type-check specializing hot paths. |
| **6. Molt Packages (JSON)** | ✅ Done | 2026-01-02 | Rust-backed `molt_json` via high-perf FFI. |
| **7. Differential Testing** | ✅ Done | 2026-01-02 | Automated verification against CPython 3.12. |
| **8. True Async Runtime** | ✅ Done | 2026-01-02 | State-machine lowering + Poll-based ABI. |
| **9. Closure Conversion** | ✅ Done | 2026-01-02 | Async locals stored in Task objects. |
| **10. WASM Host Interop** | ✅ Done | 2026-01-02 | Standardized host imports for async/memory. |
| **11. Garbage Collection** | 📅 Backlog | - | RC + Incremental Cycle Detection. |
| **12. Profile-Guided Opt (PGO)** | 📅 Backlog | - | Feedback-driven specialization. |
| **13. Performance Benchmarking** | ✅ Done | 2026-01-02 | Automated suites vs CPython 3.12. |
| **14. Multi-Version Compliance** | ✅ Done | 2026-01-02 | CI Matrix for Python 3.12, 3.13, 3.14. |
| **15. Compliance Scaffolding** | ✅ Done | 2026-01-02 | `tests/compliance/` structure for future specs. |
| **16. MLIR Pipeline** | 📅 Backlog | - | Domain-specific optimizations for data tasks. |

---

## 🛠 Feature Checklist & Implementation Details

### Core Compiler
- [x] Python AST to Molt TIR (Typed IR) Lowering
- [x] Invariant Mining (Stable Class Layouts)
- [x] Monomorphization (Function Specialization)
- [x] Global Data Support (String Constants)
- [x] Position Independent Code (PIC) for macOS/Linux
- [ ] Closure Conversion (for lambdas and inner functions)
- [ ] List/Dict Comprehension Lowering

### Runtime & Performance
- [x] NaN-Boxing (Inline Ints, Bools, None)
- [x] Static Dispatch for Tier 0
- [x] Guarded Dispatch for Tier 1
- [x] External Rust FFI
- [ ] Biased Reference Counting (Single-thread optimization)
- [ ] Incremental Mark-and-Sweep GC
- [ ] Zero-copy String passing for WASM

### Concurrency & I/O
- [x] Async/Await Syntax Support
- [ ] Task-based Concurrency (No GIL)
- [ ] Rust Executor Integration (Tokio/Smol)
- [ ] Native HTTP Package (`molt_http`)
- [ ] Native Database Drivers (`molt_sqlite`, `molt_postgres`)

### Tooling & DX
- [x] `molt build` CLI
- [x] Cross-compilation to WASM
- [x] `molt-diff` Harness (CPython Semantics Matcher)
- [ ] `molt run` (JIT-like execution)
- [ ] SBOM Generation (SPDX/CycloneDX)
- [ ] Integrated Benchmarking Regression Gates

---

## 🔬 Research & Innovation Areas
1. **Semantic Reduction via Invariant Mining:** Automatically identifying which parts of a Python app are "frozen" vs "guarded".
2. **AI-Assisted Guard Synthesis:** Using dev-time traces to generate optimal guards for dynamic sites.
3. **WASM Capability Boundaries:** Defining strict security manifests for third-party Molt Packages.
4. **Deterministic WASM:** Ensuring identical execution for database triggers or smart contracts.

---

*Last Updated: Friday, January 2, 2026 - 04:30 UTC*
