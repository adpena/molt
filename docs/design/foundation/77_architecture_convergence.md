# 77. Architecture convergence: one typed IR and one runtime boundary

Status: accepted 2026-10-14; staged per workstream below. Owner: compiler and
runtime integrators. Ledger: HF-197 to HF-213, HF-185, HF-189, HF-190, HF-195,
HF-198.

## Finding

Two structural boundaries set Molt's performance, binary-size and compile-time
ceiling. Missing passes do not: the Tier-0 analysis substrate (S1-S6 in
`00_integrated_parallel_program.md`) has landed, but its facts do not reach
codegen intact, and compiled code pays a full runtime re-entry for each dynamic
operation.

**The IR boundary.** Native, WASM op_loop, Luau and Rust generate code from
SimpleIR. TIR, where the optimizer works, is lowered back by `lower_to_simple`
(10,131 lines) and re-lifted at 27 sites. 387 of 570 op kinds (68%) survive TIR
only as `Copy` plus a string `_original_kind`. `representation_plan.rs`
re-lifts and re-runs the whole pipeline per function per backend, and maps its
facts back by variable name (a recorded `_v43` collision demoted a value to the
boxed lane). Three optimizers (the Python midend, the doc-76 SimpleIR pipeline
and TIR) repeat constant folding, DCE and exception-check elimination.
`OpIR` has 35 fields whose meaning depends on the kind; TIR attributes are a
string-keyed map read through 815 literal keys.

**The runtime boundary.** Runtime entry costs 32% of a call-heavy loop and
`_tlv_get_addr` alone 19%, because each entry reads several of 96 thread-local
statics (HF-198). Each refcount change is an out-of-line call that also enters
the runtime (HF-205). Runtime calls are indirect (`adrp`+`add`+`blr`).
Satellite crates re-enter through four untyped symbol mechanisms (HF-206).
Builtin calls in loops run 3-15x slower than CPython 3.12: `round` 395-422 ns
against 138-154, `math.trunc` 554-578 against 32-40, `pow(i, 3, m)` 314-333
against 51-56, `enumerate` 215-231 ns per element against 17.6.

**Identity by spelling.** Op kinds, exception classes, class layouts and
decorators are identified by strings. Each has produced a defect: stale
consumers of 54 dead frontend kinds and 42 hidden wire kinds (HF-197),
`except binascii.Error` missing a runtime raise (HF-207), class redefinition
failing with `SystemError` and a user `dataclass` decorator getting the stdlib
transform (HF-203).

## State of the art

- **One declarative op definition, closed generated types.** MLIR ODS and IRDL
  (PLDI 2022; xDSL in Python) generate typed op classes, builders and verifiers;
  effects are traits and interfaces (`MemoryEffectOpInterface`, `Pure`), not
  parallel tables. Cranelift's meta crate generates `Opcode` and typed
  `InstructionData` formats. CPython's cases generator turns one `bytecodes.c`
  DSL into the interpreter, tier-2 uops, optimizer cases, metadata and JIT
  stencils. rustc MIR uses closed enums with exhaustive matches.
- **One-way lowering with facts on values.** MLIR progressive dialect
  conversion and Cranelift CLIF keep facts on SSA values and never re-lift.
  Cranelift runs one aegraph mid-end. LLVM's new pass manager tracks preserved
  analyses; `mlir-opt --verify-each` checks every pass; FileCheck and `.clif`
  filetests test IR as text.
- **Verified rewrites.** Cranelift's ISLE rules are checked with SMT by Crocus
  (ASPLOS 2024) and Arrival (OOPSLA 2025); Alive2 checks LLVM peepholes.
- **A cheap runtime boundary.** CPython keeps one `PyThreadState` per thread and
  inlines `Py_INCREF` with an immortal check; PEP 703 biases refcounts to the
  owner thread. HotSpot, V8 and Go keep the thread or isolate context in a
  register. HPy passes `HPyContext*` to every call. PyPy names its cpyext
  object map as the main cost of emulating the C-API.
- **Typed identity.** CPython raises class objects, not names; mypyc resolves
  decorators to full names before it specializes them.

## Target architecture

### A. One op definition

