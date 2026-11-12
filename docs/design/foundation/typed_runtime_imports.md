# Typed runtime imports in the native backends

Status: implemented (2026-11-12) after one Goldfish round; section 10 records the settled change
(B1) and where the implementation differs from the text below. Ledger row: HF-821 (family A, P0
item 2: "calls by `RuntimeFn`"). Lane: `foundation/typed-runtime-imports`.

## 1. Problem

Compiled code calls the runtime through about 2,800 entries. A closed type already names them:
`molt_ir::runtime_fn_generated::RuntimeFn`, generated from the `[[import]]` rows of
`runtime/molt-backend-wasm/src/wasm_abi_manifest.toml` by `tools/gen_wasm_abi.py`. The two native
backends do not use it as the key. They choose, declare and cache runtime functions by spelled
string. I measured this checkout (base `b658a6917d`):

| Site | Count |
|---|---|
| Cranelift `import_func_id_split` / `import_func_ref` calls (name + hand-written signature) | 327 call sites, 230 distinct names, 34 with a computed name |
| Cranelift typed calls (`import_runtime_func_ref`, `import_runtime_func_id_split`) | 26 call sites, all forwarding to the name path |
| `import_ids: BTreeMap<&'static str, (FuncId, ImportSignatureShape)>` plumbing | 1,300 mentions in about 40 files |
| `ImportSignatureShape` | allocates two `Vec<String>` of type names on every import lookup to compare signatures |
| LLVM import lookups by name (`ensure_runtime_*_fn`, `declare_fixed_runtime_function`, `get_execution_function`) | 167 non-test sites; 94 pass a literal |
| LLVM `FIXED_RUNTIME_IMPORTS` | a hand-written second registry: 130 name rows with arity, pointer signatures and LLVM attributes |
| LLVM `build_execution_call` | 98 call sites; each decides the calling convention by `ends_with` on the callee's symbol |
| `runtime_execution_abi(logical: &str)` | a 3,912-arm string `match`, run on every Cranelift declaration and every LLVM lookup, including lookups of compiled (non-runtime) functions |
| `format!("molt_{kind}")` then `runtime_boxed_abi(&symbol, n)` | 5 LLVM sites choose an entry by formatting an op-kind name |
| Ratchet `spelled_runtime_entries` (`tools/structural_audit_baseline.json`) | 892 for all compiler crates; 718 lines in the native crate alone |

Two gaps stop a mechanical rename:

- **G1. Non-word signatures have no identity.** `RuntimeFn` covers only rows whose wasm32
  signature is all `i64` (`runtime_fn_specs`, `tools/wasm_abi_gen/manifest.py:1095`). The native
  backends call 14 entries that pass a pointer or return an `i32` status or a pointer:
  `get_attr_object`, `get_attr_object_ic`, `get_attr_special`, `set_attr_object_ic`,
  `del_attr_object`, `load_method`, `load_super_method`, `dict_from_pairs`, `string_from_bytes`,
  `bytes_from_bytes`, `bigint_from_bytes`, `bytes_as_ptr`, `handle_resolve`, `sleep_register`.
  The manifest spells a native pointer as wasm32 `i32`, so an `i32` result is ambiguous: a status
  (`string_from_bytes`) or an address (`handle_resolve`, `bytes_as_ptr`).
- **G2. Entries with no `[[import]]` row.** The native backends call 20 runtime exports that have
  an execution ABI but no manifest row: `call_func_fast0`..`3`, `call_builtin`, `getitem_method`,
  `setitem_method`, `delitem_method`, `getitem_unchecked`, `list_getitem_int_fast`,
  `list_getitem_raw_idx`, `string_eq_fast`, `warn_stderr`, `bytes_from`, `float_as_f64` (returns
  `f64`), `debug_trace`, `profile_struct_field_store`, `set_attr_object`.

Why it matters. The owner rule is "never name-based lookup again" (2026-11-11). P0 item 2 of
`call_anatomy.md` needs a builder trait whose calls take `RuntimeFn`; it cannot exist while each
backend holds its own name registry. The LLVM fixed table also states facts the manifest does not
own: some `willreturn` attributes in it are unsound (section 7, R1).

## 2. Goals and non-goals

Goals:

1. Every runtime function either native backend calls is chosen by `RuntimeFn`. No backend code
   spells a runtime entry, formats one, or looks one up by string.
