# 78. Whole-program type flow and specialization (the P0 program)

Status: design, 2026-11-14; slice 1 implemented; slice 2's seeding blocked on an unseen-caller
premise (section 5.3); slice 3's scalar contexts, clones, dispatch and merge implemented
(section 5.4, 2026-11-17). Owner decision 2026-11-14: this is the P0 compiler program.
Finding: HF-847. Supersedes the E5 section (2.3) of `03_E3-E5-ipo.md`; that design's E3
capture and purity summaries remain its own arc.

## 1. The finding

Molt compiles operations, not programs. Facts reset at every function boundary. A parameter
enters TIR as `DynBox` unless the source annotates it (`lower_from_simple.rs`), and an
annotation admits subclasses, so it proves no exact type. The body then runs the full dynamic
protocol on every operation: boxed words, reference counts, runtime arithmetic, comparison and
truthiness calls, exception checks.

The intraprocedural machinery already exists and works when it has facts: the `TirType` lattice
(`I64`, `F64`, `Class`, containers), `Repr::RawI64FullDeopt` (raw `i64` with checked
overflow), `type_refine`, SCCP, static basic block versioning, the inliner (E1) and the call
graph (S4). Nothing feeds them facts across a call.

Measured against native C references (instructions per iteration, release LLVM, macOS arm64,
2026-11-13; the C programs are the bar, CPython is the correctness oracle):

| benchmark | Molt | C | ratio |
|---|---:|---:|---:|
| `fib` (per call) | 498 | 14 | 36x |
| `class_hierarchy` | 5,145 | 20.4 | 252x |
| `struct` | 21,906 | 457 | 48x |
| `math_numeric` | 634 | 63 | 10x |

In `bench_struct`, compiled code holds 0.7% of self samples; the runtime holds the rest
(construction and dispatch 25.7%, teardown 18%, allocation and collection 7.5%, locks 6.7%).
The suite-wide technique table (section 9) assigns each benchmark's runtime share to the
technique that removes it.

## 2. What a world-class AOT compiler does here

Every whole-program compiler for a dynamic language that reaches native speed has the same
core: a whole-program abstract interpretation that infers the types reaching each function,
then compiles one body per distinct argument-type tuple.

