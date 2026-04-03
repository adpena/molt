# Dynamic-First Representation-Aware Backend Design

**Status:** Approved
**Date:** 2026-04-03
**Scope:** Native, WASM, and LLVM-facing backend architecture
**Primary goal:** Replace hint-driven boxed lowering and backend-local shadow
recovery with one dynamic-first, representation-aware SSA contract that is
correct, aggressively optimizable, and shared across backends

---

## 1. Goal

Molt's dynamic implementation should be the real product, not the slow path.
The compiler must preserve Python 3.12+ semantics for the supported Molt
surface while keeping values unboxed for as long as semantics allow.

The immediate architectural goal is:

1. preserve semantic type and representation facts through SSA;
2. make representation transitions explicit;
3. make native, WASM, and future LLVM lowering consume the same backend
   contract;
4. delete backend-local shadow state as an architectural concept.

This design exists to support Molt's actual bar:

- full CPython `>=3.12` parity for supported semantics;
- no host-Python fallback;
- no `exec` / `eval` / `compile`;
- no runtime monkeypatching as a compatibility strategy;
- no unrestricted reflection that breaks AOT determinism;
- maximum dynamic-language performance, with the long-term intent to beat
  CPython broadly and close the gap to the fastest ahead-of-time Python
  compilers without adopting their language restrictions.

## 2. Non-Goals

- Making Molt a strict, annotation-required language in the style of Codon.
- Preserving the current `SimpleIR` hint/shadow architecture for backwards
  compatibility.
- Adding a separate semantics engine for a "fast mode."
- Expanding dynamic execution or reflection policy.
- Building a large new speculative framework before fixing the core compiler
  contract.

## 3. Problem Statement

Molt's current implementation has an architectural mismatch:

1. canonical docs and TIR types already point toward typed SSA;
2. the live backend entry path still degrades many representation facts into
   `SimpleIR` transport hints such as `fast_int`, `fast_float`, and `raw_int`;
3. the native backend then reconstructs unboxed lanes locally with shadow state
   and backend-specific rules.

That has four concrete costs:

1. **Correctness risk.**
   Join points, loop-carried state, and block parameters become fragile because
   the real representation is not first-class in the IR contract.

2. **Performance loss.**
   Boxing/unboxing pairs proliferate because the optimizer cannot treat the
   unboxed lane as the canonical value.

3. **Backend divergence.**
   Native and WASM have to rediscover or approximate the same facts through
   transport-specific hints rather than sharing one lowering contract.

4. **System complexity.**
   Shadow maps, special-case joins, and transport-only hint fields create
   compiler debt that compounds with every new optimization.

## 4. Design Principles

### 4.1 Dynamic-first

The dynamic compiler is primary. Performance comes from representation-aware
optimization beneath Python semantics, not by narrowing the language surface.

### 4.2 Representation is part of SSA

The compiler contract must carry representation in SSA values and block
parameters. Representation cannot remain a side channel recovered late by a
backend.

### 4.3 Explicit boundaries

Boxing, unboxing, widening, overflow escape, and deopt materialization are
explicit IR operations or explicit lowering rules. They are never implicit
backend folklore.

### 4.4 One backend contract

Native, WASM, and future LLVM lowering must consume the same representation-
aware backend IR contract. Backend-specific shadow recovery is prohibited as a
stable architecture.

### 4.5 Correctness before benchmark theater

Every representation optimization must preserve supported Python semantics.
Where an optimization requires guards, the guards and their escape path are part
of the design, not deferred cleanup.

## 5. Canonical Layer Model

The canonical compiler stack remains:

1. `HIR` — desugared high-level structure
2. `TIR` — semantic typed SSA
3. `LIR` — representation-aware SSA plus explicit ownership/layout effects
4. backend lowering — native / WASM / LLVM

### 5.1 TIR responsibility

TIR carries semantic type information:

- `I64`, `F64`, `Bool`, `Str`, `List(T)`, `DynBox`, `Union(...)`, etc.
- explicit control flow
- SSA values and block parameters
- high-level operation semantics

TIR answers: "what does this program mean?"

### 5.2 LIR responsibility

LIR materializes representation and runtime boundaries:

- which SSA values are unboxed machine scalars;
- which values are boxed `DynBox` runtime values;
- where box/unbox/overflow/materialization happens;
- explicit ownership/layout-sensitive operations;
- backend-ready block parameters and joins.

LIR answers: "how is this value represented right now, and where does that
representation change?"

### 5.3 Transport status

Current `SimpleIR` is an implementation transport for the existing backend path.
It is not the canonical long-term backend contract. Migration should reduce its
architectural importance until it is either replaced or reduced to a thin
serialization of the canonical LIR surface.

## 6. Representation Model

### 6.1 Initial required representation lattice

The minimum required LIR representation set is:

- `DynBox`
  - a full runtime value in Molt's NaN-boxed representation
- `I64`
  - proven unboxed signed integer lane
- `F64`
  - proven unboxed float lane
- `Bool1`
  - proven unboxed boolean lane

These are enough to eliminate the worst current boxing churn in arithmetic,
comparisons, control-flow predicates, and loop-carried counters.