2. One generated record per entry gives its machine signature, execution context, linker symbol
   and effects. The backends derive declarations from it. No backend states a signature.
3. Cranelift caches `FuncId` and `FuncRef` in dense tables indexed by `RuntimeFn`. LLVM caches
   `FunctionValue` the same way.
4. The legacy name paths, the fixed table, `ImportSignatureShape`, and the tests that pin them are
   deleted in the same change.

Non-goals (each has a named follow-up or owner, section 9):

- The WASM backend's own import table. P0 item 3 moves WASM onto LLVM; its op lowerings are
  deleted, not migrated.
- Symbols compiled code *defines*: `molt_main`, `molt_isolate_*`, `molt_app_*`, data pools, and
  LLVM module-local helpers (`__molt_inline_retain`, `__molt_frame_push_N`,
  `__molt_global_slot_read_N`). They are not runtime imports.
- A TIR call whose target is data (an `s_value` symbol the frontend wrote). The backend decodes it
  to `RuntimeFn` once, at the op (boundary B1). Carrying the typed callee in TIR is a frontend
  change.
- The JIT symbol resolver and the linker. They resolve linker names by definition.

## 3. Prior art

- **rustc_codegen_llvm**: `declare_fn`/`get_fn` cache declarations per `Instance` (a typed key) in
  `CodegenCx::instances`; the symbol name is produced once by `symbol_name(instance)`. Lang items
  (`LangItem` enum) name runtime hooks, never strings.
- **Cranelift / Wasmtime**: `cranelift_module::FuncId` and `FuncRef` are dense entity indices;
  `SecondaryMap<K, V>` is the canonical dense side table. Wasmtime's `BuiltinFunctionIndex` names
  every libcall by a generated index with a generated signature (`foreach_builtin_function!`); the
  compiler holds an array of `Option<FuncRef>` indexed by it.
- **Go**: `cmd/compile/internal/typecheck/_builtin/runtime.go` is the one source; `mkbuiltin.go`
  generates the table, and `typecheck.LookupRuntime` returns a typed symbol.
- **Sparse sets** (Briggs and Torczon, 1993): O(1) clear by resetting only touched entries.

Molt history: `3cbf168b9` generated `RuntimeFn`; `22b0a56612` sourced the LLVM import audit from
it; `59f6b205e1` named frame-record entries by typed identity; `7f502e66dd` added the
`spelled_runtime_entries` ratchet. None migrated the import caches.

## 4. Mechanism

### 4.1 One record per entry (generator)

`RuntimeFnAbi` gains the machine signature and the execution facts. The generator writes them from
the manifest; the backends only read them.

- **Machine types.** A closed `RuntimeMachineType { Word, Address, I32, F64 }`. `Word` is a 64-bit
  integer on every target. `Address` is pointer width: `i32` on wasm32, the target pointer on
  native. The manifest's `static_type` vocabulary gains `"ptr"` and `"f64"`. Every current `i32`
  parameter in an `[[import]]` row is a Rust pointer (I read the extern signature of all 32
  non-word rows); those positions become `"ptr"` in one pass. Of the `i32` results, 9 are an
  `i32` status (`string_from_bytes`, `bytes_from_bytes`, `cbor`/`json`/`msgpack_parse_scalar`,
  `db_exec`, `db_query`, `ws_connect`, `ws_pair`) and stay `"i32"`; 3 are pointers
  (`bytes_as_ptr`, `handle_resolve`, `arena_new`) and become `"ptr"`. The WASM type renderer
  maps `"ptr"` to `I32`, so emitted WASM is unchanged.
- **Identity for every import.** `runtime_fn_specs` admits every `[[import]]` row with a runtime
  export, not only all-word rows. `RuntimeFn::is_word_signature()` keeps the old subset for
  consumers that need it: `CallRuntime` (late TIR) and boxed calls.
- **Execution facts.** `context: RuntimeExecutionContext` and `linker_symbol: &'static str` come
  from the same rows that generate `runtime_execution_abi`. `linker_symbol()` is the one place the
  physical name exists.
- **Rows for G2.** Each of the 20 entries gets an `[[import]]` row. An entry the wasm32 runtime
  does not export gets the existing `cfg` key (`cfg = 'not(target_arch = "wasm32")'`).
