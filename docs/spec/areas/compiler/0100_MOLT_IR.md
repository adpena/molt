# Molt IR

Molt IR is typed SSA with explicit control flow, ownership, and effects. It exists in multiple layers (HIR, TIR, LIR), each enforcing stronger invariants to enable aggressive optimization while preserving soundness.

## Module transformations and demanded analyses

The shared cached TIR runner optimizes functions before the module phase.
Luau requests SimpleIR projections within bounded optimization waves.
Native and WASM retain final owned TIR through module optimization or the explicit
terminal-drops-only route. LLVM and FactGraph borrow preparation input and retain only optimized TIR;
neither cache hits nor misses build an unused SimpleIR projection. Owned module
assembly accepts moved LLVM source bodies or borrowed diagnostic inputs, retaining
source order and canonical extern declarations. LLVM freezes source ABI metadata
before retiring those bodies and consumes each TIR function after LLVM lowering.
`molt-passes::tir::module_phase::run_module_pipeline` builds the initial call
graph and summaries consumed by inlining and generator fusion, then performs
slot promotion, changed-body optimization, and terminal ownership finalization.
It returns a deterministic `BTreeSet<String>` of changed function names. The
SimpleIR wrapper consumes this set directly for selective back-conversion and
returns its count; the owned-TIR wrapper returns the transformed bodies. Changes
from multiple passes name a function once. Body order and original SimpleIR
indices remain independent of set order.

Native and LLVM apply the shared typed copy-alias, split-field and direct-raise
transformations before final type refinement and ownership planning. Native
projects each final typed body and its representation facts together through
the actual emitted value-name allocation. The consumed typed body is then
retired. Dead-function removal and partition externalization move or retire
each source body and its facts as one pair. Every normal, skip and batch native
codegen entry consumes that pair; codegen does not lift the projection or rerun
the optimizer to reconstruct facts. Extern declarations carry no local plan.
This preserves the existing source transport without treating it as a second
semantic authority. WASM uses the same retained module owner and runs typed
dead-operation elimination before release insertion. Existing full or
exception-only release markers prevent that mutation. Its final projection
produces each body, scalar facts and demanded LIR plan together, sharing the
same value-range/representation derivation. Function retention retires whole
pairs, while escaped-callable admission selects generic lowering without
changing operations. Public compile/diagnostic/checked entry points all consume
this owner; generic frame planning never lifts or optimizes the projection.
Each body and its facts retire at emission. Native and WASM still project
source control flow/names; carrying typed control flow through machine lowering
remains a separate convergence obligation.

`MOLT_DUMP_REWRITTEN_FUNC` explicitly requests the final native/LLVM source
projection after module and ownership finalization. Ordinary LLVM compilation
does not construct that diagnostic projection. `MOLT_DISABLE_SPLIT_FIELD_DEFOREST`
continues to disable the shared typed transformation by presence.

Final analyses are requested where their inputs and consumers are known.
FactGraph builds a graph and summaries over the final local module and requests
`CallFactsTable::build_for_function` only for the selected function. Native leaf
metadata is computed after its later lifetime and splitting transformations;
it must account for every call in the bodies codegen will emit. Ordinary module
transformation does not construct diagnostic CallFacts tables. These ownership
rules apply to native, WASM, LLVM, and Luau drivers regardless of build profile.

## IR stack
- **HIR (High-level IR)**: desugared AST with explicit control flow.
- **TIR (Typed IR)**: SSA with concrete `MoltType` for every value.
- **LIR (Low-level IR)**: explicit memory operations, reference counting, and layout-level access.

## Canonical backend contract
- The canonical optimization/codegen contract is representation-aware SSA carried
  through TIR and, where needed, made explicit in LIR.
- Representation is part of the IR contract, not backend-local recovery
  metadata. Values proven inline or unboxed must stay in that lane through SSA
  values, block parameters, and join points until an explicit conversion is
  required.
- `Box`, `Unbox`, `Cast`, and `Widen` are first-class boundary operations. They
  are the canonical way to cross representation boundaries.
- The current `SimpleIR` transport still accepts `type_hint` metadata for
  legacy consumers. It is a transitional compatibility detail, not the
  long-term semantic contract. The scalar `fast_int` / `fast_float` hints are
  retired: no backend read them, and the transport now rejects them.