### 6.2 Deferred representation lanes

The following are valid future extensions, but are not required to ship the
core redesign:

- typed pointer/reference lanes for runtime-owned objects;
- specialized container payload lanes;
- small-string / bytes view lanes;
- SIMD-friendly contiguous element lanes beyond existing specialized container
  work.

Those lanes should be added only after the base `DynBox` / `I64` / `F64` /
`Bool1` contract is stable across native and WASM.

## 7. Core Invariants

### 7.1 SSA invariants

- Every SSA value has exactly one representation.
- Every block parameter has exactly one representation.
- Every use site must accept the operand's declared representation.
- Representation-changing edges are explicit.

### 7.2 Join invariants

- Phi/join merges happen in the represented lane directly.
- If two incoming edges disagree on representation, the lowering must insert
  explicit conversion before the join or choose a boxed join intentionally.
- Merge blocks must not rely on side-channel shadow state.

### 7.3 Loop invariants

- Loop-carried values preserve representation through block parameters.
- Induction variables proven `I64` remain `I64` through the loop body until an
  explicit escape to `DynBox` is required.
- Overflow exits are explicit control-flow edges, not implicit truncating
  reboxing tricks.

### 7.4 Call invariants

- Internal calls may use specialized signatures when both caller and callee are
  compiled under the same representation contract.
- Runtime/library boundaries use explicitly boxed calling conventions unless a
  shared specialized ABI has been defined and validated.
- External or unknown calls conservatively materialize to `DynBox`.

## 8. Lowering Rules

### 8.1 Arithmetic and comparisons

- `I64 + I64 -> I64` stays unboxed unless overflow requires an explicit escape.
- `F64` arithmetic stays in `F64`.
- `Bool1` predicates stay unboxed through branches and comparisons.
- `DynBox` arithmetic remains a runtime-dispatched path unless upstream guards
  or refinement prove a narrower lane.

### 8.2 Overflow and widening

- Overflow from `I64` arithmetic is an explicit transition.
- The default widening target is `DynBox`, materializing either an inline int
  when representable or a heap bigint when required by semantics.
- Overflow handling must preserve exact Python integer semantics for supported
  operations.

### 8.3 Branch conditions

- Branches over `Bool1` consume `Bool1` directly.
- Boxed truthiness checks materialize a boolean lane only once, then branch on
  the explicit result.

### 8.4 Boxing boundaries

- Boxing occurs at ABI boundaries, polymorphic joins, heap/container storage,
  and any operation that semantically requires a full runtime value.
- Unboxing occurs only when proved safe by semantic type refinement or explicit
  guards.

## 9. Backend Contract

### 9.1 Native backend

The native backend consumes representation-aware SSA values directly. It must
not reconstruct unboxed integer or float lanes by maintaining shadow maps keyed
to boxed variable names.

### 9.2 WASM backend

The WASM backend consumes the same representation-aware SSA contract. It may
choose different code sequences than native, but not a different semantic model
for joins, overflow, or materialization.

### 9.3 LLVM backend

Any revived LLVM lane must consume the same LIR contract rather than inventing
its own notion of "fast" values.

## 10. Migration Strategy

### 10.1 Immediate architectural move

Introduce explicit representation-aware LIR and make it the backend source of
truth. Do not expand `fast_int` / `raw_int` / shadow architecture further.

### 10.2 Allowed migration staging

Migration may be staged by operation family, but with strict rules:

- no new long-lived user-visible fallback lane;
- no new shadow-based abstractions;
- no permanent dual architecture;
- old transport/hint machinery remains only until the corresponding op family is
  cut over and verified.

### 10.3 Recommended cutover order

1. constants, phis, block params, box/unbox
2. integer arithmetic and comparisons
3. boolean/control-flow predicates
4. float arithmetic
5. loop-carried induction variables
6. internal call signatures
7. container/storage boundaries
8. WASM and LLVM parity completion

## 11. Verification Requirements

Every migration phase must ship with:

- differential correctness coverage for the touched op families;
- native and WASM parity checks for the touched semantics;
- targeted regression tests for join points and loop-carried state;
- benchmark evidence on the affected hot paths;
- explicit confirmation that no new host-Python or dynamic fallback path was
  introduced.

Required benchmark families include at minimum:

- sieve / loop-heavy integer kernels;
- attr/class-heavy workloads;
- exception-heavy workloads;
- string kernels;
- bytes/bytearray search kernels;
- startup and build/daemon-sensitive workflows where compile-time overhead is
  affected by the new IR path.

## 12. Consolidation Policy

This document is the canonical backend architecture for representation-aware
lowering.

To prevent drift, the repo should not keep parallel sprint-era backend design
documents once their architecture has been replaced. The previous 2026-03-30
conformance/perf sprint docs should be deleted rather than maintained as
"historical but still present" competitors to the canonical backend contract.

From this point forward:

- `0100_MOLT_IR.md` is the stable IR contract;
- this document is the stable backend-architecture design for representation
  lowering;
- implementation plans may reference this design, but must not redefine the
  backend contract;
- shadow/hint architecture notes belong only in current-state audits, not in
  parallel design docs.