- **Agesen, Cartesian Product Algorithm** (ECOOP 1995): analyze each function once per tuple
  of monomorphic argument types; a call with union arguments fans out over the product. Precise
  without call-string depth limits (k-CFA's `k` is an arbitrary constant).
- **Starkiller** (Salib 2004) and **Shed Skin** (Dufour 2006): CPA plus iterative flow
  analysis applied to Python; per-class abstract objects with field types.
- **RPython annotator** (PyPy; Ancona et al., DLS 2007): whole-program annotation from the
  entry point to a fixpoint over a finite lattice; one specialization per `argtype` tuple.
- **Codon** localizes type inference to a function body, delays instantiation until its argument
  types are known, and memoizes the realized instance. CIR transformations can request more
  instances through the same type checker. Its machine-sized integers and by-value tuples
  inform representation choices subject to Molt's overflow, identity and lifetime proofs.
  [Codon, sections 2–3 and Appendix A](https://doi.org/10.1145/3578360.3580275).
  Appendix A revisits unresolved expressions to a local fixed point, and calls
  demand a realization only once their operands are concrete. For Molt, retain
  the demand and memoization boundary, and propagate changes through the typed
  readers in §3.3. A call with unknown operands remains a valid generic Python call.
- **Julia** (Bezanson et al., SIAM Review 2017): inference per method instance (function plus
  concrete signature), cached; union splitting at call sites.
- **GraalVM Native Image** (Wimmer et al., OOPSLA 2019; saturation, PLDI 2024): closed-world
  type-flow graph from the entry points; reachability and devirtualization from it.
- **Lazy and static basic block versioning** (Chevalier-Boisvert and Feeley, ECOOP 2015;
  Melançon, Feeley and Serrano, ECOOP 2024): versions inside a body carry type facts past
  checks. Molt has static BBV; type flow gives it entry facts.

Two common shortcuts conflict with the rules in AGENTS.md and are rejected:

- Fixed budgets (specializations per function, union-split width, code-growth caps, Graal's
  saturation threshold). Each bound in this design follows from the program or from a stated
  precision requirement.
- Trusting annotations (mypyc). Molt's types come from allocation sites and operations, so a
  subclass or a wrong annotation cannot make a proof false.

## 3. Design

### 3.1 Where it runs

One pass, `type_flow`, in the TIR module phase (`module_phase.rs`), before the
module call graph and the inliner, over the whole user program. HF-773 forbids a second optimizer, so it is not
built in the Python mid-end. Its results are facts on typed SSA values (parameter types, call
targets, return types) that flow one way to codegen.

The module phase runs after per-function optimization, so the analysis reads optimized bodies.
A body whose entry or result facts change runs the re-entry pipeline, also used
for bodies the inliner changes. Today a call result is `DynBox`: `type_refine`
has no `Call` case, and `infer_return_type` stays inside one function. The flow supplies both
joins: a parameter type is the meet over its contexts' call sites, and a call result is the
callee context's return type.

### 3.2 Abstract values

The existing `TirType` lattice, extended where identity must be typed:

- Scalars: `I64` (exact Python `int`), `F64` (exact `float`), `Bool`, `None`, `Str`, `Bytes`.
- Instances: `Class(ClassId)`, an exact class (slice 1 replaced `UserClass(String)`):
  identity is typed, never spelled.
- Callables: a set of `FunctionId` (and bound-method pairs), so a function passed as a value
  (a callback, a `key=`, a decorator result) devirtualizes like a direct call.
- Containers with element types: `List(t)`, `Dict(k, v)`, `Tuple(ts)`, `Set(t)`, `Iterator(t)`.
- `Union` of the above, and `DynBox` (top).

A value gets an exact type only from a producer the program proves: a literal, an allocation
site (`C(...)` produces `Class(C)` when `C` is a proven binding), or an operation whose result
type CPython defines (`int + int` is `int`; `int / int` is `float`; `bool + int` is `int`).

For one immutable function body, the exact scalar solver supplies both the type map and the
scalar-forwarding view. Range analysis, scalar evolution and counted-loop recognition borrow
the same forwarding view. Carrier selection and late lowering consume the exact type proof
from that same solve. When lowering splits the body it takes an owned snapshot for the
original operations. A subsequent consumer of the rewritten body computes facts against its
new SSA definitions. Interprocedural solves with different parameter or call-result seeds
still evaluate their own context.

### 3.3 The analysis

A context is `(FunctionId, key)`, where `key` holds one monomorphic abstract value per
*relevant* parameter (3.4). The worklist starts at the program's entry contexts (module
bodies, and every function the program cannot see a caller of: exported, address-escaped to
unknown code, or reached from the runtime). The worklist:

1. Evaluates a context's body with its parameter types, using the existing transfer functions
   (`type_refine`'s result inference) extended at calls.
2. At a call whose callee set is known (a proven binding, a `FunctionId` set, or a resolved
   method), splits union arguments into the cartesian product of monomorphic tuples. It
   projects each tuple onto the callee's relevant parameters and records an edge to that
   context. The call's result type is the join of those contexts' return types.
3. At a call whose callee is unknown, passes `DynBox` and gets `DynBox`, and records why
   (3.8).
4. Re-queues a context when a parameter, field or return type it read grows.

Return types in a recursive cycle start at bottom (optimistic) and grow to the fixpoint:
`fib(int)` returns `n` (an `int`) on one path and `int + int` on the other, so it returns `int`.

Instance fields are field-sensitive per class: one abstract object per `ClassId`, and a field's
type is the join of every store the program makes to it. Module globals and closure cells are
abstract cells in the same way: the join of every store, or the one store's type for an InitOnce
cell. A generator's parameters flow into its frame; a value that `send()` or `throw()` delivers
enters as `DynBox` unless the program proves it. A proven callee binds positional and keyword
arguments at compile time and takes default types from the def-time default objects (the InitOnce
facts); `*args` and `**kwargs` keep the generic path. A class whose layout facts are proven
(design 68) turns a field read into a fixed-offset load of a known type.

Exact SSA inference uses executable reverse postorder. Numeric block IDs have
no effect on propagation along an acyclic path. Backedges converge through the
same lattice joins. A solve plan holds only body structure and initial bottom
facts. Type-flow contexts reuse that plan, including reachable return operands.
Each solve supplies its own argument and call-result facts.
[Clang's forward data-flow order](https://clang.llvm.org/doxygen/structclang_1_1ForwardDataflowWorklist.html)

### 3.4 Specialization key: only what changes the code

A parameter is relevant to a function when its type reaches an operation whose lowering
depends on it: arithmetic, comparison, truthiness, attribute access, subscript, iteration, or a
call that dispatches on it, including through a callee where the parameter is relevant. A
backward pass over the body (and the callees' relevance) computes the set. A parameter that is
only stored or passed along opaquely never splits a context.

Parameter relevance is a backward demand walk over scoped SSA values. Ordinary
uses seed the walk. The shared scalar-forwarding predicate, CFG block arguments,
and exact positional call edges identify predecessor values. A forwarding cycle
without an observing use contributes no demand. Exact parameters of any supported
arity participate by identity. The graph shares the resolved-call endpoints with
context analysis and visits each recorded dependency at most once. It uses one
edge arena and no per-round walks over the module.

Once the lattice settles, traverse final call targets from each generic context.
Generic entries remain roots for dynamic callers. Only reached specialized bodies
are copied and re-optimized. Transient contexts can contribute return information
during the solve, including in a recursive component, without producing code.

After optimization, contexts of one function whose bodies are identical merge into one clone
(compared by body digest). The number of clones is the number of reachable contexts that
compile differently.
The comparison serializes each origin once and uses the existing function table until
compaction. Report a merge by the representative's symbol in that same table.

Codon's generic function values can produce redundant realizations that an LLVM pass
later merges. Molt's TIR merge runs before backend lowering so it can save Cranelift,
LLVM and Wasm work as well as code bytes. The merge shares compiled code; each
observable Python function keeps its own identity, defaults, and closure. Track
analyzed contexts, distinct emitted bodies and linked bytes independently. Final
binary size alone cannot show whether specialization repeated unnecessary work.
[Codon, Appendix A](https://doi.org/10.1145/3578360.3580275).

### 3.5 Termination, with no constant

- The class set, the function set and the scalar families are finite in a closed world.
- Container types can nest without limit only through a cycle in the flow (`x = [x]`). At a
  cycle in the flow graph, the widening operator replaces any component whose type
  constructor changed since the last visit with `DynBox`. Each component can widen once, so
  every chain is finite.
- Hence the set of contexts is finite and the worklist terminates. CPA can be exponential in
  arity when many arguments are unions. The pass reports the context count per function in the
  build diagnostics. A program that grows it beyond its benefit gets a principled widening
  (merge contexts whose bodies optimize identically, which 3.4 already does), not a cap.

### 3.6 Representation selection and Python `int`

Python `int` has unbounded precision. A specialized body keeps `int` values as NaN-boxed words
of semantic type `int` at its boundary. An operation unboxes with one tag test (inline int47
versus heap BigInt) into `Repr::RawI64FullDeopt` and runs raw machine arithmetic with an
overflow check. The overflow edge goes to a static basic block version in which that value is
`MaybeBigInt` and the rest of the block uses the generic operations. Every step is an inline
compare and branch, not a runtime call. `float` maps to `FloatUnboxed`. An unboxed `i64`/`f64`
calling convention between specialized bodies follows when callers prove the representation
(D3, stage 6 carriers); it changes the ABI, not this analysis.

### 3.7 Dispatch

- A call site whose argument tuple the analysis proves calls its clone directly: one
  `call` instruction, no runtime lookup.
- A call site with a union argument splits on the value's tag or class id inline (Julia's union
  splitting) when every member has a clone; the split costs one compare per member.
- A call site with `DynBox` arguments calls the generic body, which is today's code.
- The generic body survives only where some site reaches it: a `DynBox` caller, or a function
  object that escapes to code the analysis cannot see. Otherwise whole-program DCE removes it.

### 3.8 Methods, classes and the closed world

`obj.m(args)` with `obj: Class(C)` resolves `m` through `C`'s MRO at compile time when the
program holds no writer to `m` on `C` or on any class in that MRO. This is the same "possible
writer" rule as module globals (call_anatomy 4.5a, the `code_rebinding` premise). The
dynamic-execution contract excludes `__bases__` reassignment, and class attribute stores are
writes the analysis sees. A resolved method is a static call edge, so it is inlined and
specialized like a function. With a writer present, the site keeps its inline cache (stage 4),
and the diagnostic names the writer.

### 3.9 The standard library, compiled once

The stdlib is compiled once into a cached partition and must not be recompiled per build.

- The runtime ABI declares semantic result facts for typed intrinsic calls.
  The shared exact scalar solver accepts a successful integer result when the
  call's exception check dominates every use. The integer may need heap
  storage, so the representation planner still checks the required width.
- A Python-source stdlib function participates in the flow through its cached TIR (read-only).
  A context the user program needs is cloned into the user unit and cached by
  content address: the stdlib body digest plus the key. A key compiles once across builds and
  programs. Generic stdlib bodies stay in the shared partition (`non_inlinable` holds).

### 3.10 Incremental rebuilds

A context's result depends on its body, its key, and the return and field types it reads. Per
context, the pass stores those inputs' digests and its outputs, content-addressed. An edit
invalidates the edited function's contexts. A dependent re-runs only when a type it read
changed (early cutoff, as Salsa's red-green and Zig's analysis units do). The no-change rebuild
bar (Cargo, Go, uv) holds; the pass's cold cost is measured on the largest corpus program and
on the stdlib.

The module phase currently reads bodies that have already passed per-function
optimization (§3.1). The re-entry pipeline skips first-entry passes and
repeats other work on changed bodies. Use the existing pass timings to identify
expensive overlap and check which normalized operations the solver reads. This
shows which passes can move after fact propagation to run once on cold builds.
Per-context reuse addresses unchanged work on edits.

Record the dependencies of the type summary separately from those of the optimized body. A
caller that only uses a callee's return type or effects depends on those facts. A caller that
inlines the callee also depends on its body. An edit that leaves the first set of facts equal
keeps the caller's analysis valid. A body edit can still invalidate an inlining caller's code.
Use the same typed dependency identities for source and stdlib contexts.

The solver seeds generic contexts only for bodies that can change a resolved
call or supply its return fact. It marks both ends of each resolved edge and
preserves their document order. Generic code for other bodies is still emitted
under the same reachability rules and can run through unseen callers.
Demanding specialized bodies from externally observable roots still requires
the callable escape and reachability proofs. Reuse prepared TIR and target
artifacts at their existing boundaries.

Measure cold, single-function edit and unchanged latency separately from generated-code speed.
Codon's realization cache demonstrates reuse within one compilation and its benchmark appendix
reports program execution. Its roadmap lists incremental module caching separately.
In Appendix B, the reported `nbody` times on Apple M1 Max are 0.15 s for Codon and
C++ and 4.29 s for CPython. The `sum` computation disappeared during optimization
in Codon and C++. For a per-iteration comparison, use an input that keeps the
measured loop live and check observable outputs and edge cases against the oracle.
[Codon, Appendix B](https://doi.org/10.1145/3578360.3580275).
[Codon roadmap](https://docs.exaloop.io/developers/roadmap/).

### 3.11 Diagnostics

A call site that stays generic says why in a build diagnostic: unknown callee, a `DynBox`
argument and the producer it came from, a writer that keeps a method dynamic, or a function
object that escapes. `tools/opt_explorer.py` surfaces it beside the profile. A lost fact never
falls to the runtime in silence.

## 4. Semantics obligations (each needs an independent CPython oracle test)

- Exactness: a subclass instance (`class MyInt(int)`) never reaches an `int` context; types come
  from producers, not annotations.
- `bool` is a subclass of `int`: `True + 1 == 2`, `type(True + 1) is int`; `bool` keeps its own
  type in contexts.
- Raw `i64` floor division and modulo round toward negative infinity, as Python does; overflow
  of `+ - *`, of `-(-2**63)` and of `abs` reaches BigInt.
- Mixed `int`/`float` arithmetic and true division produce `float`.
- `RecursionError` still occurs at CPython's depth.
- A class attribute store, `setattr`, or a store through `__dict__` anywhere in the program keeps
  the affected methods dynamic; differential tests write the method after the first call.
- A function object that escapes (stored in a container, passed to unknown code) keeps a correct
  generic body.
- A clone is another compiled entry of the same Python function, not a new function object:
  `f is g`, `f.__code__`, `__qualname__`, line numbers, tracebacks and frame records equal the
  original's.

## 5. Slices

Each slice lands the same day with its measured before and after, deletes what it replaces,
and covers Cranelift, LLVM and WASM through the shared lowering.

1. **Typed identity.** `FunctionId` on TIR call edges and `ClassId` in the type lattice. Delete
   `s_value Str(name)` callee matching in `call_graph.rs` and `UserClass(String)`. Names stay
   for symbols, reflection and messages.
2. **Monomorphic flow.** The analysis of 3.3 with no cloning. Where every context of a function
   agrees, it seeds the parameter and return types into the existing pipeline. This covers
   functions called with one type (`fib`, most numeric kernels). It adds the 3.11 diagnostics.
3. **Specialization.** Relevance (3.4), cloning per context, merge by body digest, direct
   dispatch and union splitting (3.7), and DCE of unreached generics.
4. **Objects.** Field-sensitive class objects, closed-world method resolution (3.8), and
   callable sets for callbacks. The InitOnce lane's closed-world dispatch work feeds this slice.
5. **Stdlib participation** (3.9) with content-addressed clones.
6. **Unboxed calling convention** between specialized bodies (3.6, D3).

### 5.1 Slice 1: typed identity

Design review: 2026-11-14, approved with conditions C1 to C5 below.

**Decisions.**

- **D1. The document's function vector is the function table.** A `FunctionId(u32)`
  (`molt_ir::function_table`) indexes `SimpleIR::functions`, bodies and extern
  declarations alike. No second registry repeats its rows; `FunctionTable` is a
  structure-of-arrays projection (symbol, row kind) for consumers that do not hold the
  vector, such as `TirModule::table` and a backend's call lowering.
- **D2. Decode resolves each wire symbol once.** The Python wire keeps `s_value` symbols.
  `resolve_function_references` moves the symbol of each op the generated
  `simpleir_function_reference_roles` table names into the typed `callee`. A `required`
  reference (`call_internal`, `call_guarded`, `func_new`, `func_new_closure`) that names no
  row is a decode error: a caller that spells a callee instead of reading
  the definer's symbol (`compiler_analysis/source_function_symbols.py`) fails at once. An
  `optional` reference (`call`, `call_async`, `alloc_task`, `stateful_locals_register`)
  that names no row keeps its runtime or external symbol for TIR ingest (HF-821 B1); a
  runtime poll function (`molt_async_sleep_poll`) is such a target.
- **D3. TIR carries the id as a typed field.** `TirOp::callee` beside `runtime`;
  `CallSiteTarget::Direct`, `CallEdge::StaticDirect`, `CallTargetFact::StaticDirect`, the
  call graph, module summaries, the inliner, generator fusion, guarded targets and the
  linkage signatures index by `FunctionId`, with vectors and `FunctionSet` bitsets.
- **D4. Backends read a callee's symbol only from the table.** LLVM
  (`LlvmBackend::function_table`), Cranelift (the final document's table replaces the
  name set `defined_functions`; a callee is local when its row has a body) and WASM
  (`WasmCallableCallSiteAbi::target_symbol`) take the symbol of a typed callee from its
  row, and an optional reference's runtime symbol from `s_value`
  (`OpIR::reference_symbol`). Their declaration maps (Cranelift arities, WASM function
  indices) stay keyed by linker symbol: `cranelift_module` and the WASM linker key by
  symbol, and no callee reaches them by spelling.
- **D5. Caches are position-independent.** The FunctionIR digest writes each callee as its
  row's symbol; a cached TIR body stores its callees as relocations into its own symbol
  list (`MOLT:TIRFUNC:v6`) and restore binds them to the current document's ids.
- **D6. Exact classes.** `TirType::Class(ClassId)` replaces `UserClass(String)`. A
  `ClassId` (`molt_ir::class_identity`) is the stable 64-bit hash of the class
  statement's injective `definition_symbol` (HF-220), the rustc `DefPathHash` pattern: it
  compares as an integer, needs no table, and is the same in every document, so cached
  TIR types need no relocation. Decode rejects two statements of one program whose ids
  collide. `object_new_bound` joins the class-definition carriers and the frontend writes
  its statement there; the frontend no longer writes user-class names into `type_hint`,
  and `from_type_hint` produces no class. The review draft had a dense `ClassTable` with
  cache relocation; nothing in this slice indexes a vector by class, so the hash removes
  the table and the relocation. A later slice that needs dense class vectors builds a
  per-program index over these ids.
- **D7. One generated reference-role table** replaces the s_value retention list.
  Dead-function elimination retains exactly the ids ops carry. `call_method` (a
  `BoundMethod:` spelling), `class_def` (a layout tuple) and `code_new` (no symbol) are not
  references.

**Invariants and their checks.**

| invariant | check |
|---|---|
| I1 one authority: the function vector is the table | `FunctionTable::of` is a projection; no registry stores rows |
| I2 no callee name after decode | IR schema and TIR verifier reject a callee on a non-reference op; the both-carriers rejection returns when the transitional symbol retires |
| I3 ids are valid in their document | program contract (`function_row`) and verifier |
| I4 rows are stable within a document; a transformation that removes, reorders or partitions rows rewrites ids through one helper | `retain_functions`, `retain_rows_with`, `remap_function_ids`, `project_document` |
| I5 caches never hold a raw id | relocation unit test (`callees_relocate_by_symbol_across_documents`) |
| I6 `Class(id)` is exact | produced only by `object_new_bound` (C1): the SSA lift and the generated `object_class_definition` refine rule |
| I7 a backend reads a callee's symbol only from the table | `OpIR::reference_symbol`, `LlvmBackend::function_table`, `WasmCallableCallSiteAbi::target_symbol`; decode clears the wire symbol, so a stale read finds nothing |
| I8 complete migration | rustc: the deleted helpers no longer exist |

**Conditions.**

- **C1.** `Class(id)` comes only from `object_new_bound`. The result of `C(...)` is not
  `Class(id)` in this slice (`__new__` may return another object; a metaclass may override
  `__call__`). A differential case has `__new__` return an instance of another class.
- **C2.** A row a batch appends is visible only in its own batch document: every document
  split (`append_referenced_external_declarations`, `partition_user_owned_functions`,
  stdlib externalization) builds a self-contained document through `project_document`,
  so no id crosses documents. Megafunction split appends chunk rows and leaves every
  existing id in place.
- **C3.** Acceptance: the full `tests/differential` corpus on native, LLVM and WASM; a full
  stdlib build; cold compile and no-change rebuild time for `bench_fib` and `bench_struct`
  (the Cranelift no-change rebuild stays about 0.4 s); `bench_fib` and
  `bench_class_hierarchy` instructions retired unchanged.
- **C4.** Lands after `foundation/module-function-symbols` (the definer's symbol
  authority D2 depends on).
- **C5.** This subsection is the slice's design record.

**Status (2026-11-14).** Implemented: D1 to D7, I1 to I8 and C1, C2, C4, C5. Decode
clears the wire symbol of every resolved reference; the IR schema and the TIR verifier
reject an op that carries both a callee and `s_value`. The differential case
`tests/differential/basic/class_identity_typed.py` covers same-named classes and functions
across modules and a `__new__` that returns an instance of another class.

**Rejected alternatives (slice 1).** The frontend emits ids: Python integration steps add,
rename and drop functions by symbol after emission, so each would need its own remap; a
later slice gives the frontend a typed function table and deletes the decode resolution.
Ids assigned at TIR lift with names kept in SimpleIR: Cranelift and WASM consume
back-converted SimpleIR and would keep matching names. A per-function callee table
(Cranelift `FuncRef`): every module-level analysis would translate local ids. Hashed
function ids: the call graph, summaries and inliner index vectors by `FunctionId`, which
needs dense ids. A qualified `UserClass` name: still spelled identity, and still exact for
annotations.

### 5.2 Exact receivers: status and blockers (2026-11-15)

Slice 4 needs exact receivers before any TIR fact can cross a method call. The frontend's
constructor proof is their only producer, and it refuses a class whose statement the source
demands mark opaque (`definition_exposed`).

- **Landed (5125aeef58).** `@property` and operators on instances no longer expose a class
  (call_anatomy.md 4.5, "Closed-world properties"); an exact receiver reads a property as the
  field load or a direct getter call. `bench_descriptor_property` -98.6% instructions on LLVM.
- **Method calls expose their receiver.** The binding flow treats `obj.m(args)` as a call of an
  unknown callable and escapes `obj`, so `Leaf()` (`bench_class_hierarchy`) and `Obj()`
  (`bench_method_defaults`) stay opaque. The fix binds the receiver to the method's `self`: the
  binding flow skips the escape for a fresh instance of a plain class whose member is a plain
  user function, and the source demands bind the instance to the method's first parameter where
  an attribute value selects it. Two facts block a sound version. The class must be reachable
  only through its own module, and discovery scans the entry module as an imported module, so no
  scan knows it is a private root. And discovery's first scans run before the graph's premise,
  while escapes are monotone, so the decision must be optimistic and rescan when a premise names
  a writer: a reflection of the module, a computed attribute store, or a store of the method's
  name.
- **TIR specialization by trial inlining** (splice a refused callee into a copy of the caller,
  re-optimize, keep it only when no callback site is added) has no consumer yet: a module
  function call is `call_guarded`, an opaque call role, until guarded call promotion (HF-773
  slice D) makes proven calls `call_internal`, and a method call is a method site until its
  receiver is exact. The same holds for rewriting `GuardedFieldGet/Set` to fixed-offset sites on
  a `Class(id)` receiver, which also needs `object_new_bound` to carry the
  `instance_class_writer` fact, so that TIR types an allocation `Class(id)` only when no code can
  assign `__class__`.
- **`bench_struct`'s allocation** is not scalar-replaced: SROA admits only nothrow, callback-free
  allocations, and `object_new_bound` is neither (it may seal class metadata), but the first
  blocker is the frame home store of the local (`FrameHomeStore`), which observes the object.
  Stage 1 of call_anatomy.md (no record for a leaf no observer can reach) owns that.

### 5.3 Slice 2: status and blocker (2026-11-17)

**State.** The `type_flow` module pass is not built. Its parameter seeding has no sound producer
for a published def, and the measured upside of seeding with today's consumers is small, so the
lane landed the two changes the slice's measurements called for instead.

- **The blocker: every published def has an unseen caller.** A module-level `def` stores its
  object in its module's namespace. A cell-eligible module stays eligible under computed-name
  reads (`getattr(m, name)`, star import, pickle's `find_class`; call_anatomy.md 4.5a), and
  `sys.modules` holds every module, so code the analysis cannot follow can read the object and
  call it with any argument (`tests/differential/basic/type_flow_escape.py`). Section 3.3 makes
  such a function an entry context with `DynBox` parameters. Seeding becomes sound in one of two
  ways: a whole-program reader fact (no computed-name read reaches the module object), which the
  binding flow does not compute today; or slice 3, where a proven call site enters a clone and the
  original body stays generic for every other caller. Slice 3 needs no reader fact, so it is the
  next step; an entry check that versions the body (the retired block versioning) is the same
  2x-code clone with a check added, and is rejected.
- **Measured upside of parameter facts, as an upper bound.** An unsound probe that trusted
  annotated `int` parameters as exact (LLVM release, macOS arm64, never landed):
  `bench_loop_invariant_shift` 10.39 G -> 9.42 G instructions retired (-9%). The shift itself
  stays a runtime call: `Shl` of exact ints has no inline late-TIR lowering, so the
  `lshift_impl` share (49%, section 9) needs that lowering, not type facts. An exact `int` still
  needs the inline-versus-BigInt tag test at each use, so `bench_fib`'s body gains nothing from
  an exact parameter. `bench_tuple_index`'s `index_impl` share left with 35ea8cfa81 (exact tuple
  reads inline).
- **Landed: annotated defs keep proven calls (adc6792ae5; the HF-888 lane owns the rule).** An
  attribute store into a def's new function object no longer counts as a start of the function
  in init dominance, so an annotated recursive function enters itself directly: `bench_fib` 1.274 G -> 0.633 G instructions on LLVM
  (-50%), 2.214 G -> 1.548 G on Cranelift (-30%). Annotated and plain programs now compile to the
  same calls.
- **Landed: HF-890.** Nothing produced the TIR `TypeGuard`, so static block versioning and the
  type-guard hoist never fired, while both backends lowered it as a copy and `type_refine`
  trusted its type. The op, both passes and every consumer are deleted (HF-573 keeps the IV
  strength-reduction half). The type flow seeds proofs, never guards.
- **Kept for slice 3:** the section 4 differential programs `type_flow_int_semantics.py`,
  `type_flow_recursion.py`, `type_flow_escape.py` and `type_flow_identity.py`, which pass on
  native and LLVM against CPython today and guard the clone obligations.

### 5.4 Slice 3: as built (2026-11-17)

**What runs.** `type_flow` (`runtime/molt-passes/src/tir/passes/type_flow/`) is the first
step of the TIR module phase, before the call graph, the inliner and generator fusion, so
every later analysis sees a clone as an ordinary row.

- **Resolved sites.** A proven `call_guarded` (`proven_target`, HF-846 slice 2) and a
  `call_internal` whose argument count equals the callee's parameters. Every other call is
  an unknown callee; the report counts them per function.
- **Contexts (3.3).** A key is one `ArgKind` per parameter: `Int`, `Bool`, `Float`, `Str`,
  `None`, `Class(ClassId)`, or `Dyn`. Every body has its generic context (all `Dyn`), which
  every unseen caller enters. Evaluating a context solves the body's exact scalar and class facts
  (`type_refine::solve_exact_scalars`) seeded with the key and with the return types of the
  contexts its resolved sites enter. Return types start at `Never` and rise; the int family
  joins to the semantic `int` (`I64`), any other disagreement to `DynBox`. A site's result
  is the join of every context it has entered, so it never falls when its key widens, and
  the worklist terminates: keys have a finite domain and every fact rises in a finite
  lattice (3.5, no constant).
- **Relevance (3.4).** A parameter splits a context only when its value, through forwarding
  copies and block arguments, reaches an operation other than a reference-count operation,
  a branch condition, a switch value or a return, or an argument of a resolved call whose
  callee parameter is relevant. The module solves the callee dependence from "nothing
  relevant", so a parameter only passed around a recursive cycle stays irrelevant.
- **Clones.** One per reached context with a non-generic key, named
  `<origin>__tf_<letters>` (one letter per parameter: `i b f s n d`). The clone is a copy of
  the origin's optimized body whose new typed field `TirFunction::exact_param_types` holds
  the key. The exact solver and `type_refine` seed the entry arguments from it; nothing
  else produces it, and an annotation never does. It is serialized (`MOLT:TIRFUNC:v7`).
- **Dispatch (3.7).** On the bodies the analysis evaluated, each site that selects a clone
  context gets the clone as its typed callee, and each site gets its target context's exact
  return type as `exact_result_proof`. A proven guarded call
  keeps its callee word, which the late stage stores as the invocation handoff, so the
  clone's frame record holds the original function and code object: `f.__code__`,
  `__qualname__`, line numbers and tracebacks are the original's (section 4). Clones and
  every caller whose sites changed re-run `refine -> pipeline -> refine`.
- **Fixed fields (5.8).** Use the class definition's admitted slot table with exact receiver
  facts from allocations, call results and specialized parameters. Lower those accesses before
  clone comparison so a clone whose only advantage is fixed offsets survives deduplication.
- **Merge (3.4).** A clone's digest is its canonical serialization under a fixed name with
  seeds, counters and parameter types normalized, plus the exact family of every
  non-parameter value. Clones of one origin with equal digests merge, and the survivor's
  seeds become the meet of theirs; a clone equal to its origin folds into it. Merged rows
  are removed and callee ids renumbered once.
- **Rows.** Clones are rows after the document's rows. `ModulePhaseOutcome::clones` and
  `TirOwnedModule::clones` name each clone's origin. The module phase extends the linkage
  signatures it hands the drop phase; the LLVM driver copies the origin's signature and
  entry custody to the clone's symbol; the native driver and the fact graph append the
  origin's SimpleIR row under the clone's symbol (`TirOwnedModule::append_clone_rows`).
- **Target gate.** A target whose backend compares a guarded call's code pointer with its
  target (`FinalLowering::BackendLanes`: Cranelift) would send a clone's every call to the
  runtime fallback, so it gets no clones; its proven calls still get their callee's return
  type. Cranelift gains clones when it consumes the late stage (HF-821, the codegen-walker
  lane).
- **Diagnostics (3.11).** With `MOLT_INLINE_STATS=1` the module phase writes, to stderr and
  to the debug artifact `tir/type_flow.txt`, one line per module (contexts, clones, sites
  specialized, results typed), one per body with resolved sites (contexts, clones, resolved
  and unknown-callee calls), and one per resolved site that stays generic with its reason:
  the callee's refusal (generator, execution context, partition, linked elsewhere,
  target gate), "no parameter's type changes the callee's code", or the first relevant
  argument that is not an exact scalar and its producer.

- **Re-optimization.** Changed bodies run `PipelineEntry::Reoptimize`: the default pipeline
  without async-work poll placement. Placement runs once, on a body's first entry; its marks
  persist (`check_exception_elim` keeps them), and it reads loop shape that the first run's
  later passes (`checked_int_loops`) rewrite, so a second placement panicked on a versioned
  loop's latch.

**Measured** (instructions retired, release, macOS arm64, `/usr/bin/time -l`, interleaved, best
of 3, outputs identical; base 2a19257808):

| program | LLVM before | LLVM after | Cranelift |
|---|---:|---:|---|
| `bench_fib` | 630.6 M | 586.8 M (-6.9%) | +0.1% |
| `bench_loop_invariant_shift` | 10.39 G | 9.43 G (-9.2%) | 0.0% |
| `bench_math_numeric` | 838.1 M | 837.7 M (-0.1%) | 0.0% |
| `bench_class_hierarchy` | 16.11 G | 16.10 G (-0.1%) | 0.0% |
| `bench_method_defaults` | 5.004 G | 5.004 G (0.0%) | +0.1% |
| idiom `recursion` | 71.6 M | 70.8 M (-1.0%) | 0.0% |
| idioms `call_two_args`, `closure`, `int_arith`, `method_call` | | within +-0.3% | within +-0.2% |

The fib and shift gains come with the clones' exact parameters and typed call results. That
the late stage then drops float arms and exception checks of exact-int arithmetic is
inferred from the mechanism, not yet profiled. The flat rows
need what this slice does not yet feed: inlining of clones (a proven guarded call is an opaque
edge), the inline exact-int shift, and object keys (slice 4). Cranelift is flat by the target
gate.

**Not built in slice 3 (remaining).**
- Union splitting at a site (3.7): the exact lattice has no unions (`scalar_or_bottom`).
- Whole-program DCE of generic bodies (3.7): a module-level `def` keeps its function object,
  which a computed-name read may return, so its body stays; downstream dead-function
  elimination still removes unreferenced rows.
- Inlining a clone: built in 5.5.
- Container, class and callable keys (slice 4) and stdlib clones (slice 5).

### 5.5 Inlining clones: as built (2026-11-17)

**Static edges (HF-773 slice D).** `call_targets::static_call` is the one authority for a call
whose callee and argument binding the program proves: a direct call (its operands are its
arguments) or a proven `call_guarded` (operand 0 is the callee's function object, the rest
bind its parameters). The shared call-site projection, and so the call graph, call facts and
the inliner, read both as the same `StaticDirect` edge. A caller whose only calls are proven
is no longer "recursion-capable by an opaque call", and the inliner splices a proven call's
arguments and drops its callee word.

**Frame elision (call_anatomy.md 4.3, "Which functions need no record at all").** Every Python
`def` links a frame record, and the inliner refuses a callee with an execution context, so no
user function was inlined. `passes/frame_elision` deletes the record of a body no observer can
reach: on every block normal control flow reaches, each operation is nothrow and calls no
Python code (exact-scalar effects), no operation reads the frame, no check is an async-work
poll, no branch tests truth through Python, there is no handler region or suspension, and every
home holds an exact scalar (so no finalizer moves). It deletes the record's link and pop, the
locations and every `CheckException`, turns home stores into value copies, and sets
`ExecutionContextPolicy::None`. It runs after the type flow, on every body, for targets that
consume final TIR; a nothrow clone (`add__tf_ii`) is then an ordinary leaf the inliner splices
by its existing cost rules. Cranelift (`BackendLanes`) keeps every record: its drivers keep
each row's SimpleIR execution context.

**Facts cross the splice.** A clone's `exact_param_types` are what the flow proved of its
call site's arguments, which the caller's own solver cannot derive again (they rest on callee
return types). The splice binds each proven parameter through a binding op that carries the
type as `exact_result_proof`, so the inlined body keeps the facts it was compiled under.

**Recursion depth.** An inlined leaf no longer charges the recursion depth, as the native
backend already omits the charge for a callee that makes no call (`CallGraph::leaf_functions`).
A program sees the difference only when a leaf call is the one that would cross the limit;
RecursionError still occurs in every recursive cycle (section 4).

**Re-entry.** Every module transform (type flow, frame elision, inliner, generator fusion,
slot promotion) re-optimizes a changed body through `passes::reoptimize`: refine, the
`PipelineEntry::Reoptimize` pipeline, refine. Async-work polls are placed once, on first entry.

**Measured** (2026-11-17, release, macOS arm64, instructions per loop iteration as the slope
between n = 200,000 and 4,200,000, best of 3; main c5b996c08e against the lane; C is clang -O2,
CPython is the managed 3.12.16 with `-I`):

| idiom | LLVM before | LLVM after | C | CPython | Cranelift |
|---|---:|---:|---:|---:|---|
| `call_two_args` | 235.0 | 59.7 (-74.6%) | ~0 | 864 | 591, flat |
| `method_call`, `construct`, `instance_attr`, `closure`, `recursion` (per call) | | within +-0.1% | | | flat |

`call_two_args` inlines its two `add__tf_ii` sites; it stays above 2x C, so HF-773 stays open.
The other rows enter no frameless clone: their callees can raise (field access, a closure
call, `fib`'s recursion), which needs inline frames (HF-824).

**Still open.** Generic-body DCE (3.7): a published def keeps its function object, which a
computed-name read may return (5.3), so the body stays until the import closed-world lane
proves no such reader. Union splitting (3.7): the exact lattice has no unions. Cranelift
clones and frame elision wait for Cranelift on the late stage (HF-821).

### 5.6 Exact receivers: method calls bind `self` (2026-11-17)

**The cost.** A sampled profile of the `method_call` idiom (LLVM release) put about 80% of
self samples in the runtime's guarded field calls (`guard_layout_match`,
`instance_dict_bits_ptr`, shape ids, `field_storage::resolve`): inside a method, `self` was
only *declared* its class, so every `self.x` read and store was `guarded_field_get/set`, a
runtime call. Compiled code held about 16% of the samples.

**Receivers stay bound (5.2's blocker).** The binding flow no longer escapes the receiver of
`obj.m(args)`: the call's receiver origins carry a `method` demand, and the source demands bind
the receiver to the first parameter of each function the selection reaches
(`_select_attribute`). The receiver escapes only where that binding is incomplete: a class whose
MRO is not all plain class statements, a member that is not a plain function, `classmethod`,
`staticmethod` or `property`, a dunder no class defines, an instance store that shadows a class
member, `__dict__`, or `super()` above a class with unknown bases. `type(x)`, `vars(x)` and
`x.__class__` expose the class to stores through them. Zero-argument `super().m()` binds the
enclosing function's first argument to the member above its class. So `Counter()` in
`kernel` constructs inline and its result is exact.

**Closed calls across modules.** A qualified positional call records possible argument exposure
at its own source site. Once discovery settles, the existing argument-to-formal transfer
discharges that exposure if the caller's import and the provider's function have exhaustive
source publications. Missing publication proof or a reachable writer releases the generic
exposure. The proof also rechecks held sites when discovery admits more code.

The default `isinstance` protocol can inspect the subject's `__class__`. Its class operand
stays private when the bound declaration used the builtin class builder, has metaclass
`type`, and has no class decorators. The compiler retains the subject's lookup and retirement
edges. In `bench_matrix_math`, these two source proofs preserve the class through all twelve
qualified `molt_buffer.get` and `set` calls and recover exact receivers for both buffer methods.

An init-dominated module cell read can identify the actual class object it
loads. Initialization proves that the unique store publishes the object of a
stamped class statement that runs once. Shared TIR folds `isinstance` to true
when this class object matches the subject's exact, lifetime-stable class.
Python handles this exact match before subtype and metaclass checks. Other
queries keep the runtime protocol.

**Exact `self`.** `PythonSourceDemands.exact_receivers(module)` lists each function whose every
entry binds its first parameter to an instance one class statement of the module built: every
`_bind_receiver` receiver came from that one statement, no call entered it without a receiver,
no attribute selected it through anything but an instance, and no unseen code activates it. The
fact joins each module's namespace premise once discovery closes the graph
(`PythonNamespacePremise.exact_receivers`). No scan reads it, so it never revises a scan or a
scan's cache key (`scan_view`, which also drops the other lowering-only facts and, by
HF-926, the foreign names the source cannot spell); the lowering cache key carries it. A first version refreshed
the premise on every change, and the rescans made a stdlib discovery test run past its 10-minute
limit. The frontend compiles such a method's
first parameter as exactly its class (`_closed_world_receiver`) when the method is a plain
function and not a special method (the runtime enters a special method with no selection the
graph records), the body never assigns or deletes the parameter, no closure captures it, and
the class layout is stable. Its field reads and stores are then the shared `field_slot` code
with no layout guard.

**Measured** (same method as 5.5; main 57f173a162 against the lane):

| idiom | LLVM before | LLVM after | C | CPython | Cranelift before | after |
|---|---:|---:|---:|---:|---:|---:|
| `method_call` | 1,926.7 | 403.2 (-79.1%) | 1.1 | 1,100 | 2,175.7 | 652.4 (-70.0%) |
| `construct`, `instance_attr`, `closure`, `call_two_args`, `recursion` | | within +-2% | | | | flat |

`method_call` is now 2.7x faster than CPython and still 370x C, so HF-847 stays open. What is
left per iteration: the method site's guard and indirect call, `bump`'s frame record, the
missing-field test in each field access, boxing of `count`, and the call's reference counts.
The next steps remove whole classes of that cost: a direct call for a method resolved at
compile time (3.8), a field type from the join of its stores so `count + k` is an exact-int
add, and inlining `bump` (it can raise only on a missing field, so it needs either a
definite-initialization fact or inline frames, HF-824), then scalar replacement of `c`.

### 5.7 Proven method calls (2026-11-17)

**What runs.** `obj.m(args)` whose receiver holds an exact class fact, durable or not, is a
proven call when the class is namespace-closed, `m` resolves through the MRO to a plain
function with no closure, defaults, `*args`, `**kwargs` or keyword-only parameters, the class
defines no `__getattribute__` or `__getattr__`, the call is positional with the exact arity, no
code object is reachable (`code_rebinding`), and the target's entry adopts its arguments as the
source call does. A durable fact (one that survived a callback) suffices: the source demands
expose a class whose instance gets an attribute that shadows a member, or whose instance
mapping some code reaches (5.6), so no callback can have shadowed `m`. The frontend emits the
method load and a `call_guarded` of the method's symbol with `proven_target`, the shape
integration gives a proven module call (HF-846 slice 2). So the call is a static edge for the
call graph, the type flow and the inliner, and the late stage enters the target (or its
clone) directly, handing it the loaded function for its frame record.

Method selection reads the fact on the evaluated receiver value. A fresh
allocation used immediately in `C().m(args)` has the same class proof as a
local holding that allocation. The method load captures its function before
arguments execute. A factory or a custom `__new__` whose result has no exact
value proof keeps ordinary lookup.

**The load.** The callee's frame record needs the function object (`generate_frame_push`
validates the handoff). A first version called the method's symbol with no handoff: every call
took the runtime's frame entry (`molt_frame_record_enter`), and `method_call` rose from 404 to
1,375 instructions per iteration. The method load supplies the object. A `load_method` with
`proven_target` runs no code and raises nothing a program can observe, so
`op_instance_effects_for_op` makes it nothrow and effect-free: its exception check goes, and a
load nothing reads (its call was inlined) is dead. The load still runs the method site's
guard. The end state is a static member cell: the function object of a member no code
rebinds, stored once when the class statement runs and read by one load, as module functions
read their `InitOnce` cells; that removes the guard and the site record from proven sites
(call_anatomy.md 4.5a).

**Measured; landed 2026-11-19.** Instructions per iteration (slope of two sizes, best of 3,
release, macOS arm64), main e75c63a77f against this change: `method_call` LLVM 383.0 -> 379.3
(-1.0%), Cranelift 523.2 -> 507.3 (-3.0%); output identical. On 2026-11-17 Cranelift rose
653.5 -> 752.5 (+15.2%) because it lowered `call_guarded` in its own backend and ignored
`proven_target`; since the W3 switch (5144080ca6) both backends enter a proven call through the
shared late stage, so the admission call and the code compare are gone on both. The gain is small
because the call itself stays: C pays 1.1 instructions per iteration, so `method_call` is still
about 345x C. What remains is the static member cell (the load below) and inlining the target.

**Inlined methods were slower (not landed).** A frame-elision rule that admits one object
parameter home (a method's `self`; CPython clears a returning frame's locals with the caller
already current, so releasing the parameter at its last use is indistinguishable) made a
nothrow method frameless and the inliner spliced it. The probe `total = (total + s.twice(i)) &
16777215` went from 386.6 to 444.3 instructions per iteration (main 400.9): the spliced body
and its binding aliases cost more in the caller's versioned loop than the direct call. The
rule waits until that loop cost is understood; the patch is kept with the lane's notes.

### 5.8 The measured gate (2026-11-20)

**Coverage.** Across the 22 benchmarks slower than CPython (release LLVM, macOS arm64,
`MOLT_INLINE_STATS=1`), the type flow resolves 28 call sites and leaves 4,598 with an unknown
callee; it makes 9 clones, all in `matrix_math` and `object_class_lifecycle`. Most unknown
sites are not calls but value releases that may run Python (`frame_home_store`, `trace_exit`,
`store`, `tuple_new`); the Python calls left are polymorphic method sites (`class_hierarchy`),
calls through the generic binder (`call_bind`) and calls of function objects.

**Time.** The in-process sampler over the same 22 (symbol-keeping builds named with `atos`;
each benchmark weighted equally) puts 12.9% of samples in compiled code. Generic call entries
take `call_bind_ic_owned` 52% of `object_class_lifecycle`, `call_loaded_method1` plus
`load_super_method` 62% of `class_hierarchy`, and `call_func_owned` 35% of `matrix_math`, 34%
of `heap_canonical_cache` and 28% of `weakref_native`. Object lifecycle (reference counts,
instance allocation, allocator page work) takes 50% of `struct` and up to 79% of
`builtin_locals_slice`; by-name attribute access 15% of `heap_canonical_cache`.

**Exact classes in TIR.** An allocation's `class_definition` identifies its class statement.
`stable_instance_class` records that the closed program has no `__class__` writer. SSA lift
and the exact solver require both facts to type the allocation for its full lifetime. A wire
record without the stability proof keeps the class identity for allocation and layout guards.
The exact solver carries `Class(id)` from stable allocations, and the
type flow clones a function for an exact-class parameter (`ArgKind::Class`). A probe whose
`driver` calls `norm2(Point(i, i + 1))` gets the clone `norm2__tf_c…`, but the clone compiles
to the same 1,119 instructions: the frontend lowered `point.x` on an unannotated parameter as
a by-name read (`get_attr_generic_obj`), because class layouts (field name to offset) exist only
in the frontend (call_anatomy 4.10, `FieldSlot(ClassId, j)` missing at compile time). A fact
proven in TIR cannot reach an attribute access the frontend has already lowered by name.

For a resolved call to a body shared by all argument types, a demanded result can still
inherit a supplied argument's exact type. The body must be available in this document,
and every successful return must forward the same parameter. The existing call-site
binding maps that parameter to its argument, including the hidden closure offset. The
caller keeps the original call and exception path. This projection creates no context
or code clone, and a runtime type check alone supplies no exact class.

**Order.** Facts first: class layouts as a typed TIR table (one authority, from the frontend's
class model), so any attribute access on a receiver TIR proves exact becomes a field load or
store; then call resolution (union dispatch for polymorphic method sites, function-object calls
of proven functions). Object lifecycle (inline allocation, reference-count elimination,
scalar replacement) follows, since its facts are the same exact classes.

### 5.9 Function cells: the proven callee's object in one load (HF-847 S4 step 2; design 2026-11-21)

**The cost.** A proven method call (5.7) still runs `load_method` to hand the callee its own
function object for its frame record; that load runs the method site's guard and site record,
and on the `super()` path `molt_load_super_method` (15.9% inclusive in `bench_class_hierarchy`).
5.7 names the end state: the object stored once when the class statement runs, read by one load.

**The fact.** Key the cell by the callee, not by (class, member). A function symbol `S` whose
one `func_new` sits in a cell-eligible module's initialization (5.3, `static_module_cells`
C1-C6), runs unconditionally there, and runs once (the body runs once per process, C6) has
exactly one function object per process. Every proven call of `S` needs that object and no
other: `proven_target` already proved the member resolves to `S`, so the cell supplies identity,
never resolution, and a rebinding the premise would see refuses the call before the cell matters.
A method body is inline in its class statement's chunk, so a top-level class gives its methods
this fact; a class in a function or a branch does not, and its calls keep `load_method`.

**The mechanism.** Integration (`static_module_cells`) gives each such `S` a *function cell*: a
word of module `M`'s static cell block after its layout words. The registry row carries the count
(`function_cells`), which sizes the block in codegen, the runtime's bind and its shutdown
retirement; the by-name paths walk only the layout, so the cells are invisible to `dir`,
`getattr` and star imports. After `S`'s `func_new`, integration inserts the cell store (the
cell owns one reference, released at retirement, so an `atexit` method call never reads a freed
object). A proven `load_method` whose call targets `S`, in `M`'s own code that runs only after
the store (`init_dominance`), becomes a `static_cell_load` of the cell: one load, no guard, no
site record, no retain. Every other site keeps `load_method`, and the build diagnostics name
why (cross-module read, undominated read, non-top-level class).

An ordinary method resolved later by TIR can read the same published function cell.
The `static_cell_set` carries the planner's `init_once` fact. TIR checks that the stored SSA
value is the exact method maker, the cell has a single store, and that store dominates
the class stamp, including exceptional paths. The receiver's exact class then witnesses
that this generation's function has been published. For a complete positional call that
alone consumes the pair, the original lookup becomes a cell read and owned aliases of
the callable and receiver, captured before argument evaluation. Methods without a
previously allocated function cell keep their lookup.

**Measured** (2026-11-21, release, macOS arm64; instructions retired, slope of 1M and 2M
iterations, best of 3; main fdfb075361 against the slice). A `method_call`-shaped probe (an exact
`Leaf` receiver, `total = (total + obj.compute(i)) & 16777215`): LLVM 359 -> 268 per iteration
(-25.3%), Cranelift 513 -> 403 (-21.4%); output identical. `bench_class_hierarchy` is flat (LLVM
16.02 G -> 16.05 G, Cranelift 18.16 G -> 18.17 G): its `compute` methods use zero-argument
`super()`, so each holds the `__class__` cell, 5.7 refuses the closure target, and no call in it
is proven. That benchmark moves only after S4's static `super()` and class-cell targets (the
revised order in 5.10), which then read their callees from these cells.

### 5.10 `super()` and closure targets (HF-847 S4; design 2026-11-20, revised 2026-11-21)

**The cost.** `bench_class_hierarchy` (in-process PC sampler, release, 2026-11-20, 648 samples):
89% of samples sit under `call_loaded_method`, and compiled method bodies hold about 23% of
self samples. About 64% is call machinery: `invoke_function_trampoline` 12.8%, frame-scope
`validate` plus `enter` 17.9%, method load and call 13.1%, `molt_load_super_method` 15.9%
inclusive, binding and copying about 13%. Molt runs it at 3,204 instructions per iteration
(LLVM) against CPython 3.15's 2,528 and C's 2. Two refusals cause it. Zero-argument `super()`
gives a method the implicit `__class__` cell, and 5.7 refused every closure target, so
`obj.compute(i)` stays a runtime call. And `super().m(args)` has no static path.

**Design review (2026-11-20, Codex, read-only; findings verified against the code).**
- *Defects on main*, both fixed: `super = len` passed the `super` fast path (HF-955), and
  `exact_receivers` named false receivers for implicit classmethods, `__new__` and a property
  getter's returned function (HF-968).
- *Subclass receivers need class facts before method lowering.* A row (function, building
  class) can name a subclass of the defining class (`self` is exactly `Leaf` in `Mid.compute`
  when only `Leaf` instances reach it through `super()`), but when `Mid`'s method lowers, the
  later `Leaf` has no class model, so its MRO and layout checks refuse. The source graph also
  walks every strict ancestor for `super()` and discards the lexical start class, so a row does
  not establish the selected edge.
- *Static `super()` needs more premises.* The `super` binding is the builtin identity. The
  live `__class__` cell must hold `C`: it starts empty, `nonlocal __class__; del __class__`
  empties it, a metaclass can intercept `__classcell__`, and an inherited `__init_subclass__`
  can run Python before the class statement returns. Cell and argument errors keep their order.
  Every namespace in the C3 suffix must be modeled (`resolve` skips builtin entries; the static
  path must refuse them). A classmethod, staticmethod or property target keeps the generic path.
- *Closure targets are sound with the live closure tuple*, not with a constant cell: cells stay
  writable through `nonlocal` and `f.__closure__[i].cell_contents`. The environment goes after
  the callable handoff and before `self`. User arguments can specialize independently of
  the environment. Inlining needs the callable's activation lifetime.
- *Removing the load needs the member cell*: shared lowering emits `molt_load_super_method`
  for a surviving proven load, so only a static cell (5.9) removes it on every backend.

**Order and status.**
1. HF-955, HF-968: landed.
2. Function cells (5.9): landed (d93fea9125).
3. Class facts before method lowering. A class model needs its members, and a member exists
   only after its method compiles, so this is a split of model building from emission in
   `classes.py`. It serves only a `super()` whose receiver is a *later* subclass (`Mid.compute`
   below `Leaf`), so it follows 4 and 5.
4. Static `super()` with the premises above, in a method whose receiver is its own class or an
   earlier one (`Leaf.compute`), reading the selected callee from its function cell.
5. Closure targets: (a) direct calls, below; (b) inlining proven calls with
   activation-owned callable lifetime; (c) specialization that carries environment facts.

**5a. Direct calls of closure targets (as built, 2026-11-21).** A compiled closure takes its
environment as its first physical parameter (`_MOLT_CLOSURE_PARAM`). The runtime writes a
function object's closure slot (`molt_codegen_abi::FUNCTION_CLOSURE_OFFSET`, word 3, the one
constant `object/layout.rs` reads and writes through) only when it creates the function, and the
binding protocol passes that tuple. So a proven call needs no closure proof of its own: the
frontend admits a closure member in a proven method call (5.7), and the final lowering stage
expands the proven `call_guarded` of a closure target into `FunctionClosure %f` (one load) and
`DirectCall %f, %env, args...`. The tuple is read live, so a write to one of its cells is seen,
as the review requires. `%env` is a *held view* (`ownership_lattice_min`): the function owns
the tuple and outlives the direct call that borrows both, so nothing releases it. Function cells
(5.9) now cover `func_new_closure` makers, so a top-level method's object is one cell load.
`validate_extern_call` accepts `Direct(n)` for a closure declaration with `n - 1` physical
parameters. Unproven guarded calls of closures keep the runtime lane.