`op_kinds.toml` grows from fact tables into one row per op: wire spelling,
frontend spelling, producer layer, operand format and typed properties, effects,
ownership, raising, aliases and per-backend status. The generator emits closed
types on both sides: an `OpKind` enum and typed operand structs in Rust, and
typed builders plus a `Literal` kind type in Python. A registered op without a
proven producer, or a consumer of an unproduced op, fails a check
(`tools/op_kinds/census.py`, HF-197). Rewrite rules on impure ops require a type
guard (HF-204).

### B. One typed IR as the backend input

TIR, lowered to LIR, becomes the only input of every backend. Facts live on
`ValueId`s and are computed once by cached analyses. `lower_to_simple`, the
re-lift sites, the name-keyed fact maps and the `Copy`/`_original_kind` carrier
are deleted as each backend moves. Each doc-76 SimpleIR pass is deleted when its
fact family is covered in TIR (SCCP, DCE, exception-check elimination first);
the Python midend keeps only the work that needs AST facts. SimpleIR remains the
frontend's wire format until the frontend emits TIR through generated builders.
TIR gets a textual parser, FileCheck-style tests and verify-each.

### C. One runtime boundary

Compiled code runs attached and carries one execution context: one per-thread
runtime context read once per entry, then passed from compiled code (HF-198).
Refcount, inline-cache guards and type tests have inline fast paths generated
from `molt-codegen-abi` facts; the runtime is called only for slow paths
(HF-205). Runtime imports are direct calls when the link plan is static. A
guarded builtin lowers to a direct runtime call. Satellite crates use one
generated `#[repr(C)]` `RuntimeApi` table carried by the GIL token (HF-206).
Exceptions are raised by generated class handles (HF-207). Native object state
lives in typed layouts with drop and traverse, not global handle maps (HF-208).
The C-API keeps one error slot and O(1) object views (HF-209).

### D. Typed identity everywhere

Class facts are keyed by binding-flow definition identity, and decorators are
resolved through `python_binding_flow` (HF-203). Attribute and property keys
are typed fields of the op format (HF-202). Compiler options are a typed request
struct, not process environment (HF-211).

### E. Verification that matches the claim

Every test file runs in a proof cell (HF-212). Rust tests run process-per-test
under nextest; fuzzing, TSan, Miri and Kani cover the shipped code; a per-merge
instruction-count row guards performance (HF-210). Each source scraper is
deleted when its fact is generated or type-checked (HF-211).

### F. Build and artifact

The runtime splits along feature boundaries (HF-189) so that reachability on the
typed IR selects linked code without stubs (HF-195); proof receipts are keyed by
input digest (HF-190, HF-211).

## What each goal gains

| Goal | Mechanisms |
| --- | --- |
| Runtime speed | C (context, inline fast paths, direct calls); B (facts reach codegen) |
| Binary size | B (reachability on typed IR); F (feature crates, no stubs) |
| Compile time | B (no re-lifts or repeated pipelines; one optimizer); F (digest caching) |
| Ecosystem | C (one error slot, O(1) views, typed API); D (binding identity) |
| DX | A and B (typed builders, IR text tests, verify-each); typed options |
| Correctness | A, D (no identity by spelling); E (tests that run) |

## Order

Each step is a complete migration of one contract, measured before and after.

1. **Gates first.** The op-kind census (HF-197 stage 1, landed with this doc),
   proof-cell ownership for every test file, and the per-merge perf row.
2. **Runtime boundary (C).** It is the largest measured cost and is independent
   of the IR work: the per-thread context, inline refcount, direct runtime
   calls, then guarded builtins as direct calls. Gate: the builtin probe at or
   below CPython 3.12.
3. **Typed identity fixes (D).** The reproduced miscompiles (HF-203, HF-207,
   HF-213) close first because they are wrong answers.
4. **Wire census and dead kinds (A, HF-197 stage 2), then per-kind registry
   rows and the generated `OpKind` (stages 3-4).**
5. **Backends onto LIR (B).** Start with the representation plan keyed by
   `ValueId`; move native, then WASM op_loop, Luau and Rust; delete
   `lower_to_simple` and the doc-76 passes as their families move.
6. **Typed runtime API, object layouts and C-API (C).**

The live finding rows in `docs/agent/V1_HANDOFF_FINDINGS.md` own the status of
each step. This document owns the design and changes only when the design does.
