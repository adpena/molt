# 78. Whole-program type flow and specialization (the P0 program)

Status: design, 2026-11-14; slice 1 implemented; slice 2's seeding blocked on an unseen-caller
premise, slice 3 recommended next (section 5.3, 2026-11-17). Owner decision 2026-11-14: this is
the P0 compiler program.
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
- **Codon** (Shajii et al., CC 2023): bidirectional inference; every function is realized per
  argument-type tuple; integers are machine integers (Molt keeps Python `int` semantics instead).
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

One pass, `type_flow`, in the TIR module phase (`module_phase.rs`), after the call graph and
before the inliner, over the whole user program. HF-773 forbids a second optimizer, so it is not
built in the Python mid-end. Its results are facts on typed SSA values (parameter types, call
targets, return types) that flow one way to codegen.

The module phase runs after per-function optimization, so the analysis reads optimized bodies.
A function whose entry facts change re-runs the per-function pipeline, as the module phase
already does for bodies the inliner changes. Today a call result is `DynBox`: `type_refine`
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

### 3.4 Specialization key: only what changes the code

A parameter is relevant to a function when its type reaches an operation whose lowering
depends on it: arithmetic, comparison, truthiness, attribute access, subscript, iteration, or a
call that dispatches on it, including through a callee where the parameter is relevant. A
backward pass over the body (and the callees' relevance) computes the set. A parameter that is
only stored or passed along opaquely never splits a context.

After optimization, contexts of one function whose bodies are identical merge into one clone
(compared by body digest). The number of clones is the number of reachable contexts that
compile differently.

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

- Intrinsics declare result types as functions of argument types in the intrinsics manifest,
  as generated typed signatures.
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
- The thread-record words (stage 2) and frame records (stage 1) are the per-call costs that
  remain after this program; they are already in flight.

## 8. Rejected alternatives

- Bottom-up specialization on caller-side `Repr` with budgets (`03` 2.3): no top-down flow, so
  call sites have no facts to key on; it excludes recursion and caps by constants.
- k-CFA call-string sensitivity: `k` is arbitrary, and CPA is more precise for argument types.
- Annotation-driven types: unsound for subclasses and wrong annotations.
- A runtime tracing JIT: Molt emits standalone AOT programs with no interpreter.

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

**Reading.** By each benchmark's leading symbol: slices 2–4 and 6 own 22 of 53; the
list-storage lane owns 9; allocation, reference counting and cycle collection lead 7; byte
algorithms 4; hash tables 3. Eight lead with a symbol the profile does not resolve (a
truncated or deduplicated name, a closure, or an import-path lookup); they need a symbolized
rerun before anyone claims them. Hash-table probing is the one leading cost with no owner.