A closure member now carries its symbol (`member_from_method_info` reads `ClosureFunc:` hints
as well as `Func:`). Two readers had used "has a symbol" to mean "takes no environment": the
exact-receiver property getter (a plain `call` with no environment) and the staticmethod
result hint. Both now require `Closure.NONE`. A property getter that uses `super()` keeps the
descriptor call.

**Measured** (2026-11-21, release, macOS arm64, instructions retired, interleaved, best of 3;
main 7348a56b6b against the slice; outputs identical). `bench_class_hierarchy` (5M iterations)
LLVM 16.02 G -> 13.02 G (-18.7%; 3,204 -> 2,605 per iteration, CPython 3.15 2,528, C 2): the
outer `obj.compute(i)` is now a direct call; both `super()` calls below it still run
`molt_load_super_method` and `call_loaded_method` (step 4). A probe with an exact `Leaf`
receiver whose `compute` names `__class__` (2M iterations): LLVM 2.32 G -> 1.145 G (-50.7%).
Cranelift: `bench_class_hierarchy` 18.15 G -> 15.02 G (-17.2%), the probe 2.82 G -> 1.565 G
(-44.6%).

**5b. Inlined closure lifetime.** At a proven guarded call on a final-TIR target, the
splice takes the exact function object at the original call site and holds it through
normal and exceptional exits. It loads the live closure tuple from that object and
binds an owned alias to the environment parameter. The function is released after
result capture and frame cleanup, preserving the timing of function weakrefs. Unproven
sites, mismatched physical arity and non-final-TIR targets are refused before editing
the caller. This removes call dispatch while preserving closure cells and the identity
and lifetime of their observable owner.