- **Effects.** `RuntimeEffects` gains `MayRaise`: may raise, never calls Python, terminates.
  The LLVM attributes in `FIXED_RUNTIME_IMPORTS` become manifest `effects` values where they are
  true (section 7, R1).
- **Check against the runtime.** The generator already scans runtime extern signatures
  (`generator_runtime_export_signature_rows`). Its normalization keeps pointers as `ptr` instead
  of folding them to `i32`. The generator refuses a `RuntimeFn` row whose machine types differ
  from the export it names. This replaces the fixed-table half of
  `tools/llvm_runtime_abi_audit.py`.
- **Op-kind selectors.** The five `format!("molt_{kind}")` sites read a generated
  `preserved_op_runtime_fn(kind: OpKind) -> Option<RuntimeFn>`, the same way
  `runtime_boxed_op_abi` and `numeric_runtime_fn` already work. `exception_observer_abi.rs` (also
  generated) returns `RuntimeFn`.

### 4.2 Cranelift

- `RuntimeImportIds`: one per object module, a dense `Box<[PackedOption<FuncId>]>` of
  `RuntimeFn::COUNT` slots (about 11 KiB). It declares an entry on first use from
  `runtime_signature(module, function)`.
- `NativeFunctionImports`: one per object module, reused across functions. A dense
  `Box<[PackedOption<FuncRef>]>` plus a `touched: Vec<RuntimeFn>` list; `begin_function` clears
  only the touched slots (sparse set). It keeps `refcount_admission`.
- One call path: `import_runtime_func_ref(module, ids, builder, refs, RuntimeFn) -> FuncRef`.
  `import_func_id_split`, `import_func_ref`, `ImportSignatureShape` and the name-taking
  `declare_runtime_execution_function` are deleted. The 327 sites drop their hand-written
  signature argument; the signature comes from the record.
- Helpers that return a name return `RuntimeFn`: `typed_slot_store_helper_name`,
  `index_fallback_import_name`, `LOADED_METHOD_CALLS`, `CallArgumentForm::runtime_constructor`,
  the GPU `call_targets` table, the set/frozenset constructor pairs.
- `declare_python_execution_function(logical)` splits: a function-object target that is a runtime
  entry carries `RuntimeFn` (decoded at B1); a compiled target calls
  `declare_compiled_execution_function`. The 3,912-arm string match leaves the declaration path.

### 4.3 LLVM

- `LlvmRuntimeImports<'ctx>` on `LlvmBackend`: `Box<[Cell<Option<FunctionValue<'ctx>>>]>` indexed
  by `RuntimeFn`. `Cell` because lowering holds `&self`; `FunctionValue` is `Copy`.
- `runtime_function(function: RuntimeFn) -> RuntimeCallee<'ctx>` declares on first use:
  `linker_symbol()`, the type from `runtime_function_type` (machine types, plus the context word
  when `Attached`), `nounwind`, and the attributes `effects()` implies. Nothing else declares a
  runtime function.
- `RuntimeCallee<'ctx> { function, context }` and a compiled callee type replace the symbol
  suffix test in `build_execution_call`: the calling convention comes from the typed fact.
- Deleted: `runtime_imports/fixed.rs` (`FIXED_RUNTIME_IMPORTS`), `abi_facts.rs`
  (`runtime_import_return_abi`, `is_runtime_import_abi`), `declare_conservative_runtime_function`,
  `declare_fixed_runtime_function`, `ensure_runtime_decl`, `ensure_runtime_i64_fn`,
  `ensure_runtime_void_fn`, `RuntimeReturnAbi`, and `get_execution_function` for runtime entries.
  Compiled-function lookups use `get_compiled_function(logical)`, which formats
  `compiled_execution_symbol` without consulting the runtime table.
