# Compiler Formalization and Verification: State-of-the-Art Research

> **Purpose**: Deep survey of compiler verification techniques and their applicability to Molt's Lean 4 formalization of TIR semantics and pass correctness.
>
> **Date**: 2026-03-12
>
> **Scope**: CompCert, CakeML, Vellvm, Lean 4 proof engineering, Iris/separation logic, Alive2/translation validation, Tree Borrows, abstract interpretation, refinement types, and fuzzing+formal methods hybrids.

---

## Table of Contents

1. [CompCert Deep Dive](#1-compcert-deep-dive)
2. [CakeML End-to-End Verification](#2-cakeml-end-to-end-verification)
3. [Vellvm and Interaction Trees](#3-vellvm-and-interaction-trees)
4. [Lean 4 Compiler Verification](#4-lean-4-compiler-verification)
5. [Iris and Separation Logic](#5-iris-and-separation-logic)
6. [Alive2 and Translation Validation](#6-alive2-and-translation-validation)
7. [Stacked Borrows / Tree Borrows](#7-stacked-borrows--tree-borrows)
8. [Abstract Interpretation in Practice](#8-abstract-interpretation-in-practice)
9. [Refinement Types](#9-refinement-types)
10. [Fuzzing + Formal Methods Hybrid](#10-fuzzing--formal-methods-hybrid)
11. [Synthesis: Applicability to Molt](#11-synthesis-applicability-to-molt)

---

## 1. CompCert Deep Dive

### Overview

CompCert is a verified optimizing compiler from Clight (a large subset of C) to assembly code for multiple architectures, developed in Coq by Xavier Leroy and collaborators at INRIA. It remains the gold standard for end-to-end verified compiler construction.

### Key Proof Techniques

**Simulation Diagrams**: The central verification technique. Each compilation pass is proved correct by establishing a simulation diagram relating source and target transition systems. Four variants are used:

- **Lock-step forward simulation**: Each source step maps to exactly one target step. Used for simple translations (e.g., instruction selection).
- **Star forward simulation**: Each source step maps to zero or more target steps. Used for optimizations that eliminate code (dead code elimination).
- **Plus forward simulation**: Each source step maps to one or more target steps. Used for expansions (e.g., instruction expansion).
- **Backward simulation**: Target steps are related back to source steps. Required when the compiler removes non-termination (e.g., removing infinite loops on dead paths). CompCert proves that forward simulations for deterministic languages imply backward simulations, avoiding the need for direct backward proofs in most passes.

**Compositional Pass Verification**: Each of the 20 compilation passes is verified independently. The simulation diagrams compose transitively: if pass A preserves semantics (source refines A-output) and pass B preserves semantics (A-output refines B-output), then the composition preserves semantics. This compositional structure is what makes 100,000 lines of Coq proof tractable.

**Validation vs. Verification**: CompCert uses a hybrid approach. Most passes are verified (the algorithm itself is proved correct). But register allocation uses an *unverified* OCaml graph-coloring algorithm paired with a *verified validator*: the validator checks each specific output, so the algorithm can be arbitrary as long as the validator accepts. This is a pragmatic compromise that simplifies the hardest proofs.

### Architecture: 11 Intermediate Languages, 20 Passes

The compilation pipeline flows through:

```
C source -> CompCert C (AST) -> Clight -> C#minor -> Cminor
-> CminorSel -> RTL -> LTL -> Linear -> Mach -> Assembly
```

Each intermediate language has a formal operational semantics in Coq. The key insight: **more intermediate languages make verification easier**, because each pass does less work and the simulation invariant is simpler.

### Quantitative Proof Effort

- **100,000 lines of Coq**: 14% compilation algorithms, 10% language semantics, 76% correctness proofs.
- **Six person-years** of effort.
- **Proof-to-code ratio**: Roughly 5.4:1 (proofs vs. compiler code).
- **Zero miscompilations** have been found in the verified parts, despite extensive testing by independent groups (including Csmith).

### Hardest Parts

1. **Register allocation**: The most complex proof, motivating the validation approach.
2. **Memory model**: C's pointer arithmetic, aliasing, and undefined behavior require a sophisticated memory model. CompCert's memory model went through multiple revisions.
3. **Linking and separate compilation**: Originally a gap in the verification; addressed in later work (CompCertO, CompCertM).
4. **Floating-point semantics**: Faithfully modeling IEEE 754 in Coq required substantial effort.

### Lessons Learned

- Designing intermediate representations *for verifiability* (not just for optimization convenience) is critical.
- The proof-to-code ratio (~5:1) is a stable constant across mature verified compilers.
- Validation (checking outputs) is dramatically easier than verification (proving algorithms correct) for some passes.
- Observable behavior (traces of I/O events) as the correctness criterion is both practical and formally clean.

### Key URLs

- [CompCert main page](https://compcert.org/)
- [CACM paper: Formal verification of a realistic compiler](https://dl.acm.org/doi/10.1145/1538788.1538814)
- [CompCert structure diagram](https://www.absint.com/compcert/structure.htm)
- [Xavier Leroy's CompCert publications](https://xavierleroy.org/publi/compcert-CACM.pdf)
- [Cornell CS 6120 CompCert analysis](https://www.cs.cornell.edu/courses/cs6120/2019fa/blog/comp-cert/)

### Applicability to Molt

CompCert's pass-by-pass simulation diagram approach maps directly to Molt's `formal/lean/MoltTIR/` structure. Each TIR pass (desugaring, type inference, specialization, lowering) should have its own simulation theorem. The validation approach is particularly relevant for Molt's Cranelift codegen: rather than verifying Cranelift itself, we can validate that specific Cranelift outputs preserve TIR semantics.

---

## 2. CakeML End-to-End Verification

### Overview

CakeML is a verified compiler for an ML-like functional language, developed in HOL4. It is notable for being the first verified compiler to bootstrap itself inside the logic of a proof assistant: the compiler compiles itself, producing a verified binary.

### Architecture

- **Proof assistant**: HOL4 (Higher-Order Logic).
- **12 intermediate languages** in the backend (more than CompCert's 11).
- **6 target architectures**: x86-64, ARMv6, ARMv8, MIPS-64, RISC-V.
- **Functional big-step semantics**: CakeML defines language semantics using a functional big-step style, which is more natural for functional languages than CompCert's small-step approach.

### End-to-End Verification Scope

CakeML's verification is more comprehensive than CompCert's:

1. **Source language semantics**: Formally specified in HOL.
2. **Compiler correctness**: Every pass proved to preserve semantics.
3. **Verified runtime**: Including a verified generational copying garbage collector.
4. **Verified bignum library**: Arbitrary-precision arithmetic is proved correct.
5. **Self-bootstrapping**: The compiler compiles itself inside HOL4, producing a verified binary that provably implements the compiler.
6. **CFML verification framework**: Post-hoc verification of CakeML programs.

### PLDI 2024: "Much Still to Do"

Magnus Myreen's keynote at PLDI 2024 identified critical open problems:

- **Verified memory management**: Ruling out unwanted out-of-memory errors remains unsolved. Current verified GCs prove functional correctness but not resource bounds.
- **Reusability**: Verified compiler components are largely monolithic. Making passes reusable across different source languages is an open challenge.
- **Integration with verified applications**: Using a verified compiler as part of a larger verified stack (OS, libraries, applications) requires compositional reasoning that current tools struggle with.
- **Wider adoption**: Despite 20+ years of progress, verified compilers remain niche. The gap between the state of the art and practical usefulness is still large.

### Key Differences from CompCert

| Aspect | CompCert | CakeML |
|--------|----------|--------|
| Source language | C (imperative) | ML (functional) |
| Proof assistant | Coq | HOL4 |
| Semantics style | Small-step operational | Functional big-step |
| GC | N/A (C manages memory) | Verified copying GC |
| Bootstrap | No | Yes (inside HOL4) |
| Target count | 4 architectures | 6 architectures |

### Key URLs

- [CakeML project page](https://cakeml.org/)
- [PLDI 2024 keynote: "Much Still to Do"](https://pldi24.sigplan.org/details/pldi-2024-papers/100/Much-Still-to-Do-in-Compiler-Verification-A-Perspective-from-the-CakeML-Project-)
- [Verified CakeML Compiler Backend (JFP 2019)](https://cakeml.org/jfp19.pdf)
- [CakeML: A Verified Implementation of ML (POPL 2014)](https://cakeml.org/popl14.pdf)

### Applicability to Molt

CakeML's verified GC is directly relevant to Molt's RC/GC runtime (`runtime/molt-obj-model/`). The functional big-step semantics style may be more natural for Lean 4 than CompCert's small-step approach. CakeML's experience with reusability problems is a cautionary tale: Molt's formalization should design pass interfaces for composability from the start.

---

## 3. Vellvm and Interaction Trees

### Overview

Vellvm (Verified LLVM) formalizes a large sequential subset of LLVM IR in Rocq (formerly Coq). Its key innovation is using Interaction Trees (ITrees) as a denotational semantics framework, enabling compositional reasoning about impure, recursive programs.

### Interaction Trees: Core Technique

Interaction Trees are a coinductive data structure representing programs that interact with their environment:

```
CoInductive itree (E : Type -> Type) (R : Type) :=
| Ret (r : R)
| Tau (t : itree E R)
| Vis (e : E X) (k : X -> itree E R)
```

- **Ret**: Pure return value.
- **Tau**: Silent step (models non-termination via infinite Tau chains).
- **Vis**: Visible event (I/O, memory access) with a continuation.

ITrees are monadic: they support `bind` and `ret`, enabling compositional construction of interpreters. The key insight: language semantics are defined as *interpreters* that map events to effects, and these interpreters compose modularly.

### Architecture (Rocq)

```
src/rocq/
  Syntax/
    LLVMAst.v          -- Frontend AST from LLVM parser
    CFG.v               -- Control-flow graph representation
  Semantics/
    DynamicValues.v     -- Runtime values
    LLVMEvents.v        -- Event types (memory, I/O, etc.)
    Denotation.v        -- Programs as ITrees
    Handlers/           -- Event handlers (memory, global state, etc.)
    TopLevel.v          -- Complete interpreter
  Theory/
    Refinement.v        -- Refinement relations
    TopLevelRefinements.v -- End-to-end soundness
    DenotationTheory    -- Equational reasoning
```

### Memory Model

Recent work (ICFP 2024) introduced a "Two-Phase Infinite/Finite Low-Level Memory Model" that handles integer-pointer casts -- one of the hardest problems in LLVM formalization. The model:

1. First allocates in an infinite logical address space.
2. Then concretizes to finite machine addresses.
3. Handles pointer-integer round-trips that are ubiquitous in real LLVM IR.

### Why ITrees Matter

**Compositionality**: Unlike operational semantics (used by CompCert), ITrees allow modular construction. Add a new language feature by adding a new event type and handler, without modifying existing semantics.

**Equational reasoning**: ITrees come with a rich equational theory. Compiler correctness proofs reduce to rewriting ITree expressions using proved equations, rather than case-splitting on transition rules.

**Executable extraction**: ITrees can be extracted to OCaml, giving an executable interpreter that serves as a reference implementation and differential testing oracle.

### Current Limitations

- No concurrency support (sequential LLVM only).
- Limited instruction coverage (no switch/resume/invoke, no landing pads).
- Memory model is still evolving.

### Key URLs

- [Vellvm GitHub](https://github.com/vellvm/vellvm)
- [Vellvm: Formalizing the Informal LLVM (NFM 2025)](https://www.cis.upenn.edu/~stevez/papers/nfm25.pdf)
- [Interaction Trees: Representing Recursive and Impure Programs in Coq (POPL 2020)](https://arxiv.org/abs/1906.00046)
- [Two-Phase Memory Model (ICFP 2024)](https://link.springer.com/chapter/10.1007/978-3-031-93706-4_6)

### Applicability to Molt

**High relevance.** Molt's TIR can be given ITree-style denotational semantics in Lean 4. The key benefits:

1. **Modular pass proofs**: Each TIR pass transforms ITrees; correctness is ITree refinement.
2. **Executable reference**: Extract to executable Lean code for differential testing against CPython.
3. **Event-based I/O model**: Maps naturally to Molt's capability-gated I/O system.
4. **Compositionality**: Adding new TIR ops doesn't require rewriting existing proofs.

The main adaptation challenge: ITrees are Coq-native (coinductive). Lean 4 has coinductive types but they are less mature. We may need to define a Lean 4 ITree library or use an alternative (e.g., fuel-based bounded recursion with explicit non-termination modeling).

---

## 4. Lean 4 Compiler Verification

### Overview

Lean 4 is both a programming language and a theorem prover, making it uniquely suited for verified compiler development: the compiler passes can be written in Lean and their proofs live alongside the code.

### Key Proof Engineering Techniques

**Tactic Automation Stack**:

- **`simp`**: Simplification with a configurable set of lemmas. The workhorse tactic.
- **`omega`**: Decision procedure for linear arithmetic over naturals and integers. Essential for index bounds proofs in compiler passes.
- **`decide`**: For decidable propositions. Evaluates the decision procedure at proof time. Useful for finite case analysis in opcode coverage proofs.
- **`aesop`**: White-box best-first proof search. Users tag lemmas with `@[aesop]` to build domain-specific search strategies. Highly relevant for automating routine compiler invariant proofs.
- **`lean-smt`**: SMT tactic that discharges goals to Z3/cvc5. Published at CADE 2025. Enables automated proofs of arithmetic properties without manual case splitting.

**Metaprogramming**: Lean 4's metaprogramming is fully integrated (not a separate macro language). Tactics, elaborators, and code generators are all written in Lean itself, with effectful access to the elaborator's internal state. Key combinators: `getMainGoal`, `inferType`, `unify`, `mkFreshExprMVar`.

**Decidable Procedures**: A proposition is `Decidable` when there is an algorithm to compute its truth value. The `decide` tactic leverages `Decidable` instances for automated proofs. For compiler verification, defining `Decidable` instances for pass preconditions enables fully automated verification of those preconditions.

### Best Practices for Large Proof Developments

From the Hitchhiker's Guide to Logical Verification (2024 edition) and mathlib4 conventions:

1. **Keep `autoImplicit` off** (already enforced in Molt's `formal/lean/`). Explicit universes and types prevent proof brittleness.
2. **Use `structure`/`class` for semantic interfaces**: Define pass interfaces as typeclasses. Passes that implement the interface automatically get access to shared lemmas.
3. **Prefer `theorem` over `lemma`** for key results. Use `lemma` for intermediate steps.
4. **Split files by concern**: One file per pass, one file per semantic domain. Lean 4's module system supports this cleanly.
5. **Use `#check` and `#print` liberally during development** for type exploration.
6. **Minimize `sorry`**: Track all `sorry` instances; they represent proof debt.

### Lean 4 vs. Coq for Compiler Verification

| Aspect | Lean 4 | Coq/Rocq |
|--------|--------|----------|
| Language for proofs & code | Same (Lean) | Separate (Gallina + OCaml extraction) |
| Tactic language | Lean metaprogramming | Ltac/Ltac2 |
| Coinductive types | Supported but less mature | Well-established (ITrees, etc.) |
| Code extraction | Native (compiled Lean) | OCaml/Haskell extraction |
| Community size | Growing rapidly (mathlib: 210k+ theorems) | Mature and large |
| Proof automation | Aesop, omega, simp, lean-smt | auto, omega, lia, CoqHammer |
| IDE support | VS Code (excellent) | VS Code / CoqIDE |

### Key URLs

- [Lean 4 homepage](https://lean-lang.org/)
- [Hitchhiker's Guide to Logical Verification (2024)](https://lean-forward.github.io/hitchhikers-guide/2024/)
- [Lean 4 System Description](https://lean-lang.org/papers/lean4.pdf)
- [Aesop: White-Box Best-First Proof Search](https://dl.acm.org/doi/10.1145/3573105.3575671)
- [lean-smt: SMT Tactic for Lean](https://link.springer.com/chapter/10.1007/978-3-031-98682-6_11)
- [VeriBench: End-to-End Verification Benchmark](https://openreview.net/forum?id=rWkGFmnSNl)
- [Bridging Formal Mathematics and Software Verification (CAV 2024)](https://leodemoura.github.io/files/CAV2024.pdf)

### Applicability to Molt

Molt's existing `formal/lean/MoltTIR/` should adopt these patterns:

1. **Use `aesop` with domain-specific rules** for TIR pass invariant proofs. Tag key lemmas about TIR ops with `@[aesop]`.
2. **Use `omega` for all index/bounds reasoning** in lowering proofs.
3. **Use `decide` for finite opcode coverage**: Define `Decidable` instances for "all opcodes handled" predicates.
4. **Consider `lean-smt`** for arithmetic properties in the specialization pass.
5. **Keep proofs alongside code**: Each TIR pass module should contain both the pass definition and its correctness theorem.

---

## 5. Iris and Separation Logic

### Overview

Iris is a higher-order concurrent separation logic framework implemented in Coq/Rocq. It is the most widely used framework for proving properties of concurrent programs, including compiler correctness for concurrent languages.

### Core Architecture

**Separation Logic Basics**: The assertion `P * Q` means the heap can be split into two disjoint parts, one satisfying `P` and one satisfying `Q`. This enables *local reasoning*: prove a function correct by reasoning only about the memory it touches.

**Iris Innovations**:

- **Higher-order ghost state**: Abstract logical resources that don't correspond to physical memory. Used to track ownership, permissions, and protocol state.
- **Invariants**: Logical assertions that hold at every program step. Protected by namespace-based access control.
- **Step-indexing**: Resolves circularity in recursive predicates. The "later" modality (`|>P`) means P holds after one computation step.
- **Impredicative invariants**: Invariants can quantify over other invariants, enabling reasoning about higher-order concurrent patterns.
- **Prophecy variables**: Enable reasoning about future computation outcomes.

**Resource Algebras (Cameras)**: Iris's generalization of partial commutative monoids. Custom cameras define custom notions of resource ownership. For compiler verification, one might define a camera for register file ownership or memory region permissions.

### Key Instantiations

- **RustBelt**: Proves soundness of Rust's type system, including unsafe code. Uses Iris's lifetime logic to model Rust's borrow checker semantics. Extended by RefinedRust (2024) with refinement types for functional correctness of unsafe Rust code.
- **VST-on-Iris (POPL 2024)**: Applies Iris to verify CompCert C programs. Develops a resource algebra for CompCert's memory model, bridging logical heaps with concrete memory. Key challenge: coherence between logical heap predicates and CompCert's memory representation.
- **Aneris**: Distributed systems verification with network protocols.
- **Actris**: Session-type protocols for message-passing concurrency.

### Proof Automation (2025 PhD Thesis)

Ike Mulder's 2025 PhD thesis on "Proof Automation for Fine-Grained Concurrent Separation Logic" advances automation for Iris proofs, addressing the historically high manual proof burden for concurrent verification.

### Key URLs

- [Iris Project homepage](https://iris-project.org/)
- [VST-on-Iris (POPL 2024)](https://iris-project.org/pdfs/2024-popl-vst-on-iris.pdf)
- [RefinedRust (PLDI 2024)](https://iris-project.org/pdfs/2024-pldi-refinedrust.pdf)
- [Proof Automation for Concurrent Separation Logic (2025)](https://iris-project.org/pdfs/2025-phd-mulder.pdf)
- [RustBelt: Securing the Foundations of Rust](https://plv.mpi-sws.org/rustbelt/popl18/paper.pdf)
- [Beginner's Guide to Iris](https://arxiv.org/abs/2105.12077)
- [Safe Systems Programming in Rust (CACM)](https://cacm.acm.org/research/safe-systems-programming-in-rust/)

### Applicability to Molt

Molt's Rust runtime (`runtime/molt-runtime/`, `runtime/molt-obj-model/`) performs manual memory management with NaN-boxed values, reference counting, and unsafe code. Iris-style reasoning is directly applicable:

1. **NaN-boxed object model safety**: Define a resource algebra for NaN-boxed value ownership. Prove that all unsafe transmutes in `molt-obj-model` preserve type safety.
2. **Reference counting correctness**: Use separation logic to prove that RC operations maintain the invariant "refcount = number of live owners."
3. **Concurrent collections**: The `collections_ext.rs` bug (thread-local to global migration) is exactly the kind of issue separation logic catches: the old `thread_local!` broke the ownership invariant across threads.
4. **CallArgs aliasing**: The `call_bind` use-after-free fix (protecting aliased return values) is a textbook separation logic proof obligation.

**Practical limitation**: Iris is Coq-based. Porting Iris to Lean 4 is a major undertaking. The pragmatic path is to:
- Use Lean 4 for TIR-level proofs (type preservation, pass correctness).
- Use Coq/Iris for runtime-level proofs (memory safety, concurrency) if needed.
- Bridge via shared specifications (both prove conformance to the same IR semantics spec).

---

## 6. Alive2 and Translation Validation

### Overview

Alive2 is a bounded translation validation tool for LLVM IR optimizations. Unlike CompCert's approach of proving each optimization correct universally, Alive2 *checks each specific optimization instance* by encoding source and target as SMT formulas and asking a solver to find a counterexample.

### Core Technique: Refinement Checking

For each optimization instance:

1. **Encode source IR** as an SMT formula capturing all possible behaviors (including undefined behavior, poison, and undef).
2. **Encode target IR** similarly.
3. **Check refinement**: For every input, every behavior of the target must be a valid behavior of the source. Formally: `target refines source` iff for all inputs, `behaviors(target) ⊆ behaviors(source)`.
4. **Query SMT solver**: Ask "does there exist an input where the target produces a behavior not in the source's behaviors?" If yes, the optimization is buggy.

### Handling Undefined Behavior

Alive2's UB model is sophisticated:

- **Immediate UB**: Division by zero, null dereference. Source UB means any target behavior is acceptable (the optimization is vacuously correct).
- **Poison values**: A deferred UB marker. Poison can propagate through computations without triggering UB until it reaches a side-effecting operation.
- **Undef values**: Nondeterministic. Each use of `undef` can resolve to a different value. This is *not* the same as "an arbitrary fixed value."
- **Refinement with UB**: The target may introduce new UB only if the source already had UB. The target may *remove* UB (replacing it with defined behavior).

### Quantitative Results

- **95 bugs found** in LLVM's own unit test suite.
- **30+ miscompilations** caused specifically by incorrect `undef` handling.
- **47 new bugs** reported, 28 fixed, plus 8 patches to the LLVM Language Reference.
- **Runtime**: Full test suite in ~15 CPU-hours (~2 hours on 8 cores).

### Recent Extensions

- **Memory optimization validation**: New SMT encoding of LLVM's memory model enables validation of programs with hundreds of thousands of lines; found 21 additional bugs.
- **NLnet funding**: Ongoing work to extend Alive2's scope to more LLVM features.

### Key URLs

- [Alive2 GitHub](https://github.com/AliveToolkit/alive2)
- [Alive2: Bounded Translation Validation for LLVM (PLDI 2021)](https://users.cs.utah.edu/~regehr/alive2-pldi21.pdf)
- [Alive2 Part 2: Tracking Miscompilations](https://blog.regehr.org/archives/1737)
- [Alive2 Part 3: Undef in LLVM](https://blog.regehr.org/archives/1837)

### Applicability to Molt

Translation validation is the most immediately practical technique for Molt:

1. **TIR optimization validation**: Rather than proving each TIR optimization pass correct universally in Lean 4, encode specific optimization instances as SMT queries and check refinement. This catches bugs without the 5:1 proof overhead.
2. **Cranelift codegen validation**: After Cranelift generates machine code, validate that the generated code refines the LIR. This is cheaper than verifying Cranelift itself.
3. **Differential testing as bounded validation**: Molt's existing `tests/molt_diff.py` is already a form of translation validation (compare Molt output to CPython output). Alive2 shows this can be formalized and made more systematic.
4. **Poison/UB model**: Molt's invariant mining pass can learn from Alive2's experience with UB. Define a clear poison/UB model for TIR so optimizations have well-defined semantics.

**Implementation path**: Build a `tools/tir_alive.py` that takes a TIR optimization instance, encodes source and target TIR as Z3 formulas, and checks refinement. Start with arithmetic optimizations (constant folding, strength reduction) where the encoding is straightforward.

---

## 7. Stacked Borrows / Tree Borrows

### Overview

Stacked Borrows and its successor Tree Borrows are formal aliasing models for Rust, implemented in Miri (Rust's interpreter for detecting undefined behavior). Since Molt's runtime is written in Rust with extensive `unsafe` code, these models define what is and isn't legal in Molt's runtime.

### Stacked Borrows (Original Model)

Each memory location maintains a *stack* of borrow tags. Rules:

- Creating a `&mut` reference pushes a `Unique` tag onto the stack.
- Creating a `&` reference pushes a `SharedRO` tag.
- Accessing memory pops all tags above the one being used, invalidating them.
- **UB** occurs when accessing memory through a tag that has been popped (invalidated).

**Limitations**: Too restrictive. Common safe patterns were flagged as UB:
- Two-phase borrows (creating `&mut` then `&` from the same source).
- `as_mut_ptr()` followed by `as_ptr()` on the same container.
- Container-of patterns common in embedded/systems code.

### Tree Borrows (New Model, PLDI 2025)

Replaces the stack with a *tree* of borrow relationships:

**Permission State Machine**: Each pointer has a permission state:
- **Reserved**: Initial state for `&mut`. Tolerates reads from foreign (unrelated) pointers. Only transitions to Active on first write through this pointer or a child.
- **Active**: Exclusive write access. Foreign reads are UB.
- **Frozen**: Read-only. Foreign writes are UB, but foreign reads are fine.
- **Disabled**: Permanently invalidated. Any access is UB.

**Tree Structure**: Pointers form a parent-child tree. When `y` is derived from `x`, `y` is a child of `x` in the tree. Access through `y` is a "child access" for `x` and a "foreign access" for any sibling of `x`.

**Key Improvements over Stacked Borrows**:
- **Delayed uniqueness**: `&mut` references start as Reserved, not immediately exclusive. This allows common patterns like creating a raw pointer from a mutable reference.
- **54% fewer false positives**: Tree Borrows rejects 54% fewer test cases than Stacked Borrows.
- **Proper two-phase borrow support**: Directly models the pattern used by method calls.
- **Formalized in Rocq**: Unlike Stacked Borrows, Tree Borrows has a machine-checked formalization.

### Miri: Dynamic Checker

Miri is Rust's MIR interpreter with UB detection. Recent developments:

- **Tree Borrows as default**: Now the primary aliasing model in Miri.
- **GenMC integration**: Combines Miri with GenMC (a weak memory model checker) for exhaustive concurrent UB detection. This is the cutting edge of practical Rust verification.
- **Native FFI**: Experimental support for checking UB across Rust/C FFI boundaries.
- **CPU intrinsics**: Coverage through AVX-512, enabling hardware-specific UB detection.

### Key URLs

- [Tree Borrows (PLDI 2025)](https://iris-project.org/pdfs/2025-pldi-treeborrows.pdf)
- [Tree Borrows blog post (Ralf Jung)](https://www.ralfj.de/blog/2023/06/02/tree-borrows.html)
- [Miri: What's New (2025)](https://www.ralfj.de/blog/2025/12/22/miri.html)
- [Stacked Borrows paper](https://plv.mpi-sws.org/rustbelt/stacked-borrows/paper.pdf)
- [Miri GitHub](https://github.com/rust-lang/miri)
- [RustBelt paper](https://plv.mpi-sws.org/rustbelt/popl18/paper.pdf)

### Applicability to Molt

**Critical for Molt's runtime correctness.** The runtime (`runtime/molt-runtime/`, `runtime/molt-obj-model/`) uses extensive `unsafe` Rust:

1. **Run Miri on the runtime**: `cargo +nightly miri test -p molt-runtime -p molt-obj-model`. This catches Tree Borrows violations in existing code. The `tools/runtime_safety.py miri` command should be run regularly.
2. **NaN-boxed object model**: The `transmute`-heavy NaN-boxing code is exactly the kind of unsafe code that Tree Borrows models. Each NaN-box operation creates a new borrow tree node.
3. **Reference counting**: RC increment/decrement involves raw pointer manipulation. Tree Borrows' Reserved-to-Active transition models whether concurrent RC access is safe.
4. **CallArgs aliasing bug**: The fixed `call_bind` use-after-free was a Tree Borrows violation: accessing freed memory through an invalidated (Disabled) pointer.
5. **Collection handles**: The `thread_local!` to `LazyLock<Mutex>` migration for collections is a concurrency correctness issue that GenMC+Miri could validate.

**Recommendation**: Add Miri to CI. Run `cargo +nightly miri test` on every PR that touches `runtime/`. The cost is manageable (Miri is ~10-50x slower than native execution) and it catches real bugs.

---

## 8. Abstract Interpretation in Practice

### Overview

Abstract interpretation is a theory of sound approximation of program semantics. Instead of computing exact program behavior (undecidable in general), abstract interpretation computes an *over-approximation* using abstract domains. If the approximation says "no error," there is no error in the concrete program.

### Astree: Industrial Success Story

Astree proved the absence of all runtime errors in the Airbus A380 flight control software (~130,000 lines of C) before its maiden flight in 2005. Key design principles:

- **Hierarchical reduced product**: Multiple abstract domains (intervals, octagons, polyhedra) combined with partial reduction. Each domain contributes different precision/cost tradeoffs.
- **Modular, extensible architecture**: New domains (choice trees for data case analysis, filters, etc.) plug in without modifying the core.
- **Soundness over completeness**: False positives are acceptable; false negatives are not. Every alarm must be either a real bug or a known imprecision.
- **Scalability through specialization**: Domain-specific abstract domains for common patterns (linear filters, state machines) avoid the exponential blowup of general-purpose domains.

### Abstract Domain Hierarchy

From least to most precise (and most to least efficient):

| Domain | Constraints | Complexity | Use Case |
|--------|------------|------------|----------|
| Signs | `x > 0`, `x < 0`, `x = 0` | O(n) | Very coarse analysis |
| Intervals | `a ≤ x ≤ b` | O(n) | Range analysis, overflow detection |
| Zones | `x - y ≤ c` | O(n²) | Timing analysis |
| Octagons | `±x ± y ≤ c` | O(n²) | Relational properties between pairs |
| Polyhedra | `Σ aᵢxᵢ ≤ b` | Exponential | Full linear relational analysis |

### Compiling with Abstract Interpretation (POPL 2024)

Recent work explores using abstract interpretation *inside* the compiler, not just for static analysis:

- Abstract domains guide optimization decisions (e.g., if the interval analysis proves a value is in [0, 255], use a byte-width operation).
- Abstract interpretation can prove loop invariants that enable vectorization.
- The soundness of the abstract interpretation guarantees that the optimization is correct.

### Patrick Cousot's 2024 Retrospective

Cousot's 2024 paper provides a historical perspective on abstract interpretation's development, emphasizing that the gap between theoretical precision and practical scalability remains the central challenge. The key lesson: *specialized abstract domains* for common program patterns are more effective than increasingly precise general domains.

### Key URLs

- [Astree homepage](https://www.astree.ens.fr/)
- [AbsInt Astree](https://www.absint.com/astree/slides/4.htm)
- [Cousot 2024: Personal Historical Perspective](https://cs.nyu.edu/~pcousot/publications.www/Cousot-FSP-2024.pdf)
- [Compiling with Abstract Interpretation (POPL 2024)](https://dl.acm.org/doi/10.1145/3656392)
- [Abstract Interpretation (Wikipedia)](https://en.wikipedia.org/wiki/Abstract_interpretation)
- [Why Does Astree Scale Up?](https://link.springer.com/article/10.1007/s10703-009-0089-6)

### Applicability to Molt

Abstract interpretation is directly useful for Molt's compiler passes:

1. **Type specialization pass**: Molt's TIR Specialized stage uses invariant mining to narrow types. This *is* abstract interpretation with a custom abstract domain (Molt's type lattice). Formalizing the type lattice as an abstract domain and proving the Galois connection gives soundness for free.
2. **Range analysis for integer operations**: Interval analysis can prove that integer operations don't overflow, enabling direct machine integer lowering instead of arbitrary-precision.
3. **Null/None analysis**: An abstract domain tracking nullability can eliminate None checks in hot paths.
4. **Dead code elimination**: Abstract interpretation over control flow can prove branches unreachable, justifying their elimination with formal soundness.
5. **Proving abstract interpretation soundness in Lean 4**: Define the concrete semantics (TIR operational semantics), the abstract domain (type lattice, interval domain), and the Galois connection. Prove that the abstract transfer functions over-approximate the concrete ones. This is a well-structured proof pattern that Lean 4 handles well.

---

## 9. Refinement Types

### Overview

Refinement types augment standard types with logical predicates. A refinement type `{x : Int | x > 0}` denotes integers that are positive. An SMT solver checks that programs satisfy their refinement type annotations.

### Liquid Haskell

The most mature refinement type system for a general-purpose language:

- **Architecture**: GHC plugin that adds refinement type checking to Haskell's type system.
- **Solver**: Discharges proof obligations to Z3/cvc5/MathSat via SMTLIB2.
- **Expressiveness**: Can express and verify: sorted lists, balanced trees, resource bounds, termination, information flow.
- **Typeclass refinements**: Refinement types can constrain typeclass instances, enabling verified generic programming.

### Refinement Types for Compiler Optimization

The Minotaur superoptimizer (OOPSLA 2024) uses refinement-style reasoning:

- An optimization is expressed as a *refinement check*: "does the target refine the source for all inputs?"
- Wrapped in an exists-forall SMT query: "does there exist a constant valuation such that the candidate refines the source for all inputs?"
- This is essentially Alive2's approach expressed in refinement type terminology.

### Key URLs

- [Liquid Haskell](https://hackage.haskell.org/package/liquidhaskell)
- [Refinement Types course (Nikki Vazou)](https://nikivazou.github.io/lh-course/Lecture_01_RefinementTypes.html)
- [Liquid Haskell through the compilers (Tweag, 2024)](https://www.tweag.io/blog/2024-05-30-lh-upgrades/)
- [Refinement-Types Driven Development (2025)](https://arxiv.org/abs/2509.15005)
- [Minotaur: SIMD Superoptimizer (OOPSLA 2024)](https://users.cs.utah.edu/~regehr/minotaur-oopsla24.pdf)

### Applicability to Molt

Refinement types can enhance Molt's type system and its formalization:

1. **TIR type system as refinement types**: Molt's type specialization pass already infers refined types (e.g., "this variable is always a positive int"). Formalizing this as a refinement type system gives access to established theory (decidability results, inference algorithms, soundness proofs).
2. **Compiler pass preconditions**: Express pass preconditions as refinement types on TIR nodes. E.g., the lowering pass requires `{node : TIR | all_types_resolved(node)}`. The Lean 4 type system can enforce this statically.
3. **SMT-backed verification**: Use `lean-smt` to discharge refinement type obligations in Lean 4, getting Z3-powered automation for compiler invariant proofs.
4. **Optimization correctness**: Express optimization rules as refinement relations and verify them with SMT, following the Minotaur/Alive2 pattern.

---

## 10. Fuzzing + Formal Methods Hybrid

### Overview

Fuzzing and formal verification are complementary: fuzzing finds bugs cheaply but cannot prove absence of bugs; formal verification proves correctness but is expensive. The state of the art combines both.

### Differential Testing (Csmith Lineage)

Csmith established the paradigm for compiler fuzzing:

- Generate random C programs that avoid undefined behavior.
- Compile with multiple compilers.
- Compare outputs: any difference is a miscompilation bug.
- **Results**: 325+ bugs found in GCC, LLVM, and other compilers over 3 years.
- **CsmithEdge (2022)**: Relaxes UB-freedom constraints probabilistically, finding bugs in UB-adjacent code that Csmith misses. Found 7 new miscompilation bugs in GCC, LLVM, and MSVC.

### SpecTest: Specification-Based Compiler Testing

- Uses an executable formal semantics as a test oracle.
- Introduces "semantic coverage": a mutation-based coverage criterion that targets under-tested language features.
- Combines grammar-based fuzzing with semantic oracles to find deep semantic bugs.

### Property-Based Testing as a Bridge

Property-based testing (PBT) occupies the middle ground:

- **Stronger than unit tests**: Tests properties over random inputs, not just specific examples.
- **Weaker than proofs**: Cannot prove universal properties, only increase confidence.
- **Bridge to formal methods**: Properties expressed in PBT can often be lifted to formal specifications. If a property holds for 10,000 random inputs, it's a strong candidate for a formal theorem.

### The K Framework Approach

Runtime Verification's K Framework provides a unified pipeline:

1. Define formal semantics of the language in K.
2. Automatically derive: interpreter, symbolic executor, fuzzer, and formal verifier from the same semantics.
3. Use fuzzing to find bugs quickly; use symbolic execution for bounded verification; use the formal verifier for full proofs.
4. All tools share the same semantic foundation, eliminating specification drift.

### Randomized Testing of Verified Compilers (ICST 2024)

Testing verified compilers (like Dafny's) with fuzzers found 24 bugs, 9 of which were soundness issues. Key insight: even "verified" compilers have bugs in unverified components (parser, linker, runtime, etc.). Fuzzing complements formal verification by testing the gaps.

### Key URLs

- [Csmith GitHub](https://github.com/csmith-project/csmith)
- [Finding Bugs in C Compilers (PLDI 2011)](https://users.cs.utah.edu/~regehr/papers/pldi11-preprint.pdf)
- [CsmithEdge](https://link.springer.com/article/10.1007/s10664-022-10146-1)
- [SpecTest: Specification-Based Compiler Testing](https://pmc.ncbi.nlm.nih.gov/articles/PMC7978860/)
- [Randomised Testing of Verification-Aware Compilers (ICST 2024)](https://www.doc.ic.ac.uk/~afd/papers/2024/ICST.pdf)
- [K Framework: Formal Verification and Fuzzing](https://runtimeverification.com/blog/how-we-build-formal-verification-and-fuzzing-tools)
- [Survey of Modern Compiler Fuzzing (2023)](https://arxiv.org/pdf/2306.06884)

### Applicability to Molt

Molt already has strong differential testing (`tests/molt_diff.py`). The research suggests concrete improvements:

1. **Semantic coverage metrics**: Adopt SpecTest's "semantic coverage" to identify under-tested TIR ops and language features. Current differential tests may cluster around common patterns while missing edge cases.
2. **Grammar-based fuzzing**: Build a Python AST fuzzer that generates programs in Molt's supported subset. Use CPython as the oracle. This is a more systematic version of the current differential testing.
3. **Property-based testing bridge**: Express key compiler invariants as Hypothesis properties in Python. If a property holds for 10,000 random programs, promote it to a Lean 4 theorem. This creates a *pipeline* from testing to proving.
4. **Shared semantics**: Define TIR semantics in Lean 4 and extract an executable interpreter. Use this interpreter as a test oracle alongside CPython, creating a three-way differential test: CPython vs. Molt binary vs. Lean-extracted TIR interpreter. Disagreements between the Lean interpreter and the Molt binary indicate compilation bugs; disagreements between the Lean interpreter and CPython indicate semantic specification gaps.
5. **Miri as runtime fuzzer**: Run Miri on fuzzed runtime inputs to find memory safety bugs in the Rust runtime. Combine with `cargo fuzz` for coverage-guided fuzzing of runtime intrinsics.

---

## 11. Synthesis: Applicability to Molt

### Recommended Formalization Architecture

Based on this research, the recommended formalization strategy for Molt is a **three-tier approach**:

#### Tier 1: Lean 4 Proofs (TIR Level)

**What**: Prove correctness of TIR-level passes using Lean 4.

**Techniques to adopt**:
- **Simulation diagrams** (CompCert-style) for each TIR pass.
- **Aesop + domain-specific rules** for automating routine invariant proofs.
- **`omega`/`decide`** for arithmetic and finite case analysis.
- **`lean-smt`** for SMT-backed discharge of arithmetic obligations.
- **Abstract interpretation formalization**: Define the type specialization pass as a Galois connection; prove soundness.

**Priority passes to verify**:
1. Type inference (TIR -> TIR Typed): prove type soundness.
2. Invariant mining (TIR Typed -> TIR Specialized): prove abstract interpretation soundness.
3. Dead code elimination: prove simulation (every behavior of optimized code is a behavior of the original).
4. Lowering (TIR -> LIR): prove semantic preservation.

#### Tier 2: Translation Validation (Optimization Level)

**What**: Check specific optimization instances rather than proving optimizations universally.

**Techniques to adopt**:
- **Alive2-style SMT checking**: Encode TIR optimization source and target as Z3 formulas; check refinement.
- **Validation, not verification** (CompCert's register allocation lesson): Let optimizations be unverified code; verify their outputs.
- **Bounded checking**: Accept that validation is bounded (not all inputs checked) but catches most real bugs.

**Implementation path**: `tools/tir_alive.py` -- encode TIR arithmetic/boolean optimizations as Z3 queries.

#### Tier 3: Dynamic Checking (Runtime Level)

**What**: Use Miri, fuzzing, and differential testing for runtime correctness.

**Techniques to adopt**:
- **Miri in CI**: Run Tree Borrows checking on every runtime PR.
- **Grammar-based fuzzing**: Generate random Python programs in Molt's subset; differential test against CPython.
- **Property-based testing**: Express runtime invariants as Hypothesis properties; promote stable properties to Lean theorems.
- **Three-way differential**: CPython vs. Molt binary vs. Lean-extracted TIR interpreter.

### Proof Effort Estimates

Based on CompCert's 5:1 proof-to-code ratio and CakeML's similar experience:

| Component | Estimated Code (LoC) | Estimated Proof (LoC) | Priority |
|-----------|----------------------|----------------------|----------|
| TIR type soundness | ~2,000 | ~10,000 | P0 |
| Pass simulation (per pass) | ~500 | ~2,500 | P1 |
| Abstract interpretation soundness | ~1,000 | ~5,000 | P1 |
| Translation validation framework | ~3,000 | ~3,000 (tool code, not proofs) | P1 |
| Runtime memory safety (Iris-style) | ~5,000 | ~25,000 | P2 |

### Key Decision Points

1. **ITree-style semantics in Lean 4**: High reward but requires building a Lean 4 ITree library. Evaluate feasibility with a prototype on a small TIR subset.
2. **Coq vs. Lean 4 for runtime proofs**: Iris/RustBelt are Coq-only. If runtime proofs are needed, consider Coq for runtime + Lean for compiler, with shared specifications.
3. **SMT integration**: `lean-smt` for Lean proofs + Z3 directly for translation validation. Both use SMTLIB2.
4. **Automation investment**: Building domain-specific Aesop rules upfront saves proof effort long-term. Budget 2-4 weeks for tactic development before starting pass proofs.

### Research Papers to Read Next

1. **CompCert memory model evolution**: Leroy & Blazy, "Formal Verification of a C-like Memory Model" (2008). Directly relevant to Molt's NaN-boxed memory model.
2. **Interaction Trees in Lean 4**: No existing library; evaluate porting from Coq's `itrees` library.
3. **RefinedRust (PLDI 2024)**: Refinement types for Rust verification. Directly applicable to Molt's runtime.
4. **Minotaur (OOPSLA 2024)**: Superoptimization with SMT. Template for `tools/tir_alive.py`.
5. **SpecTest**: Specification-based testing methodology. Template for improving Molt's differential test coverage.