**5c. User-argument specialization.** A proven guarded site's `StaticCall` carries its
executable `FunctionId`, the original function object's SSA value and its Python arguments.
Demand and exact argument facts map to physical parameters starting after the closure tuple.
The tuple's context entry stays dynamic. Different instances may share specialized code
while each call reads its own live environment. Facts about captured cell contents still
require writer and lifetime proofs (3.3).

**Step 4. Static `super()` (as built, 2026-11-21).** `_proven_super_member` decides
`super().m(args)` in a method of class `C` at compile time when all of these hold, else the
runtime `load_super_method` stays:
- The `__class__` cell holds `C`. `C` has no custom metaclass, so `type.__new__` fills the cell
  before `__set_name__` or `__init_subclass__` can run Python. `C`'s body holds no
  `nonlocal __class__` (`class_cell_rebound`). The graph names no reflective cell writer: the
  new premise field `cell_writer` is the first named `cell_contents` store or delete, a
  computed-name attribute store, a frame-locals writer (a PEP 667 proxy writes free variables)
  or a native extension. A new writer is a global premise change. One stdlib module writes a
  cell: `dataclasses._update_func_cell_for__class__` (for `slots=True`), so a program that
  imports `dataclasses` keeps every `super()` dynamic. Narrowing that writer to the cells of
  the classes it decorates is open.
