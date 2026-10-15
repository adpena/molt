# 77. Architecture convergence: one typed IR and one runtime boundary

Status: accepted 2026-10-14; staged per workstream below. Owner: compiler and
runtime integrators. Ledger: HF-197 to HF-213, HF-185, HF-189, HF-190, HF-195,
HF-198.

## Finding

Two structural boundaries constrain performance, binary size and compile time.
The existing Tier-0 analysis owners must converge through them, rather than be
reimplemented. [Century section 4.4](../CENTURY_SYSTEMS_PLAN.md#44-compiler-runtime-abi-and-backends)
owns the governing invariant; this document specifies its compiler/runtime
migration. The findings ledger remains the only release-status index.

**The IR boundary.** Native, WASM op_loop, Luau and Rust still generate code
from SimpleIR. LLVM already consumes owned final TIR. A pre-migration source
census at `cff0ccf03` found 35 OpIR fields and 11 direct production lift callsites,
with additional invocation through wrappers. Of 570 registered wire names,
409 preserve Copy plus `_original_kind`; this is vocabulary cardinality, not a
dynamic operation mix. Original-kind consumers must migrate with the generated
schema, not lose their effect/ownership meaning during enum conversion.

The native migration retains owned final TIR, performs copy-alias, split-field
and direct-raise transformations before final refinement and ownership, and
projects emitted operations and facts together. Every normal/skip/batch consumer
requires that pair; dead-function removal and partitions preserve it. Native
codegen reconstruction is retired. WASM's later operation-reachability boundary
still reconstructs facts and must migrate independently through its actual
transformations. Qualification remains owned by the findings ledger. Facts stay on SSA values; transient source names
are a projection with explicit ambiguity rules, not a new semantic authority.
Overlap between Python, SimpleIR and TIR passes requires semantic retirement
proof through actual callers, not deletion because pass names match.

**The runtime boundary.** Compiled native main already establishes persistent
execution custody; WASM and the public C hooks also have explicit entry/leave
contracts. Nested runtime entry avoids a second global lease but still performs
TLS/depth work. Some satellite helpers discard an existing token and re-enter
private callbacks. Core GIL proof, runtime attachment and callable context are
distinct; a zero-sized CoreGilToken cannot license an unowned RuntimeState
pointer. Owner-bound immutable views are a correctness prerequisite before
context transport spreads. Tk's bytes/bytearray conversion must follow CPython's
actual AsObj policy, not infer a new fast path from a mistaken type branch.

The exploratory runtime-entry/TLS percentages, indirect-call observations and
builtin timings in HF-198/HF-205 are leads from their recorded source/workloads.
They do not establish current-image attribution or universal multipliers. Select
one exact-image workload for the changed boundary and retain source, image,
feature/profile, samples and independent output before choosing or claiming its
optimization. The source-proven serial RuntimeVtable is the first complete
admitted-service aperture; other bridges and public C RuntimeHooks have distinct
feature, attachment and ABI obligations.

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

Primary mechanism references:
[MLIR dialect conversion](https://mlir.llvm.org/docs/DialectConversion/)
for explicit legal transformations and type conversion;
[LLVM's analysis manager](https://llvm.org/docs/NewPassManager.html)
for cached analyses and preservation/invalidation;
[Cranelift IR](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/docs/ir.md)
for explicit SSA/block arguments; and
[CPython's buffer protocol](https://docs.python.org/3/c-api/buffer.html)
for owner-backed views and balanced release. These are design references,
not evidence that Molt's migration or a target cell is complete.

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

Final typed SSA becomes the input of every backend. Materialize a target LIR
only when its lowering consumer needs that representation; shared semantic
authority does not require every backend to allocate the same intermediate
graph. Facts live on `ValueId`s and are computed once by cached analyses. `lower_to_simple`, the
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
guarded builtin lowers to a direct runtime call. Satellite services consume a generated typed API within the existing
execution/core ownership boundary. Migrate the complete current service family
and its publication/feature closure before deleting its wrappers. Preserve the
separate public C attachment/error contract until its own complete migration;
a GIL token alone is not execution or object-lifetime custody.
Exceptions are raised by generated class handles (HF-207). Native object state
lives in typed layouts with drop and traverse, not global handle maps (HF-208).
The C-API keeps one error slot and O(1) object views (HF-209).

### D. Typed identity everywhere

Class facts are keyed by binding-flow definition identity, and decorators are
resolved through `python_binding_flow` (HF-203). Attribute and property keys
are typed fields of the op format (HF-202). Compiler options are a typed request
struct, not process environment (HF-211).

### E. Verification that matches the claim

Every shipped contract has meaningful tests in its claimed proof cells
(HF-212). Isolate process-global runtime tests using the existing child/nextest
custody; pure compiler tests do not acquire runtime isolation costs by default.
Fuzzing, TSan, Miri and Kani exercise actual shipped owners where applicable.
Performance regression checks select reproducible affected workloads and reuse
input-bound evidence; counts, blanket reruns and unmeasured gates do not prove a
contract. Source scrapers retire only when generated types enforce the same
producer/consumer invariant (HF-211).

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

1. **Correctness and custody.** Close reproduced wrong answers, unsafe views
   and producer/consumer drift through the existing owners. Reuse the landed
   op-kind producer census; every subsequent deletion migrates its real family.
2. **Final typed native facts (B).** Move post-TIR semantic rewrites under the
   typed final owner, migrate all codegen entry paths, and delete native plan
   reconstruction. Qualify this bounded deletion cut, then stop extending its
   source bridge. The next native cut carries typed CFG, ownership, module
   roots, callable metadata and partition ABI into actual codegen and deletes
   source projection and name-based plans together. Move the post-cache physical
   splitter under that typed owner before terminal ownership: preserve its
   growth bound, continuation/frame transport, error and finalizer protocol.
   Reusing optimized source and re-lifting its chunks would leave the same
   boundary in place. Reuse LLVM's direct typed
   custody and the existing analysis owners. WASM's operation-reachability
   boundary and the genuine source backends retain their complete migrations.
   Completing the generated enum vocabulary need not block typed transport:
   preserved registered operations retain their generated effects and identity
   until the complete schema migration retires their spelling.
3. **Measured runtime entry (C).** Profile one actual current-image consumer.
   Migrate its complete admitted-service/context family while preserving
   callbacks, isolate changes, shutdown, error and ownership contracts. Choose
   direct calls and inline fast paths from typed link/capability/lifetime facts;
   do not assume a blanket boundary replacement removes every measured cost.
4. **Generated closed vocabulary and typed identity (A/D).** Continue op-kind
   census, operand/property/type generation and binding identities through all
   actual consumers, deleting each replaced spelling-based authority together.
5. **Feature/artifact and ecosystem closure (C/F).** Reachability selects real
   feature crates without stubs. C-API errors/views and package/extension custody
   consume the same lifetime and identity authorities, with actual support-cell
   evidence. Development proof reuse remains outside emitted binaries.

Independent complete families can land concurrently; these dependencies do not
hold one proven repair behind an unrelated open finding. Checks validate the
exact committed inputs and reuse qualified unchanged evidence. Build each
necessary compatible graph once, then execute its affected consumer images.

Freeze each complete contract before its owning execution. Direct-codegen
fixtures preserve authored codegen input; pipeline fixtures exercise
finalization. Establish their SSA, effect, representation and ownership oracles
before building. Reuse unchanged qualified evidence and stop adding unrelated
improvements to the candidate. No additional optimizer, representation cache or
proof coordinator is needed to enforce this delivery unit.

Native still constructs AnalysisFloor LIR for fact extraction, cloning and
refining final TIR. Its initial cache pipeline also requests LIR verification
on hits and misses, constructing that representation before final extraction.
These are user compilation costs, not emitted-program work. Attribute them on
the actual compiler image before claiming an improvement or removing checks;
replace verification with the actual typed/carrier/codegen invariant when the
unused representation retires. A material unexplained regression blocks the
deletion cut. A neutral measurement does not justify restoring duplicate
semantic authorities or indefinitely optimizing the bridge being retired.

The live finding rows in `docs/agent/V1_HANDOFF_FINDINGS.md` own the status of
each step. This document owns the design and changes only when the design does.