- SimpleIR local-name identity for `copy_var`/`load_var`/`store_var`/
  `delete_var` is carried through TIR as `_var` transport metadata when a
  re-lift must be lossless. SSA operands remain the value authority; `_var`
  only preserves the original stream-local spelling for re-emission and
  lifetime-boundary consumers.
- Backend-specific shadow state or side-channel unboxed tracking is
  implementation debt and must not be treated as a stable interface.
- Container constructor result types come from
  `molt_ir::tir::op_semantics::container_constructor_result_type`, consumed by
  shared type refinement and exact allocator provenance. Constructor results
  cannot inherit their first element's scalar type; retained binding snapshots
  preserve the constructor's semantic container type. Representation planning
  does not reseed result facts from output names.
- Container alias identity consumes generated canonical alias roles and copy
  classifications. Pure copies, type guards and owned binding captures preserve
  heap identity; a capture's retain remains an ownership effect. Escape and
  mutation invalidate physical-storage proofs across those aliases. Semantic
  container type, exact builtin class, physical layout and ownership remain
  distinct facts, and ambiguous producers cannot authorize direct storage access.
- Native int, bool, and float lowering have retired their raw scalar shadow
  lanes. Integer raw-carriers are proven by the value-keyed
  `repr_by_value_for` / `value_range_for` path and projected through
  `SimpleValueNames` into `ScalarRepresentationPlan::repr_by_name`, where native
  consumes `is_raw_int_carrier_name`, `is_inline_safe_int_name`, and
  `is_full_deopt_int_name`. Bool and float carriers still derive through
  `is_bool_unboxed` / `is_float_unboxed` as `Repr::Bool` /
  `Repr::FloatUnboxed` views over the native name map. Bool/F64 names floor to
  boxed storage in this native name-keyed authority and are raised only by
  explicit carrier eligibility filters. Non-primary bool and float values are
  represented immediately in their main boxed I64 variable. The 100-year
  endpoint is to move the bool/F64 representation facts through typed
  value-keyed TIR/LIR and every backend without re-deriving native name facts.