- The first parameter holds exactly class `E` (an `exact_receivers` row), and `C` is on `E`'s
  MRO, so the selection walks `E`'s MRO after `C`.
- Every class on that MRO is namespace-closed with a proven binding and no custom metaclass,
  and no builtin class but `object` follows `C` (`resolve` skips builtin namespaces).
- The member is a plain function (a classmethod, staticmethod or property keeps the descriptor
  protocol), called positionally with its exact arity, its entry adopts its arguments, and no
  code object is reachable (`code_rebinding`).
- The `super` name is the builtin (HF-955 proves it before this runs).

The proven load carries `proven_target` (`ir_schema` admits it on `LoadSuperMethod`;
`op_instance_effects_for_op` gives it the same effect-free, nothrow rule as a proven
`load_method`), and the call is a proven `call_guarded`. Function cells (5.9) read the callee
for a proven `load_super_method` as they do for a `load_method`, so the runtime selection
leaves the loop; a closure callee is entered as 5a describes.

**Measured** (2026-11-21, release, macOS arm64, instructions retired, best of 3; outputs
identical). `bench_class_hierarchy` LLVM 13.02 G (5a) -> 8.80 G (-32.4%); against main before
S4, 16.02 G -> 8.80 G (-45.1%), 3,204 -> 1,760 per iteration, below CPython 3.15's 2,528.
Cranelift 15.03 G -> 10.87 G (-27.7%; 18.15 G before S4). The
remaining `super()` call is in `Mid.compute`, whose receiver is the later subclass `Leaf`
(step 3).

