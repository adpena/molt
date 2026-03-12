# P1 Optimization Roadmap — Research-Backed Priority Queue

Based on deep research into compiler optimization literature (March 2026).

## Priority-Ranked Optimizations

| Priority | Optimization | Effort | Expected Impact | Risk |
|----------|-------------|--------|----------------|------|
| **1** | Cross-boundary inlining (TIR-level intrinsic inlining) | 3-4 weeks | 20-40% on call-heavy code | Medium |
| **2** | Perceus borrowing analysis (RC elision) | 2-3 weeks | 15-25% RC reduction | Low |
| **3** | Partial Escape Analysis (scalar replacement) | 3-4 weeks | 10-30% allocation reduction | Medium |
| **4** | NaN-box fused operations | 2-3 weeks | 5-15% arithmetic code | Low |
| **5** | WASM exception handling for StopIteration | 2-3 weeks | 5-15% iterator WASM | Low |
| **6** | Translation validation (SMT/Alive2-style) | 3-4 weeks | Indirect (enables faster opt dev) | Low |
| **7** | Perceus reuse analysis (FBIP) | 3-4 weeks | 10-20% allocation reduction | Medium |
| **8** | Cranelift ISLE custom rules for NaN-box | 1-2 weeks | 5-10% | Low |

## Key References

### Cross-Boundary LTO (#1 bottleneck)
- Recommended approach: TIR-level intrinsic inlining (Julia model) or Cranelift-native runtime hot paths
- Not LLVM cross-lang LTO (Cranelift != LLVM)
- Ref: Rust cross-lang LTO (rust-lang/rust#58057), Julia OOPSLA paper

### Perceus RC (#2)
- Borrowing inference: mark params as borrowed, elide inc/dec at call boundaries
- Reuse tokens: when drop precedes same-size alloc, reuse in-place
- Refs: Reinking et al. PLDI'21, Ullrich & de Moura IFL'19 (Counting Immutable Beans)

### Partial Escape Analysis (#3)
- Track virtual object states through CFG, materialize only on escaping branches
- Refs: Stadler et al. CGO'14, Chris Seaton (TruffleRuby)

### WASM Exception Handling (#5)
- Wasm 3.0 shipped Sept 2025, Cranelift/Wasmtime fully implemented
- Zero-cost happy path for try/catch, eliminates per-iteration StopIteration checks
- Ref: Chris Fallin blog Nov 2025, Wasmtime RFC #36

### Translation Validation (#6)
- Complement to Lean proofs: use SMT for optimization pass validation
- Alive2 found 47 LLVM bugs; 10-50x less effort than full proofs per optimization
- Keep Lean for core invariants, use SMT for optimization correctness

## Top 3 combined: 30-60% improvement in 8-10 weeks