- `runtime_callable_symbols: BTreeSet<String>` (the linked profile's callable set) is decoded once
  where the environment is read, into a `RuntimeFnSet` bitset. The three `contains(&symbol)`
  probes become bit tests.
- The `record_removed_runtime_delegate` diagnostics stop spelling runtime names that never existed
  (`molt_call_0`, `molt_scf_for`, `molt_yield`, ...); they name the `OpCode`.

### 4.4 Boundary B1

A TIR call names its target by symbol string, which is data from the frontend. Where a backend
reads such an op, it decodes the symbol once with `RuntimeFn::from_symbol` (a binary search over
the generated table) and passes `RuntimeFn` from then on. This is the only string-to-identity step
left in the native backends. Serde already decodes `RuntimeFn` this way for TIR's `runtime` field.

## 5. Invariants

- **I1. One identity.** A runtime entry that compiled code calls has exactly one `RuntimeFn`. The
  generator refuses a duplicate symbol or a call site with no row. Check: generator and the
  ratchet (I6).
- **I2. One signature.** The machine signature of a runtime entry exists only in its generated
  record. No backend file states parameter or result types for a runtime entry. Check: deleted
  APIs have no callers (compile error); rg finds no `import_func_id_split`, `FixedRuntimeSignature`
  or `ImportSignatureShape`.
- **I3. Signature matches the runtime.** Every `RuntimeFn` record agrees with its runtime export's
  Rust signature, by class (`Word`, `Address`, `I32`, `F64`, void). Check: the generator refuses a
  mismatch.
- **I4. One linker name.** `RuntimeFn::linker_symbol()` is the only producer of a runtime entry's
  physical symbol in compiler code. Check: rg finds no `execution_symbol(`, no
  `runtime_execution_abi(` in the native crate.
- **I5. Typed caches.** Every runtime import cache in the native crate is indexed by `RuntimeFn`.
  Check: rg finds no `BTreeMap<&'static str` or `HashMap<&str` keyed import table.
- **I6. Ratchet.** `spelled_runtime_entries` falls by at least the native crate's runtime-import
  literals, and the baseline records the new value. The remaining native literals are symbols the
  compiler defines (section 2), and the design lists them.
- **I7. Effects are sound.** LLVM receives `willreturn` only for `MayRaise`, `NothrowRead` or
  `NothrowWrite` entries, and `memory(read)` only for `NothrowRead`. A row gains a non-impure
  value only when its runtime body cannot call Python code.
- **I8. Late TIR stays word-only.** The TIR verifier refuses `CallRuntime` of an entry that is not
  `is_word_signature()`.

## 6. Skeleton

Generated (`runtime/molt-ir/src/runtime_fn_generated.rs`, by `tools/gen_wasm_abi.py`):

```rust
/// The machine class of one runtime parameter or result (I2, I3). `Word` is 64 bits on every
/// target; `Address` is pointer width (wasm32 `i32`, native pointer).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeMachineType { Word, Address, I32, F64 }

/// What a runtime entry may do (I7). `MayRaise`: may raise, never calls Python, terminates.
pub enum RuntimeEffects { Impure, MayRaise, NothrowRead, NothrowWrite }

pub struct RuntimeFnAbi {
    pub symbol: &'static str,              // logical export name: serde, diagnostics
    pub linker_symbol: &'static str,       // I4
    pub context: RuntimeExecutionContext,
    pub params: &'static [RuntimeMachineType],
    pub result_type: Option<RuntimeMachineType>,
    pub result: RuntimeReturnContract,
    pub form: RuntimeCallForm,
    pub effects: RuntimeEffects,
}

impl RuntimeFn {
    pub const COUNT: usize = /* rows */;
    /// Dense index for side tables (I5).
    pub const fn index(self) -> usize { self as usize }
    pub const fn linker_symbol(self) -> &'static str { todo!() }
    pub const fn context(self) -> RuntimeExecutionContext { todo!() }
    pub const fn param_types(self) -> &'static [RuntimeMachineType] { todo!() }
    pub const fn result_type(self) -> Option<RuntimeMachineType> { todo!() }
    /// All parameters `Word`, result `Word` or void: the set `CallRuntime` may name (I8).
    pub const fn is_word_signature(self) -> bool { todo!() }
}

/// The runtime entry a preserved op calls on boxed operands; replaces `format!("molt_{kind}")`.
pub const fn preserved_op_runtime_fn(kind: crate::OpKind) -> Option<RuntimeFn> { todo!() }
```

Native crate (`runtime/molt-backend-native/src/runtime_import_abi.rs` owns the shared parts):

```rust
/// The Cranelift signature of a runtime entry, the context word first when `Attached` (I2).
/// Requires: nothing. Ensures: params.len() == function.param_types().len() + attached as usize.
pub(crate) fn runtime_signature(module: &dyn Module, function: RuntimeFn) -> Signature { todo!() }

/// Module-wide FuncId per runtime entry (I5). Declares with `linker_symbol()` on first use.
pub(crate) struct RuntimeImportIds { ids: Box<[PackedOption<FuncId>]> }
impl RuntimeImportIds {
    pub(crate) fn new() -> Self { todo!() }
    /// Requires: `module` is the module this table was made for.
    /// Ensures: the same `FuncId` for the same `function` for the module's life.
    /// Raises: panics only if the module refuses the declaration (a symbol clash: a bug).
    pub(crate) fn func_id(&mut self, module: &mut dyn Module, function: RuntimeFn) -> FuncId { todo!() }
}

/// Per-function FuncRef per runtime entry: a sparse set reused across functions (I5).
pub(crate) struct NativeFunctionImports {
    refs: Box<[PackedOption<FuncRef>]>,
    touched: Vec<RuntimeFn>,
    refcount_admission: Option<Value>,
}
impl NativeFunctionImports {
    /// Ensures: every slot is empty; cost O(touched).
    pub(crate) fn begin_function(&mut self) { todo!() }
}

/// The one Cranelift entry point for a runtime callee.
pub(crate) fn import_runtime_func_ref(
    module: &mut dyn Module, ids: &mut RuntimeImportIds, builder: &mut FunctionBuilder<'_>,
    refs: &mut NativeFunctionImports, function: RuntimeFn,
) -> FuncRef { todo!() }

/// LLVM: a declared runtime function and its calling convention.
#[derive(Clone, Copy)]
pub(crate) struct RuntimeCallee<'ctx> { pub function: FunctionValue<'ctx>, pub context: RuntimeExecutionContext }

pub(crate) struct LlvmRuntimeImports<'ctx> { slots: Box<[Cell<Option<FunctionValue<'ctx>>>]> }
impl<'ctx> LlvmRuntimeImports<'ctx> {
    /// Requires: `module` is the backend's module. Ensures: one declaration per entry, with
    /// `nounwind` and the attributes `function.effects()` implies (I7), and no other.
    pub(crate) fn get(&self, context: &'ctx Context, module: &Module<'ctx>, function: RuntimeFn) -> RuntimeCallee<'ctx> { todo!() }
}

/// The set of runtime entries the linked runtime profile defines, decoded once (B1).
pub(crate) struct RuntimeFnSet { bits: Box<[u64]> }
```

Facts that stay strings, and why: `RuntimeFnAbi::symbol` (the logical name, for serde of TIR and
for diagnostics) and `RuntimeFnAbi::linker_symbol` (an object-file symbol; the linker and
`Module::declare_function` take a string by definition). Both are generated constants, read only
at declaration and in messages. Every other fact above is a closed type or a dense index.

## 7. Risks and limits

- **R1. LLVM attributes change.** Today `FIXED_RUNTIME_IMPORTS` marks 39 entries `willreturn`
  and 5 of them `memory(read)`; the manifest gives every one of them the impure floor. Some are unsound: `iter_next` calls `__next__`, `print_obj` calls `__str__`,
  `dec_ref_obj` runs `__del__`, and Python code may loop forever, so `willreturn` there permits
  LLVM to assume termination that the program does not have. The migration keeps `willreturn` only
  where the manifest's `effects` proves it (I7). The rows I expect to keep it (`int_from_i64`,
  `alloc`, `exception_*`, `callargs_*`, `func_new`, `code_new`, `frame_home_*`, `slice_new`,
  `set_new`, `*_from_bytes`) get `MayRaise` after I read each body; `inc_ref_obj` gets
  `NothrowWrite`; `is_truthy_int`/`_bool` (and `_nogil`) and `exception_pending` get `NothrowRead`
  unless they are boxed calls, which the generator keeps impure. Mitigation: measure (section 8).
