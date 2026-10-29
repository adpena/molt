# 77. Architecture convergence: one typed IR and one runtime boundary

Status: accepted 2026-10-14; staged per workstream below. Owner: compiler and
runtime integrators. Ledger: HF-197 to HF-213, HF-185, HF-490, HF-190, HF-195,
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
codegen reconstruction is retired. WASM shares the retained module owner and
performs typed operation reachability before release insertion. Its final
projection pairs each emitted body with scalar facts and a demanded LIR plan;
function retention and escaped-callable admission preserve that pair.
Qualification remains owned by the findings ledger. Facts stay on SSA values; transient source names
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

The existing entry hot/cold split must reach each runtime helper across Rust
code-generation units. `RuntimeExecutionGuard`'s policy-selecting entry wrappers
and borrowed-token accessors, including the native `GilGuard` accessor, expose
their small bodies for inlining. The lifecycle admission and finalization bodies
remain explicitly cold and out of line. This follows Rust's
[`inline` contract](https://doc.rust-lang.org/reference/attributes/codegen.html#the-inline-attribute):
an inlined private callee alone does not expose its enclosing wrapper to callers.
The policy, lifecycle leases, GIL nesting, token lifetimes and cleanup edges are
unchanged. The cost trade is repeated fast-prefix machine code versus removed
call/return and guard-result transport. Acceptance requires matching guest-latency
and linked-size evidence on the selected release profile. Comparative cold Rust
runtime-build cost remains unqualified until a matched rebuild comparison is
available; the current policy accepts that unknown for measured guest throughput.

The exploratory runtime-entry/TLS percentages, indirect-call observations and
builtin timings in HF-198/HF-205 are leads from their recorded source/workloads.
They do not establish current-image attribution or universal multipliers. These
profiles and source findings can select an experiment. Before landing a
load-bearing performance change, compare its affected workload before and after,
retaining source, image, feature/profile, samples and independent output. Public
performance acceptance remains the qualified V1-07 matrix; it need not precede
every development experiment. The source-proven serial RuntimeVtable is the
first complete admitted-service aperture; other bridges and public C RuntimeHooks have distinct
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
  conversion makes legality and type conversion explicit; Cranelift CLIF
  keeps optimizer facts on SSA values.
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

Phase transitions carry ownership as well as validity. [rustc's MIR queries](https://rustc-dev-guide.rust-lang.org/mir/passes.html)
group transformations into explicit intermediate states and move intermediate
bodies to avoid cloning; [MLIR's analysis manager](https://mlir.llvm.org/docs/PassManagement/#analysis-management)
binds demanded analyses to an operation and invalidates them when preservation
does not hold. Molt's final backend transition follows those mechanisms:
mutable typed transformations precede ownership finalization, then an immutable
body/fact pair reaches emission. Full and exception-only release insertion both
close dead-operation mutation. Existing phase owners enforce this boundary;
there is no second scheduler, optimizer or runtime state-machine interpreter.
WASM generic and LIR planning share one value-range/representation derivation.
Facts remain live until their body is emitted, so peak compiler memory and
latency must be measured alongside eliminated reconstruction. This is compiler
work, with no new guest execution, allocation or validation step.

### Current comparison

Source inspection on 2026-10-16 distinguishes incomplete adoption from absent
infrastructure. Molt already has a pass/analysis manager, generated operation
effects, differential/property/fuzz tests, shrinking, source custody and
content-addressed caches. Reimplementing those owners would increase cost.
The findings ledger owns each remaining obligation; this comparison adds no
parallel roadmap or release gate.

| Practice in the referenced projects | Current Molt difference and owning obligation |
| --- | --- |
| [MLIR ODS](https://mlir.llvm.org/docs/DefiningDialects/Operations/) generates operation interfaces, verification and builders from declarations. | Generated op facts exist, but generic string-keyed operands/properties and ABI signature scraping remain. Complete their typed producer/consumer migration under HF-197/HF-202/HF-211. |
| [MLIR analysis management](https://mlir.llvm.org/docs/PassManagement/) and [rustc queries](https://rustc-dev-guide.rust-lang.org/query.html) cache demanded facts and track invalidation or dependencies. | The shared analysis owner exists. Native and WASM retain final TIR and project body/fact pairs, but still project names/control flow for codegen. Complete the existing HF-201 migration rather than add another optimizer. HF-250 owns unchanged-input frontend work. |
| [Cranelift](https://cranelift.dev/) explicitly avoids pathological compiler complexity and uses fuzzing, custom oracles and validation. [MLIR filetests](https://mlir.llvm.org/getting_started/TestingGuide/) exercise individual transformations without full application builds. | Reuse existing fuzz/shrink and pass-test authorities. Qualify affected semantic and compiler-resource properties, and retain minimized failures; an inventory or test count does not prove their execution. HF-212 owns coverage, while V1-07 owns resource/performance acceptance. |
| [Wasmtime minimal builds](https://docs.wasmtime.dev/examples-minimal.html) select runtime features separately from compilation. | Molt's feature/reachability owners must close actual linked binary size and startup costs, including remaining stubs, under HF-189/HF-195. Development custody and verification belong outside emitted programs. |
| [Wasmtime's VMContext](https://docs.wasmtime.dev/contributing-architecture.html) gives compiled code access to runtime state through an explicit context. | Preserve Molt's existing attachment and object-lifetime owners while carrying context through actual callers under HF-198/HF-205. A core GIL token alone does not prove either lifetime. |
| [CPython's tar extraction policy](https://peps.python.org/pep-0706/) belongs to its public, versioned library semantics. | Compile pinned CPython semantics over shared guarded I/O and bounded codec primitives. Retire the duplicate tar parser and host-tar path with every consumer under V1-19/V1-30/HF-122/V1-35/HF-208; a copied module cannot qualify missing path, stream or version primitives. |
| [V8](https://v8.dev/blog/wasm-speculative-optimizations) uses feedback, guarded specialization and deoptimization. | Molt needs proven/guarded compiled paths and connected profiling, preserving standalone execution. An interpreter-dependent recovery design does not satisfy that contract. HF-198/HF-205/HF-206 own repeated runtime boundary work; W1/HF-222 own connected feedback and regression history. |
| [TypeScript 7](https://devblogs.microsoft.com/typescript/announcing-typescript-7-0/) parallelizes independent stages while documenting duplicated worker work, ordering and memory tradeoffs. | Recent TIR cache and batch changes already address serialization. Qualify deterministic output and total time/memory through those owners; more workers or a language rewrite alone does not establish a gain. |

These projects make different optimization tradeoffs; there is no single design
that minimizes every metric. Molt's policy must select measured latency,
throughput, code size, memory and compilation costs for the actual target and
profile. Rich Python semantics, standalone AOT execution and co-equal native/WASM
targets are product constraints. They justify necessary machinery, not duplicate
semantic authorities, inactive optimization paths or unmeasured release claims.

Optimization work follows the same information and ownership flow: preserve
types, effects, ranges and alias/ownership facts; use them to remove allocations,
fuse intermediate collections and traversals, specialize admitted calls and
eliminate unreachable code. Fusion preserves evaluation order, exceptions,
callbacks and observable object identity. Reachability includes admitted dynamic
roots and reflection. Demand-driven analysis and input fingerprints support
early cutoff and bounded invalidation; deterministic identities and atomic
publication make those caches reusable. Compact layouts, locality, fewer atomics
and persistent execution custody address runtime cost. Measure compile/link
latency, startup, throughput, live memory and linked bytes together. Development
verification and provenance do not become emitted-program work.

Native buffer custody follows the existing
[crate extraction contract](70_molt_runtime_crate_extraction.md). C-API protocol
storage is independent of runtime-core; the small runtime/C-API integration crate
consumes both owners and is consumed by native-buffer satellites. Pure Unicode
and text-storage facts belong to the existing object-model crate. This prevents
standalone C-API linkage from importing unrelated runtime bridge symbols.
Moving a function between packages may change its optimization profile; source
equivalence does not establish shipping latency or binary-size acceptance.

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

The [CPython call protocol](https://docs.python.org/3/c-api/call.html) selects
call APIs to match the argument representation already available. Its vectorcall
benefit depends on the callee consuming the array form directly. Carry that
choice through the existing typed callable ABI: materialize argument tuples or
dictionaries where the public protocol requires them, account for conversions,
and preserve ownership, recursion and exception behavior. Calling convention and
keyword admission remain distinct facts under V1-26/HF-211; a transport choice
cannot redefine the callable contract.

Public Python library semantics belong to the pinned CPython source for the
selected target version. Native primitives own necessary I/O, codecs, memory and
ABI operations. Filename entry points share the existing capability and VFS
owner before host access; module or intrinsic availability is not authorization.
Source reuse must migrate its prerequisite primitives, imports and generated
intrinsic consumers together. Archive filters, callback dispatch and format
selection must not acquire a second implementation in Rust or an ambient host
program. Requested output bounds and owner-backed streaming state bound retained
memory and avoid repeated whole-input decoding or copying (V1-19/V1-30).

Incremental BZ2/LZMA decoding shares `molt-runtime-compression/src/stream.rs`:
one persistent maintained engine, borrowed fresh input, an amortized cursor for
retained compressed tails, and output windows bounded by the caller's request.
The [pinned BZ2](https://github.com/python/cpython/blob/v3.12.13/Modules/_bz2module.c)
and [LZMA](https://github.com/python/cpython/blob/v3.12.13/Modules/_lzmamodule.c)
modules govern EOF, zero-output calls, unused input and their distinct
`needs_input` rules. Failed processing retires pending input so it cannot replay
against an advanced engine. Argument conversion reuses the native integer
owner, with an immediate-integer fast path. Zlib keeps its distinct public
`max_length=0` and unconsumed-tail protocol. File-object, raw-filter and target
qualification remain with the existing release obligations.

### D. Typed identity everywhere

Class facts are keyed by binding-flow definition identity, and decorators are
resolved through `python_binding_flow` (HF-203). Attribute and property keys
are typed fields of the op format (HF-202). Compiler options are a typed request
struct, not process environment (HF-211).

Backend language and runtime OS are separate facts. Native and WASM requests
carry their selected runtime platform; Rust, Luau and MLIR source emission
preserves an unknown OS until one is selected. Platform-specific namespace and
optimization decisions require positive target facts and never inherit the
compiler host.

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

The runtime splits along feature boundaries (HF-490) so that reachability on the
typed IR selects linked code without stubs (HF-195); proof receipts are keyed by
input digest (HF-190, HF-211).

Installed compilation must also separate product execution from development
apparatus (V1-01/V1-12). The current process, daemon, suite and developer-path
consumers import `tools` implementations. Compiler platform wheels bundle those
tools, which masks rather than removes the dependency. Migrate necessary kernel
queries, launch, resource enforcement, deadlines, registry writes and closure
under the existing product owners; move every installed caller together.
Process birth facts retain their clock domain and precision: an epoch lower
bound is not an exact boot-relative creation tick. Qualify complete installed
success and failure paths outside the checkout with forbidden development
imports observed even when caught. Explicit development commands can continue
to load their apparatus lazily. Development receipts, source audits and release
qualification do not belong in emitted programs.

Build qualification uses the same artifact, target, feature, profile and ABI
plans as execution. A library test harness and its target CRT are not an
ordinary executable with a synthesized entry point. Capability probes preserve
that distinction, including separate native host/build dependencies, and bind
their finite startup inputs to the selected toolchain. Reuse and invalidation
follow actual dependencies, as in
[rustc's incremental query graph](https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation-in-detail.html).
When changed inputs produce unchanged query results, retain qualified downstream
results whose declared dependencies remain valid. Bind reuse to the actual
source, tools, target and artifact identities, and bound retained query results,
backend artifacts, disk reads and peak live memory.
Group compatible owning checks into an existing build graph, retain their
input-bound evidence and rerun only invalidated claims. Additional layers,
parallel builds or duplicate receipts do not establish better qualification.

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
   source projection and name-based plans together. The 24 text-predicate
   calls now consume one explicit op-to-call mapping generated from the existing
   ABI manifest in native, LLVM and WASM. Their generated argument, machine ABI
   and result ownership checks share the canonical operand transaction and
   discarded-result sink; the replaced native dispatch family is deleted.
   This is the bounded declarative-definition mechanism described by
   [MLIR ODS](https://mlir.llvm.org/docs/DefiningDialects/Operations/), implemented
   through Molt's existing generator. The owning audit reads the actual family
   table and mapper; its closed source bridge remains until generated types
   enforce the complete dispatch invariant. Public execution fixtures must prove
   their specialized producers still occur, as well as compare outputs. A
   parameter annotation or a method spelling alone is no such proof. Frame-visible
   bindings and callback-invalidated class references keep their actual lifetime
   and lookup semantics; specialization requires the existing exact-value facts.
   The larger op-loop table contains heterogeneous guards, immediates, poll
   results and error protocols and requires separate complete classifications.
   This precursor introduces no second call registry and does not complete typed
   CFG or ownership transport.

   Measure the post-cache physical splitter on large bodies and post-module
   growth after source/name analyses retire. Retained batch timings without
   splits cannot prove it unnecessary. Delete it only when measured Cranelift
   costs and the existing aggregate growth policy qualify that retirement;
   otherwise move it under the typed owner after the last growth-producing
   module transform and before terminal ownership. Preserve its growth bound,
   continuation/frame transport, error and finalizer protocol until either
   outcome is qualified. Reuse the existing liveness solver
   with full SSA identity and raw values for chunk transport; ownership-root
   liveness is a different domain. Migrate the emitter's guard facts, reference
   roots and intra-operation finalizer invalidation with its typed inputs.
   Reusing optimized source and re-lifting its chunks would leave the same
   boundary in place. Reuse LLVM's direct typed
   custody and the existing analysis owners. WASM's operation-reachability
   boundary and the genuine source backends retain their complete migrations.
   Completing the generated enum vocabulary need not block typed transport:
   preserved registered operations retain their generated effects and identity
   until the complete schema migration retires their spelling.
3. **Measured runtime entry (C).** Profile one actual current-image consumer.
   Establish native TLS address-resolution costs before extending serial
   context transport. Co-locating destructor-free primitive fields under the
   existing execution owner is a candidate only if the actual heap-reference
   path benefits; scalar refcounts bypass entry, and lease/drain checks
   short-circuit. Preserve distinct GIL, attachment, isolate, shutdown-worker
   and TLS-destruction lifetimes through every accessor and callback.
   Migrate its complete admitted-service/context family while preserving
   callbacks, isolate changes, shutdown, error and ownership contracts. Choose
   direct calls and inline fast paths from typed link/capability/lifetime facts;
   do not assume a blanket boundary replacement removes every measured cost.
   Typed machine CFG enables inline lowering; refcount storage layout,
   overflow, terminal death and concurrency protocols remain necessary
   prerequisites for a correct inline fast path.
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

[The Rust rollup procedure](https://forge.rust-lang.org/release/rollups.html)
qualifies a combined integration candidate before default-branch publication and
distinguishes small compatible batches from performance- or platform-sensitive
changes. For Molt, one integration owner publishes each frozen candidate through
the existing coordination, proof DAG and landing driver. Preserve attribution
for measured changes and reuse qualified unchanged evidence as the existing
input and execution authorities permit.

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