## 6. Expected effect, stated as removed runtime calls

The measure is runtime calls executed per iteration in a sampled profile (AGENTS.md, closed
world). Per `fib` call today: runtime compare, subtract and add calls, boxing, and reference
counting. After slices 1 to 3 the body is inline tag tests, raw arithmetic with overflow
branches, and a direct recursive call. The frame record (stage 1) and the recursion-depth check
(stage 2) remain. Section 9 assigns each benchmark's measured runtime share to a slice. A
slice's claim is its interleaved A/B in instructions retired against the C reference.

## 7. Relation to other work

- E1 (inliner) inlines the clones; today it finds few static edges (0 sites inlined in
  `bench_class_hierarchy`, 2026-11-14) because method calls are opaque. Slices 3 and 4 create
  the edges.
- E2 (scalar replacement) and the `bench_struct` teardown share need exact allocation-site
  types and non-escape; slice 4 provides the types.
- Stage 6 (unboxed loop carriers) and stage 7 (TIR inlining) consume these facts.
  Codon's structured CIR exposes counted loops before they become branches (§3.1,
  §4.2). Its LLVM coroutine elision also depends on a proof that the coroutine
  handle does not escape (§3.5). Carry the loop, capture and lifetime facts through
  shared TIR so Cranelift and LLVM can consume the same decision. Preserve Python
  close, throw, finally and finalizer effects at their original boundaries.