- **R2. New `[[import]]` rows reach WASM.** A row adds a WASM import. The WASM link strips unused
  imports (`strip_import_rule`), so module size should not change. An export the wasm32 runtime
  lacks gets `cfg`. Check: one WASM differential and the WASM size of one program before and after.
- **R3. `static_type` vocabulary.** `"ptr"` and `"f64"` touch every consumer of `static_type`
  in `tools/wasm_abi_gen` (about 100 mentions). The WASM renderer maps them to `I32`/`F64`; the
  generated WASM tables must be byte-identical. Check: `tools/generators.py check` diff shows only
  the intended files change; the WASM generated tables do not.
- **R4. Widening `RuntimeFn`.** Consumers that assumed every `RuntimeFn` is all-word
  (`CallRuntime` verification, boxed selection) must use `is_word_signature()`. I8 enforces it.
- **R5. Memory.** Dense tables cost about 11 KiB (FuncId) plus 11 KiB (FuncRef) per object
  module, and 22 KiB per LLVM module, against today's `BTreeMap` nodes and per-lookup string
  allocations. Compile-time CPU should fall. Check: compiler instructions retired and peak RSS on
  one build (section 8).
- **R6. Scale.** About 500 call sites across 60 files change in one commit: an indivisible API
  change (AGENTS.md). Mitigation: change the signatures first so the compiler finds every caller;
  no adapter or alias.