- Native operation selection consumes `ExactScalarWitness`, minted only by
  the canonical exact SSA producer solver. The name projection preserves that
  witness across admitted identity and storage joins; annotations cannot create
  it. Arithmetic, in-place operations, comparisons, unary operations and truth
  branches share it. Float payload readers take a bound `NumericOperand`;
  arbitrary boxed integers retain runtime conversion and overflow semantics.
  Mixed comparisons additionally require `FloatComparisonOperands`, which
  admits integer conversion only with the existing inline-int47 range proof.
  Full-width integers keep exact runtime comparison: arithmetic's rounding
  conversion is not a comparison proof. This preserves Python's
  [precision-preserving numeric comparisons](https://docs.python.org/3.12/reference/expressions.html#value-comparisons).
  The native planner projects TIR facts directly instead of lowering another
  semantic-floor LIR body or maintaining an integer type solver. The generated
  field-role table distinguishes copy/load local-name metadata from semantic
  alias inputs. Malformed aliases with a second read cannot mint a witness.
  LIR has one producer/range representation path. Its parameter and return ABI
  projections use `Repr::carrier_type`; its verifier checks raw interpretation
  consistency and allows boxed values with scalar annotations.
- `LirRepr::Ref64` is a runtime reference-word lane for proven reference
  values such as stack-eligible user-class allocations. It is physically an
  `i64` on WASM and native pointer-sized backends, but it is not the semantic
  `I64` scalar lane: object truthiness, dynamic signature boundaries, and
  reference provenance must continue to use runtime-object semantics.
- `Repr::carrier_type` projects exact producer/range facts into machine
  carriers for every scalar kind. An `int`, `float` or `bool` annotation alone
  keeps the value boxed. Frozen native linkage rows, LLVM entry arguments,
  phis and explicit unboxing, and proven LIR use that same projection. Return
  slots join the carriers of executable return producers, including boxed
  integer paths; callers and providers serialize the same decision. Reference
  counting consumes the lowered physical carrier, so an annotated object
  retains its identity and ownership. A semantic `None` annotation likewise
  preserves its boxed word during materialization and frame-home publication;
  only a `ConstNone` producer supplies the singleton. This follows
  [Python's annotation semantics](https://docs.python.org/3.12/reference/compound_stmts.html#function-definitions)
  and [LLVM's typed phi inputs](https://llvm.org/docs/LangRef.html#phi-instruction):
  incoming edges reconcile to the producer-proven slot, never an annotation.
  Progressive MLIR consumes the same `ProducerCarrierPlan` for signatures,
  block arguments, returns and operand classification. It has no Molt runtime
  provider: preflight refuses a parameter, block argument or returned value
  without a producer-proven raw carrier, naming the function and SSA value.
  An annotated entry parameter is unsupported; constants flowing into internal
  block parameters can prove a raw carrier. No boxed MLIR ABI is invented.
  Exact scalar loops retain their producer proof in functions with exception
  polls: only resolved exception targets and explicit handler entries impose
  conservative handler inputs. An ordinary control-flow label or a poll in the
  source block does not make that block an exceptional entry. Refinement and
  exact provenance share this classification and the canonical CFG resolver.
  The generated scalar-value forwarding rule carries existing producer facts
  across home publication and owned binding aliases. It grants no ownership,
  heap-alias or effect-elision permission: an owned alias remains a distinct
  droppable reference even when its mathematical scalar value is known.
  A local LLVM invocation balances its trace-entry attempt on every return,
  including suspension and early errors. Its guard retires that credit before
  releasing frame homes can reenter Python; an inherited context never pops
  its caller's frame.

## Implementation layout
- Keep Rust crate entrypoints (`lib.rs`) thin; implement runtime/backend subsystems in focused modules under `src/` and re-export from `lib.rs`.

## Core structure
- **Module**: a set of functions, globals, and type definitions.
- **Function**: SSA values, basic blocks, and a declared effect summary.
- **Block**: parameters + instruction list + terminator.
- **Terminator**: `Jump`, `Branch`, `Return`, `Throw`.

## Type system (minimum set)
- **Primitives**: `Int`, `Float`, `Bool`, `None`.
- **Objects**: `Class(Id)`, `List(T)`, `Dict(K,V)`, `Tuple([...])`,
  `Iterator(T)`, `Str`, `Bytes`, `MemoryView`, `Range`, `Slice`.
- **Unions**: `Union(T1, T2, ...)`, `Any` (Tier 1 only).
Coverage status and planned additions are tracked in `docs/spec/areas/compat/surfaces/language/type_coverage_matrix.md` with TODO tags for new ops.

## Instruction categories (minimum set)
- **Constants**: `ConstInt`, `ConstFloat`, `ConstBool`, `ConstNone`, `ConstStr`, `ConstBytes`.
- **Arithmetic/logic**: `Add`, `Sub`, `Mul`, `Div`, `Eq`, `Lt`, `Gt`, `Is`, `Contains`, `And`, `Or`, `Not`.
- **Control**: `Phi` (TIR), `Jump`, `Branch`, `Return`, `Throw`, `TryStart`, `TryEnd`, `CheckException`, `LoopStart`, `LoopIndexStart`, `LoopIndexNext`, `LoopBreakIfTrue`, `LoopBreakIfFalse`, `LoopBreak`, `LoopContinue`, `LoopEnd`.
- **Calls**: `Call`, `CallIndirect`, `CallBind`, `InvokeFFI` (with declared effects),
  `LoadMethod`, `LoadSuperMethod`, `CallLoadedMethod`, `CallargsPushMethodReceiver`.
  An attribute call in CPython's stack form loads the method as a pair, as
  CPython's method-form LOAD_ATTR does: the callable and `self_or_null`.
  `LoadSuperMethod` loads the pair for zero-argument `super().name(...)`. A
  positional call that cannot suspend passes both to `CallLoadedMethod`. A
  call with keywords or a suspending argument pushes `self_or_null` into its
  argument builder first and calls `CallBind`. A call in CPython's
  CALL_FUNCTION_EX form (an `*` or `**` expansion, or more operands than
  CPython keeps on its stack) loads an ordinary bound attribute and calls
  `CallIndirect`.
- **Imports/modules**: `Import`, `ImportFrom`, `ModuleCacheGet`,
  `ModuleCacheSet`, `ModuleCacheDel`, `ModuleGetAttr`, `ModuleGetGlobal`,
  `ModuleGetName`, `ModuleSetAttr`, `ModuleDelGlobal`. These are observable
  runtime operations, not structural SSA copies; passes must preserve their
  cache/module-dict mutation, raising, and reference-count behavior even when
  results appear dead.
- **Object/layout**: `Alloc`, `LoadAttr`, `StoreAttr`, `GetAttrGenericPtr`, `SetAttrGenericPtr`, `GetAttrGenericObj`, `SetAttrGenericObj`, `LoadIndex`, `StoreIndex`, `Index`, `Iter`, `Enumerate`, `IterNext`, `ListNew`, `DictNew`, `Len`, `Slice`, `SliceNew`, `BytearrayFromObj`, `RangeNew`, `ClosureLoad`, `ClosureStore`.
- **Bytes/Bytearray/String**: `BytesFind`, `BytesSplit`, `BytesReplace`, `BytearrayFind`, `BytearraySplit`, `BytearrayReplace`, `StringFind`, `StringFormat`, `StringSplit`, `StringCapitalize`, `StringStrip`, `StringReplace`, `StringStartswith`, `StringEndswith`, `StringCount`, `StringJoin`.
- **Exceptions**: `ExceptionNew`, `ExceptionLast`, `ExceptionClear`, `ExceptionContextSet`, `Raise`. Raise sets the implicit `__context__`. For `raise X from Y` the frontend evaluates both operands and passes them to the runtime call `molt_exception_prepare_raise`, which turns each class into an instance and sets the explicit `__cause__` and so suppresses the context. No IR op carries the cause.
- **Generators/async**: `GenSend`, `GenThrow`, `GenClose`, `IsGenerator`, `AIter`, `ANext`, `CallAsync`, `StateSwitch`, `StateTransition`, `StateYield`, `StateSet`, `IsPending`, `TaskWait`.
  - Source `StateTransition`/`StateYield` become explicit activation exits before
    terminal ownership: state writes, a canonical poll call, exact pending test,
    conditional wait registration, and ordinary returns. Closure storage owns
    values that persist across invocations.
  - `StateSwitch` reads the saved state slot (`self` payload -16). Its final
    transport carries an explicit `state_targets` map from saved state IDs to
    control-label IDs; backends neither equate the two namespaces nor infer
    resume points from operation indices. `StateSet` also permits running states
    that are not dispatch targets. Hidden suspension ops are rejected at native,
    LLVM, and WASM emission.
  - Channel operations are runtime-bound calls. Their owning async wrappers
    explicitly await when a runtime call returns pending; channel names do not
    create source-level suspension or channel-specific IR opcodes.
- **Vector**: `VecSum`, `VecProd`, `VecMin`, `VecMax` (one bounded chunk of a fused-loop reduction over the loop's own iterator of a list, tuple or range; operands the iterator, the accumulator and the loop target's previous value; result the tuple (result, last, count, more). The op consumes the `count` next items whose updates provably run no Python code, and result and last are then exactly the accumulator and loop target the loop leaves after them; it stops before the first item it does not admit, at the iterator's end, or at its chunk bound (more). The frontend runs it in a chunk loop that publishes both bindings before each back edge and rereads them, and then the ordinary loop on the same iterator. See 0190 "Fused loops".)
- **Index**: `OperatorIndex` (`operator.index(x)`: range()'s conversion of a bound to an exact int, calling `__index__` where the value is not already an int.)
- **Guards (Tier 1)**: `GuardType`, `GuardTag`, `GuardLayout` (the receiver's class came from one named class statement, at its layout version; HF-220).
- **RC ops (LIR)**: `IncRef`, `DecRef`, `Borrow`, `Release`.
- **Conversions**: `Box`, `Unbox`, `Cast`, `Widen`, `StrFromObj`.

## Invariants
- **Callable ABI authority**: `func_new` and `func_new_closure` use the compiled
  function's exact linkage and representation plan. `builtin_func` instead uses
  the runtime callable ABI (symbol, arity, trampoline transport) that
  `tools/gen_wasm_abi.py` generates into
  `runtime/molt-ir/src/runtime_callable_abi_generated.rs` from the intrinsic and
  WASM ABI manifests; neither an integer carrier nor an operation's claimed
  arity authorizes a runtime signature. LLVM and Cranelift validate that
  contract before declaring the target, and LLVM classifies the declaration from
  the same table. WASM applies its target availability filter to the same arity
  authority. `molt-tir` maps the transport to one `TrampolineKind` for every
  backend: ordinary runtime trampolines unpack boxed arguments unchanged;
  call-frame entries receive `(closure, argv, argc)` directly. Native-only
  providers retain their ABI without becoming WASM imports. Raw/borrowed helpers
  excluded from Python callable publication remain excluded. A direct `call`
  to a runtime entry uses the generated boxed ABI, or, for a manifest row that
  opts in with `raw_call = true` (a raw-bits return through an all-i64 word
  signature, such as the host init's `molt_runtime_init`), its `RawCall` form
  in the generated `RuntimeFn` table (`runtime/molt-ir/src/runtime_fn_generated.rs`):
  LLVM binds that word without an owner and never releases it. Raw machine helpers whose
  operands or results are not Python values never become generic call targets.
- **Late lowering stage (HF-821)**: for a target whose backend consumes final TIR
  (`FinalLowering::FinalTir` in `TargetInfo::lowering`; LLVM today), the drop
  phase's `finalize_function_drops` first runs `run_late_lowering`. It expands
  each value-level fast path into representation-level control flow over three
  final-TIR ops (`final_tir_opcodes`, never on the SimpleIR wire):
  `IsInlineInt` (the runtime's tag test), `UnboxInlineInt` (the sign-extended
  47-bit payload, total) and `CallRuntime`, whose callee is the typed
  `TirOp::runtime` entry that the verifier requires on it and forbids elsewhere.
  First slice: `Add`/`Sub`/`InplaceAdd`/`InplaceSub` on two boxed words become a
  tag-test diamond whose fast arm adds the payloads exactly (`RawI64FullDeopt`)
  and boxes through the full-range path, and whose cold arm calls the op's own
  entry, `TirOp::numeric_runtime` (`numeric_runtime_fn`, generated from the WASM
  ABI manifest's `numeric_runtime_selector` rows). GVN then merges a value's
  repeated tag tests and DCE drops the rest; debug builds verify after each
  pass. Backends lower the primitives through molt-codegen-abi's shared
  `boxed_word` generators. A function without an eligible op is unchanged.
- **SSA**: every value is defined once; all uses are dominated by the definition (loop index carried via block params).
  SimpleIR construction uses one definition-stack walk over the augmented
  dominator tree. Explicit branch and state-resume arguments read those stacks
  at the terminator; exception arguments read them at the transfer operation,
  resolving their target through the same CFG exception relation used for
  argument placement and liveness. Each authored transfer ends its source block.
  Ancestor definitions remain active through each dominated subtree, and each
  definition push has one matching cleanup pop, enumerated from the same
  immutable block arguments and source definitions without a separate log.
  Handler and resume placement keeps a live binding direct when its sole
  authored defining block strictly dominates the target in that augmented
  graph. Multiple writes in that block select its final binding; transfer
  splitting prevents later definitions from supplying an earlier edge.
  Remaining implicit arguments extend the same definition-site census before
  iterated dominance-frontier placement. Mutable multi-block definitions,
  self-edges and disconnected roots retain their required transport. This
  avoids recreating dominating values at every cleanup suffix; dense liveness
  storage and live-in scans still have their own complexity costs.
  Missing reaching definitions
  remain construction placeholders until actual uses materialize them locally;
  construction does not retain full variable environments at every block exit.
- **Dominance**: one dominator core (`runtime/molt-ir/src/tir/dominators/indexed.rs`)
  serves TIR block dominance (`DominatorTree`), program-point dominance, SimpleIR
  operation dominance and SSA construction. Semi-NCA (Georgiadis, "Linear-Time
  Algorithms for Dominators and Related Problems", 2005) builds each tree, as
  LLVM's `DominatorTree` construction and Cranelift's `DominatorTree` do. One
  iterative walk numbers the tree, and `dominates` compares two pre/post
  intervals in O(1), as LLVM compares DFS in/out numbers and Cranelift compares
  pre-order intervals. No query walks an idom chain. Trees are immutable: a CFG
  change builds a new tree with its own numbering, and the analysis manager drops
  the cached `ImmediateDoms` tree with every other CFG-sensitive analysis.
  Dominance-frontier walks stop where an earlier walk for the same join already
  recorded it, so frontier work is the frontier size plus the edges. No CFG or
  dominator-tree walk recurses: function size never sets native stack depth.
- **Executable edges**: typed block-argument meets and LIR branch parameters are
  computed only from blocks reachable from the function entry through explicit
  terminators or implicit exception edges. Structural loop metadata blocks that
  survive optimization but are not executable must not feed typed block
  arguments; LIR lowering canonicalizes their bodies to `Unreachable` before
  representation verification.
- **Explicit effects**: calls and memory ops must declare their effect class.
- **No implicit exceptions**: operations that can fail must either be guarded or emit `Throw`.
- **Tier separation**: Tier 0 disallows `Any` and speculative guards; Tier 1 allows guards with deopt exits.
- **Predicate authority**: Python `And`/`Or` select an operand unchanged;
  their result is not unconditionally `Bool`. Exact operand provenance and
  effects own representation admission. Integer-overflow replay requires an
  exact-integer body and a callback-free, nonthrowing Boolean guard; annotations
  alone establish neither condition.
- **Backend megafunction splitting**: when the native/WASM backend splits an
  oversized `SimpleIR` function, generated chunk functions must preserve the
  original SSA def-use contract explicitly. Values defined before a split
  boundary and read after it travel through one synthetic split frame; loads use
  the canonical `Index`/`index` op, stores use `StoreIndex`/`store_index`, and
  generated `load_index`-style aliases are invalid. A single chunk-return
  protocol owns both the generated ABI and its caller: intermediate payload
  chunks fall through with a void ABI, the final payload chunk returns the
  owner's value, and payload-free bodies use value-ABI continuation statuses
  so a terminal cloned exception/cleanup suffix can stop later chunks. The
  owner's authored ABI is preserved even when no payload returns survive.
  Empty owner exits remain empty until machine lowering. Live-out
  frame stores belong on the normal path before the synthetic jump that skips a
  cloned suffix, never after terminal cleanup code.
  All chunk-return protocols share that frame-transport placement. Existing
  ownership/drop facts remain exactly once at the start of each chunk, before
  generated frame loads; the replacement owner stub does not inherit them.
  The planner indexes borrowed per-name read/definition positions instead of
  retaining a whole live/defined set at every operation. Same-operation reads
  precede writes; refusal leaves both the input contract and occupied symbol
  namespace unchanged.
  Generated verifier label and region roles own split-boundary and suffix
  admission. A cloned suffix starts at a balanced region boundary, and every
  generated label reference resolves inside its chunk. Frame-slot verification
  uses the allocated frame identity and slot layout, never a name-prefix guess.
  Native preparation splits before the first TIR lift, including whole-program
  linkage-ABI capture before object batching. WASM uses the same splitter before
  TIR and after final rewrites, but preserves its straight-line-only admission.
  Both stubs and chunks carry the typed `FunctionIR.codegen_partition` fact,
  defaulting to false. This is a restrict-only optimization boundary, not a
  Python-frame capability: it survives JSON/NDJSON, extern reconstruction,
  function-contract hashing, and TIR artifact transport. Shared inline
  eligibility refuses these functions even when optimization makes them tiny;
  LLVM also emits its real `noinline` attribute.
  Split results report explicit chunk-to-source provenance. Native module
  context preserves that report through batch serialization; shared stdlib
  ownership follows original source identity through one
  `stdlib_module_symbols` authority and never decodes generated symbol names.
  Callable task/trampoline/closure metadata is collected by the shared
  `molt_tir::trampolines::CallableMetadata` authority before splitting. Native
  and WASM retain source facts even when physical partitions separate callable
  creation from marker attributes, while final ABI catalogs include generated
  functions. Marker facts are captured at their source operation, before
  writes, and canonical definitions invalidate stale value/callable bindings.
  Final bodies contribute constructor/escape facts only; lowered marker
  operands never re-author retained source metadata. Conflicting facts,
  unknown values on known-callable markers, and overwritten callable bindings
  fail with a named diagnostic. The existing `_poll` task-eligibility suffix is
  not a generated-partition ownership rule.
- **Native artifact boundary**: `NativeArtifactKind` separates real `object`
  output from `archive` transport. Backend CLI `--native-output-kind` and the
  daemon job `native_output_kind` field carry this choice explicitly; direct
  callers default to `object`. Object output compiles the complete graph into
  one object module, retaining per-function splitting but refusing shared
  stdlib extraction and cross-object batching. Normal executable builds request
  deterministic, non-thin archives for application and shared-stdlib artifacts,
  including one-member and empty-stdlib cases. Emitted object headers select
  GNU/ELF, Darwin/Mach-O, or COFF archive encoding; mixed target identities fail
  before publication. Ordered member names and zero owner/time metadata are
  independent of temporary paths. Batch members are mapped read-only and the
  archive streams through the shared atomic publication boundary. There is no
  partial-link subprocess, hand-written relocation rewriting, or archive hidden
  behind an object artifact kind. The final linker whole-loads compiler archives
  to retain ordinary object inclusion semantics. Cache identities include the
  output kind; shared-stdlib reuse validates the archive and member formats as
  well as its existing exact-key/digest custody. WASM keeps its own relocatable
  module contract and cannot request a native artifact kind.
- **Native header admission**: `molt.native_artifact_header` is the single bounded
  file/bytes decoder for ELF, PE/COFF, thin Mach-O, and universal Mach-O. Native
  object output, linked binaries, release architecture checks, and loaded Python
  dependency custody consume its typed headers. Object emission requires one
  relocatable object; linked-image admission excludes relocatable objects and
  DLL/dylib kinds. ELF `ET_DYN` is explicitly a linked-image category, not proof
  that the artifact is executable rather than a shared library. The loader and
  consuming linker still own relocation, instruction, and dynamic ABI validity.
  Dependency custody retains its import/load-command semantic walks but does not
  reparse fixed headers or copy universal image slices.
  `molt.native_target_shape` describes known artifact identity encodings, not
  compiler support cells; backend/toolchain gates remain authoritative. Unknown
  shapes fail explicitly. File-header width and target pointer width are distinct
  (Mach-O arm64_32 has a 64-bit header and 32-bit pointers); header inspection
  proves only encoded identity, not complete pointer/float/calling-convention ABI.
  Meson CPU family/endianness projects the same shape facts. Exact Mach-O release
  admission checks the target subtype, rejecting generic-target substitution by
  arm64e/x86_64h or unrecognized subtype capability bits. Loaded-image custody
  may match an observed generic runtime family, but never chooses among ambiguous
  family slices or guesses the host architecture. Every universal slice's
  CPU/subtype agrees with its table row; extents, alignment, overlaps, reserved
  fields, duplicates, and nesting fail closed. A 4,096-slice resource-admission
  ceiling bounds parser allocation independently of declared file size. This is
  not a supported-architecture count. Optional smoke probes require matching
  host/target facts and never presume translation such as Rosetta is installed.

## Implementation Status Snapshot (2026-02-11)
- Implemented in this repo today:
  - `SimpleTIRGenerator` in `src/molt/frontend/__init__.py` emits a broad TIR
    op surface used by `molt build`/`molt run`.
- Instruction inventory:
  - `molt debug verify` checks every op the instruction categories list
    against the frontend op-kind registry generated from
    `runtime/molt-ir/src/tir/op_kinds.toml`.
- Partial / pending:
  - Dedicated HIR and LIR modules are not yet split into standalone compiler
    crates/modules in this tree.
  - The Python frontend performs language lowering and serializes wire IR once.
    Rust typed SSA passes own SCCP, GVN, exception-check elimination and DCE.
    The Python fixed-point optimizer and its policy/coverage controls are retired
    by HF-773 slice E. Async frame transport, traceback location publication and
    wire validation remain frontend responsibilities. Rust SCCP's loop-header
    refusal remains HF-821 work; the findings ledger bounds verified cells.
  - Frontend/lowering/backend now provide dedicated lanes for
    `CallIndirect`/`InvokeFFI`/`GuardTag`/`GuardLayout` and
    `IncRef`/`DecRef`/`Borrow`/`Release` plus conversion families
    (`Box`/`Unbox`/`Cast`/`Widen`); deterministic semantic-depth hardening and
    broader differential evidence are still in progress.
  - `molt debug verify` serializes each dedicated frontend lane through the
    real serializer and checks the lane it emits. It also checks that the
    native and WASM backends keep each lane, including the distinct call-site
    labels for `invoke_ffi_bridge`/`invoke_ffi_deopt` vs `call_func` and
    `call_indirect` vs `call_bind`.
  - LIR-level explicit RC ops (`IncRef`/`DecRef`/`Borrow`/`Release`) are
    specified here but not fully materialized as a separate lowering stage in
    the frontend emitter.
- Tracking policy:
  - Treat this spec as the contract; implementation status remains canonical in
    `docs/spec/STATUS.md`; open gaps live in the findings ledger.

## Example (TIR sketch)
```
func add(x: Int, y: Int) -> Int {
  block0(x0: Int, y0: Int):
    v0 = Add x0, y0
    Return v0
}
```