- The thread-record words (stage 2) and frame records (stage 1) are the per-call costs that
  remain after this program; they are already in flight.

## 8. Rejected alternatives

- Bottom-up specialization on caller-side `Repr` with budgets (`03` 2.3): no top-down flow, so
  call sites have no facts to key on; it excludes recursion and caps by constants.
- k-CFA call-string sensitivity: `k` is arbitrary, and CPA is more precise for argument types.
- Annotation-driven types: unsound for subclasses and wrong annotations.
- A runtime tracing JIT: Molt emits standalone AOT programs with no interpreter.
- Re-entering source inference from backend passes, as Codon's CIR can do for
  generated templates (§3.3): resolve a demanded helper from a typed context in
  the shared mid-end. Its return and effect summary can remain valid after an
  implementation edit that invalidates an inlining consumer (§3.10). Coalesce
  equivalent bodies before emission where ABI, effects and relocations agree,
  retaining each context's invalidation dependencies. Codon's later LLVM
  merging shrinks output after realization has already incurred its cost.

## 9. Suite technique table

**Measurement (2026-11-14).** LLVM release, macOS arm64, `/usr/bin/sample` at 1 ms. Each
benchmark runs from a driver (`import bench_x; for _ in range(R): bench_x.main()`), where R
gives at least `MIN_SAMPLES` = ⌈(1.96 × 0.5 / 0.025)²⌉ = 1,537 self samples, so each share is
known to ±2.5 points. 53 of 57 benchmarks reached that count. A share is the named symbols'
self samples over the program's total. Symbol rows decide the attribution.
`opt_explorer.classify_symbol` counts as compiled only the symbols of the module it is given;
this run gave it the driver's module, so time in the benchmark's own compiled code is in
`other runtime`. The allocation and reference-counting bucket shares are unaffected.