- **Limit.** Boundary B1 keeps one `from_symbol` decode per TIR call op whose target is a runtime
  entry. Removing it needs the frontend to emit typed callees (section 9).

## 8. Test and measurement plan

Each check names the defect it catches.

1. **Generator** (`tests/test_gen_wasm_abi.py`): every `[[import]]` row with a runtime export has
   a `RuntimeFn`; a row whose `ptr`/`i32`/`f64` class differs from its Rust export is refused (a
   wrong class would truncate an address at the call); `"ptr"` renders as WASM `I32`. Defect: a
   wrong or missing signature class.
2. **Rust, native crate** (rewrite `llvm_backend/runtime_imports/tests.rs` and the Cranelift
   import tests): for every `RuntimeFn`, the LLVM declaration's type equals
   `runtime_function_type`, its symbol is `linker_symbol()`, its attributes follow `effects()`; a
   second request returns the same value; Cranelift's signature has the context word exactly when
   `Attached`; `begin_function` leaves no stale `FuncRef`. Defect: a cache that returns another
   function's reference, a missing context word.
3. **TIR verifier** (`molt-ir` tests): `CallRuntime` of an `Address`-signature entry is refused (I8).
4. **Differential**, native (Cranelift) and LLVM, against CPython 3.12, chosen to cover each
   signature class and call family that changes:
   `tests/differential/basic/call_arity_trampoline.py`, `call_result_ownership.py`,
   `call_indirect_dynamic_callable.py`, `args_kwargs.py` (call and callargs entries),
   `attribute_lookup_order.py`, `attr_hooks.py`, `attribute_set_delete_order.py`
   (`get_attr_object_ic`, `set_attr_object_ic`, `del_attr_object`, `load_method`: `Address`
   parameters), `bigint_ops.py` (`bigint_from_bytes`), `dict_constructor_starargs.py`
   (`dict_from_pairs`), `exception_chaining_basic.py` (exception entries, `MayRaise`),
   `call_task_trampoline.py` (`sleep_register`, task entries). Run through
   `tests/molt_diff.py --target native,llvm`.
5. **Proof cells**: `tools/consumer_gate.py run -- <paths>`; `llvm.test.lowering`; `cargo test -p
   molt-backend-native --features native-backend --lib`; `tools/check_changed_crate_tests.py`;
   `tools/molt_dev.py land --precheck-only`.
6. **Measurement** (macOS arm64, interleaved A/B, base worktree against lane):
   - generated code: `bench_fib` and `bench_method_defaults`, LLVM release, instructions retired;
     acceptance: no change beyond noise, or an explained change from R1;
   - compile time: `molt build` of one call-heavy program (Cranelift and LLVM), compiler
     instructions retired and peak RSS with `/usr/bin/time -l`; expected to fall (no
     `ImportSignatureShape` allocation, no 3,912-arm match per declaration).

## 9. Alternatives considered

- **A1. Leave Cranelift for its move to final TIR.** `call_anatomy.md` plans to delete
  Cranelift's op lowering when it moves to the shared walker. Rejected: that move has no date,
  and the walker's builder trait needs `calls by RuntimeFn` with a generated signature first. The
  Cranelift migration here is mostly deleting the per-site signature argument.
- **A2. A second closed type for non-word entries** (for example `RuntimeAddressFn`). Rejected:
  two identities for one concept, two caches, two generators' outputs. One `RuntimeFn` with a
  typed signature and an `is_word_signature()` predicate keeps one authority.
- **A3. Key the caches by `linker_symbol()` (a `&'static str` produced from the type).**
  Rejected: still a string hash and compare per lookup; the dense index is cheaper and makes I5
  checkable.
- **A4. Look up LLVM functions through LLVM's own symbol table (`module.get_function`) by the
  typed name.** Rejected for the same reason: a string hash per call site. LLVM's table stays the
  linker authority; the backend does not query it.
- **A5. Derive LLVM attributes from today's fixed table into a new LLVM-only manifest column.**
  Rejected: the facts (may raise, calls Python, writes memory) are target-neutral and the TIR
  optimizer reads the same `RuntimeEffects`. A backend-specific column would be a second authority.
- **A6. Classify native pointers by reading the Rust source at build time only** (keep `i32` in
  the manifest). Rejected: the manifest is the authority; the source scan is the check, not the
  source of the fact.

Follow-ups this design does not do (the integrator assigns them):

- F1. The frontend emits a typed callee for TIR calls to runtime entries; boundary B1 is deleted.
- F2. LLVM module-local helpers (`FrameHelper`, global-slot readers, inline refcount) get typed
  keys instead of formatted names.
- F3. The WASM backend's import table, if WASM does not move to LLVM.

## 10. As implemented (2026-11-12)

- **B1, settled in review.** No backend file decodes a runtime target from a string
  (`rg from_symbol runtime/molt-backend-native/src` is empty). The SimpleIR-to-TIR lift decodes once
  (`molt_ir::tir::call_targets::ingest_runtime_target`, called from the SSA op translation) into
  `TirOp.runtime` for `call` (role `external_or_runtime`), `builtin_func`, `call_async` (its poll
  function) and the GPU intrinsics; `call_internal` never gains one. The TIR-to-SimpleIR projection
  copies the field to the new `OpIR.runtime`, so Cranelift reads the same fact. The two passes that
  create runtime calls after the lift (`global_slot_sites`, drop-insertion activation) set it
  themselves. The verifier admits `runtime` only on an op whose own target names that entry. The app
  resolver manifest decodes its runtime callables in `molt-tir`; the linked profile's callable set
  is decoded once into `RuntimeFnSet` in `molt-ir`.
- **Coverage of the signature check (I3).** All 2,839 `RuntimeFn` rows have a parsed runtime export
  and pass the class check, so the generator check covers every row the deleted
  `tools/llvm_runtime_abi_audit.py` checked (its fixed table and its boxed facts).
  `tools/audit_op_kinds.py --check` now reports the generator's refusal.
- **R3.** The `ptr` vocabulary change regenerated every output byte-identically (304 outputs, no
  diff). G2 has 18 entries, not the 20 section 1 counted; their 18 new `[[import]]` rows (one new
  static type, `(i64) -> f64`, for `float_as_f64`) add rows to the WASM import tables only.
- **Cranelift tables.** `NativeFunctionImports` is a dense table allocated per function (one
  `PackedOption<FuncRef>` per `RuntimeFn`, about 11 KiB), not the sparse set the skeleton showed;
  the reuse saves nothing measurable against the per-function Cranelift context.
- **LLVM.** `runtime_imports::runtime_function` is the one declaration path (lookup by the
  generated linker symbol, else declare); `LlvmRuntimeImports` caches it by dense index.
  `get_execution_function` became `get_compiled_function`: it never consults the runtime table.
- **Measured (R1).** LLVM release builds, macOS arm64, `/usr/bin/time -l` instructions retired,
  three interleaved runs each, base `8346f34eb0` (main) against the lane merged onto it; median
  base -> lane: `bench_fib` 1,505,171,199 -> 1,504,572,669 (-0.04%); `bench_method_defaults`
  (2,000,000) 6,223,744,878 -> 6,205,546,401 (-0.29%); `bench_calls` 1,305,890,348 ->
  1,306,326,272 (+0.03%); `spectral_norm` (400) 19,783,244,311 -> 19,785,977,200 (+0.01%). Every
  delta is inside the run-to-run spread; outputs are identical. Dropping `willreturn` from
  impure entries costs nothing measurable here.