The shares are VERIFIED. The slice that removes each share is INFERRED from the mechanism.
Each slice proves its own claim with an interleaved A/B in instructions retired against the C
reference (section 6).

| Mechanism (symbols) | Benchmarks: measured share | Removed by |
|---|---|---|
| Generic `int` operations on values only ever `int` (`lshift_impl`, `strict_integer_from_obj`, `scalar_value_bits`) | loop_invariant_shift 49%; csv_parse 10%; builtin_dir_slice 10% | Slices 2–3: typed `int`, raw arithmetic with an overflow edge |
| Generic reduction step over list elements (`step`, `exact_number`) | prod_list 82%; sum_list_hints 65%; sum_list 58%; min_list 56%; max_list 52% | Slice 3, together with typed list storage (list-storage lane) |
| Range materialized as a list (`molt_list_from_range`) | sum_list_hints 22%; sum_list 21%; max_list 17%; min_list 17% | List-storage lane; slice 3 supplies the element type |
| Generic indexing on a receiver of fixed type (`index_impl`) | tuple_index 49%; tuple_pack 30%; tuple_slice 19%; deeply_nested_loop 12%; class_hierarchy 11% | Slice 2: a known receiver type selects the inline index |
| Shape and attribute lookup (`object_shape_id`, `object_has_class_shape`, `class_instance_shape_id`, `molt_get_attr_object_ic`) | attr_access 34%; descriptor_property ≥ 9% | Slice 4: closed-world layouts, fixed-offset loads |
| Callee lookup and trampolines (`runtime_callable_target_ptr`, `invoke_function_trampoline`) | class_hierarchy 13% | Slices 3–4: direct calls to resolved methods |
| Call binding (`bind_python_frame`, `builtin_call_binding`, `direct_call_function`) | itertools_tee 52%; ptr_registry 11%; heap_canonical_cache 10%; fib 8% | Slice 3 (direct dispatch) and slice 6 (unboxed convention) |
| Time inside compiled bodies (boxed values, tag tests, frame records) | masked_shift_accumulator 100%; sum 100%; method_default_binding 90%; deeply_nested_loop 60%; fib 53%; tuple_index 46% | Slices 3 and 6 remove boxing; stages 1–2 own the frame and thread-record cost |
| Result-list growth and join (`insert_with_projection`, `molt_list_append_with_projection`, `molt_string_join_attached_body`) | nine `str_*` benchmarks: 32–56% (top-8 symbols only, so a lower bound) | List-storage lane (not this program) |
| Hash-table probing (`dict_probe_key_with_hash`, `set_insert_absent_with_hash`, `set_probe_with_hash`) | set_ops 55%; dict_comprehension 37%; counter_words 19% | No owner yet: runtime data structures |
| Allocation, reference counting, cycle collection (`alloc_object_zeroed_with_aux_policy`, `dec_ref_ptr_terminal_body`, `clear_cycle_edges_with_sink`) | suite mean 13.5% (buckets); gc_pressure, matrix_math, struct, object_class_lifecycle, exception_heavy lead | E2 scalar replacement; slice 4 supplies allocation-site types |
| Byte algorithms (`replace_bytes_impl`, `bytes_count_impl`, `_platform_memmove`) | bytearray_replace 91%; str_count_unicode_warm 61%; bytes_find 15% | Runtime algorithm work (not this program) |

Codon's Figure 4 removes a second dictionary lookup and reports a 12% word-count
improvement. In Molt this belongs at the typed TIR operation, where both lookups
and their intervening value update are still visible. A reusable probe requires
an exact dictionary and key protocol, and proof that no intervening effect can
invalidate the slot or observe the fused lookup. Preserve evaluation order and
the exception and lifetime boundaries. LLVM receives the proven operation.
[Codon, §4.1](https://doi.org/10.1145/3578360.3580275).

**Reading.** By each benchmark's leading symbol: slices 2–4 and 6 own 22 of 53; the
list-storage lane owns 9; allocation, reference counting and cycle collection lead 7; byte
algorithms 4; hash tables 3. Eight lead with a symbol the profile does not resolve (a
truncated or deduplicated name, a closure, or an import-path lookup); they need a symbolized
rerun before anyone claims them. Hash-table probing is the one leading cost with no owner.