- **Not done here (follow-ups).** The calling convention is still chosen by the attached-context
  symbol suffix in `build_execution_call` (LLVM) and `complete_attached_calls` (Cranelift); the
  typed `RuntimeCallee` of section 4.3 is not implemented. Task construction still selects a
  runtime poll key with `runtime_poll_native_key(name)`. Both are compile-time string tests on
  names the compiler produced; they are the next typed-identity sites in these files.

## GoldfishContext

Consumers this change touches (producer, generated projections, readers, tests, lists):

- Authority and generator: `runtime/molt-backend-wasm/src/wasm_abi_manifest.toml`;
  `tools/gen_wasm_abi.py`; `tools/wasm_abi_gen/manifest.py` (`runtime_fn_specs`,
  `RUNTIME_IMPORT_EFFECTS`, `_rust_export_signatures` normalization, static-type rendering);
  `tools/wasm_abi_gen/execution_context.py`; `tools/wasm_abi_gen/render_python.py`.
- Generated outputs (regenerate with `tools/generators.py write`):
  `runtime/molt-ir/src/runtime_fn_generated.rs`, `runtime/molt-ir/src/runtime_callable_abi_generated.rs`,
  `runtime/molt-backend-native/src/exception_observer_abi.rs`, the WASM tables under
  `runtime/molt-backend-wasm/src/wasm_abi_generated/` (must stay byte-identical, R3), and
  `src/molt/_wasm_abi_generated.py`.
- IR readers: `runtime/molt-ir/src/tir/verify.rs` (I8), `runtime/molt-ir/src/tir/op_semantics.rs`
  (`RuntimeEffects` match), `runtime/molt-ir/src/call_argument_form.rs`,
  `runtime/molt-ir/src/tir/call_targets.rs`, `runtime/molt-ir/src/runtime_call_tests.rs`.
- Cranelift: `runtime/molt-backend-native/src/native_backend/mod.rs`,
  `native_backend/simple_backend/imports.rs`, `simple_backend/refcount.rs`,
  `simple_backend/trampolines.rs`, `native_backend/function_compiler.rs`,
  `function_compiler/preanalysis.rs`, `function_compiler/shared.rs`,
  `function_compiler/scalar_carriers.rs`, every `function_compiler/fc/*.rs` that imports a
  runtime function (about 35 files), and their tests (`function_compiler/tests/*`,
  `simple_backend/tests/compile_pipeline.rs`).
- LLVM: `runtime/molt-backend-native/src/llvm_backend/mod.rs`, `execution_abi.rs`,
  `runtime_imports.rs` and `runtime_imports/*` (fixed, abi_facts, declarations, attributes,
  tests), `lowering.rs`, `lowering/*.rs` (value_materialization, op_dispatch, call_lowering,
  object_ops, container_ops, constant_ops, state_machine_ops, runtime_helpers, refcount,
  numeric_ops, preserved_ops/*), `app_resolver.rs`, `module_registry.rs`, and
  `lowering/tests/*`.
- Shared native: `runtime/molt-backend-native/src/runtime_import_abi.rs` (new home of the typed
  tables; `RuntimeReturnAbi` deleted).
- Test fakes of the boundary: hand-written runtime stubs in
  `function_compiler/tests/call_operand_custody.rs` and `native_callable_dispatch.rs` define the
  physical symbols; they keep working because linker names do not change.
- Audits and ratchets: `tools/llvm_runtime_abi_audit.py` and `tests/test_llvm_runtime_abi_audit.py`
  (fixed-table half deleted, boxed half folded into the generator check; the tool, its test,
  its `tools/proof_plan.toml` command and globs are deleted if nothing else remains);
  `tools/structural_audit_baseline.json` (`spelled_runtime_entries`, I6).
- Proof plan: `tools/proof_plan.toml` cells `llvm.test.lowering`, the native lib tests, and the
  audit command above.
- Docs: `docs/design/foundation/call_anatomy.md` (P0 item 2 status line),
  `docs/agent/V1_HANDOFF_FINDINGS.md` (HF-821 evidence), `docs/INDEX.md`.
- Environment readers: none new. `MOLT_RUNTIME_CALLABLE_SYMBOLS` keeps its one reader
  (`RuntimeCallableSymbols::capture`); its value is decoded into `RuntimeFnSet` there.
