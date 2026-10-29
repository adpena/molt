# Molt Import Bedrock — The Frozen Module Layer

**Status:** Registry/state implementation with single-dictionary source cutover; runtime qualification remains open.
**Current storage contract:** [single module dictionary cutover](import_bedrock_pr2_sys_modules_view_cutover.md). The 2026-07 historical incident analysis and proposal sections are retained as provenance, not a current source census or qualification claim. The current storage contract supersedes older table-view, overflow, journal and warm-import cost proposals; the pinned parity obligations remain applicable.
**Scope:** bootstrap, module identity, import, module init, sys.modules,
extension init, and every dispatch/callable/poll table projection of the module
graph, across native / WASM / split-runtime targets.
**Date:** 2026-07-02.
**Verification basis:** live tree at `C:\Users\adpen\OneDrive\Documents\molt`
(main @ 781a18202), CPython 3.12 docs/source, Go spec/runtime, GraalVM
native-image docs, MicroPython source, Codon docs. Citations inline.

---

## 0. Executive summary

Molt's compiled module graph is admitted through one generated `ModuleRegistry`.
`ModuleTable` owns the ordinary interpreter module dictionary plus separate lazy
initialization metadata. `sys.modules` exposes the same dictionary from bootstrap
onward. Completed rows retain no duplicate module object; ordinary initializer
slots and explicit execution contexts retain their own live namespaces. Interpreter sys and builtins
roles have explicit lifetimes independent of mutable public module entries.

`molt_module_ensure(ModuleId)` owns compiled initialization, recursive-import
custody and foreign-thread waits. Normal native/WASM/C-API imports consult the
same callback-safe dictionary before initialization. Ordinary public misses use
the ordered finder protocol; the catalog is one provider. Only explicit internal
bootstrap/initializer capabilities resolve directly through the sorted registry.
No zero-hash/two-load warm-import or timing claim follows from the source design.

The canonical cutover contract linked above supersedes the earlier custom dict
backing-tag proposal. It follows CPython's interpreter-owned ordinary module dict
and avoids changes to the dict/iterator/GC/C-API representation family. The registry
schema, layout generation, extension-phase behavior and parity obligations below
continue to apply. Runtime qualification remains separate from implementation.

---

## 1. Historical source aperture (2026-07-02)

This pre-cutover source inventory explains the original design. Consult the current
storage contract and live source for implemented behavior and qualification.

### 1.1 Module identity is a runtime string, everywhere

- `molt_module_import(name_bits)` allocates a *fresh name string* per call,
  takes the module-cache mutex up to four times, probes `sys.modules`, calls
  the app-owned `molt_isolate_import`, then re-probes and re-synchronizes
  (`runtime/molt-runtime/src/builtins/modules.rs:830-1035`).
- `molt_isolate_import` is a **generated if/else chain of `string_eq` ops over
  module names** (`src/molt/cli/backend_ir.py:409-494`,
  `_build_isolate_import_ops`). O(modules) string comparisons per cold import;
  a codegen refactor that reorders or drops a chain arm is a silent behavioral
  change. [Incident 11]
- Because every API takes `name_bits: u64` that *should* be a string, any
  frontend/custody regression that routes a non-string value into an import
  site surfaces as `TypeError: module name must be str` from `<molt-builtin>`
  on a *plain `import guardpkg`* — the `trace_bad_module_name_arg` paths at
  `modules.rs:753-827` exist purely to debug this recurring class.
  [Incident 1]

### 1.2 Module state has multiple homes with sync machinery

- Primary-ish store: `Mutex<HashMap<String, u64>>` reached through
  `crate::builtins::exceptions::internals::module_cache` — the module store
  literally lives in the *exceptions* module (`exceptions.rs:303-305`).
- Second home: the `sys.modules` dict (a plain dict stored under `"modules"`
  in the sys module dict, auto-vivified empty by `sys_modules_dict_bits`,
  `modules.rs:1879-1915`).
- Sync machinery: `molt_module_cache_set` mirrors every insert into
  `sys.modules`, replays the **entire cache** into `sys.modules` when `sys`
  itself registers, and carries a first-init-wins carve-out (added because WASM
  linked binaries emitted *duplicate init sequences* for the same module — a
  compiler bug patched in the runtime) (`modules.rs:1954-2117`). [Incident 3]
- Python-side `sys.modules` mutations (`sys.modules['x'] = m`,
  `del sys.modules['x']`) go through plain dict ops and are **invisible** to
  the Rust map until some later import happens to re-reconcile. [Incident 3]
- `molt_sys_modules()` — the intrinsic backing the `sys` module's `modules`
  attribute — returns a **freshly allocated empty dict**
  (`runtime/molt-runtime/src/builtins/sys_ext.rs:1162-1172`), with a comment
  explaining that "the real sys.modules dict lives on the Python side and is
  synchronised through molt_module_import." [Incident 4]

### 1.3 Init-exactly-once is owned in N places

- Each generated static-native init function carries its own cache-probe
  preamble (`module_cache_get` → `is None` → `if`) — added by commit
  781a18202 after an alias init bypassed the guard and re-ran a static
  extension `PyInit`, tripping "cannot load module more than once per process"
  (`src/molt/cli/backend_ir.py:686-840`). [Incident 2]
- The eager path (`molt_main` module ops), the lazy path (isolate dispatcher),
  the alias path (alias init calls provider init), and the capsule/attr-export
  path (`_append_static_native_module_attr_export_ops` calls provider inits
  again) each independently re-enter init functions and each rely on that
  per-function preamble being present and correct.

### 1.4 The name-resolution family has three near-twins

`molt_module_get_attr` (miss → `AttributeError`), `molt_module_get_global`
(miss → builtins fallback → `NameError`), `molt_module_import_from` (miss →
PEP-562 lookup → `sys.modules["pkg.child"]` recovery → `ImportError`) are
three ~100-line functions sharing `module_attr_lookup` but hand-duplicating
the guard/trace/type-check scaffolding (`modules.rs:2261-2740`). A frontend
lowering that picks the wrong op for a site changes the *exception class* user
code observes. [Incident 10]

### 1.5 The WASM projection is parallel-computed, not derived

- The callable-table layout is computed in the backend
  (`runtime/molt-backend-wasm/src/wasm/module_abi/callable_table/layout.rs`),
  while the browser manifest's `wasm_table_base` travels separately through
  `backend_execution.py` → manifest JSON → `wasm/loader_bridge.js:302-320`,
  which can only *detect* drift at page-load time:
  `"manifest wasm_table_base 4135 is above binary table base 2475"`.
  [Incident 5]
- `poll_table.rs:75` panics `"missing poll import for {name}"` when the
  generated `POLL_TABLE_IMPORTS` spec references an import the backend didn't
  declare — two hand-synchronized sets. [Incident 6]
- The ABI manifest (`wasm_abi_manifest.toml`) feeds ~15 generated files
  (`wasm_abi_generated/*.rs`, `src/molt/_wasm_abi_generated.py`,
  `wasm/wasm_abi_generated.json`); staleness between authority and consumers
  surfaces as `"runtime import 'molt_PyArg_ParseTuple' missing from WASM ABI
  manifest"` mid-build. [Incident 7]

### 1.6 The extension lane grew its own paths

- `PySys_GetObject` re-entered the full import machinery from extension init
  until it was made cache-first (`cpython_abi_hooks.rs:428-458`). [Incident 8]
- `PyImport_ImportModule("math")` from C extension init was invisible to the
  AOT graph until a C-source scan + custody channel was bolted on
  (`external_native.py:1264`, `import_scan_mode="module_init"`). [Incident 9]

### 1.7 Why it churns

Every fix above is a *local* fix to a *global* invariant. The invariants
("one module store," "init exactly once," "manifest matches binary") have no
single owner, so every refactor near the layer re-breaks one of them, and
every fix adds another reconciliation pass, trace env-var, or preamble. The
layer needs the Go/GraalVM treatment: compute everything computable at compile
time, leave a minimal, provably-coherent state machine at runtime, and freeze
it.

---

## 2. The model

### 2.1 Definitions

Let `G = (M, E)` be the compile-time module graph: `M` = the admitted module
closure (already the "binary image closure plan" authority in
`module_graph.py` / `import_system_contract.md` §3.3), `E` = import edges
(source imports + package-parent edges + extension dynamic-import edges from
the C-source scan + alias edges).

- **Registry**: `sort(M)` by canonical dotted name → bijection
  `id: M → [0, N)`. `ModuleId = u32`. The sort makes ids deterministic and
  reproducible-build-stable for a given closure.
- **Init relation**: `ensure: [0,N) → module object`, defined by the state
  machine in §4.3. The *semantic* init order is the depth-first order induced
  by executing import statements at their source positions — identical to
  CPython and to Go's `doInit` walk. A compile-time topological order over `E`
  exists for the acyclic subgraph and is used only as a *verified
  optimization* (§8.3), never as the semantic authority, because Python
  permits cycles that a flattened order cannot express.
- **Store**: one interpreter-owned ordinary dictionary, exposed as the same
  `sys.modules` object. Compiled initialization state is separate; only live
  initializer/loader transactions retain namespace bits. Dynamic names need no
  second map or overflow representation.

### 2.2 The one-transaction principle, made precise

`docs/agent/AGENTS.full.md` already demands "one import transaction per
module-state transition." The design gives that sentence a concrete type:

> Every transition of `Table[i].state`, every write of `Table[i].bits`, every
> parent-attribute publication, and every alias co-publication happens inside
> exactly one function, `molt_module_ensure` (plus the two dict-view mutation
> entry points for `Replaced`/`Tombstone`, §4.4, which are part of the same
> compiled unit and hold the same lock discipline).

The earlier proposal that every public import could become a direct `ensure`
call is superseded. Ordinary source statements, `__import__`, importlib/fromlist,
C-API and isolate imports share public cache/finder admission. Only a selected
compiled loader or an explicit internal bootstrap/entry capability dispatches
an admitted body. Explicit runpy/loader execution retains its separate private
namespace role; these roles reuse the same body dispatcher without bypassing
public finder semantics.

---

## 3. Compile-time artifacts (the single authority)

The current wire authority is schema 5, magic `MOLTMOD5`, with required
stdlib-provider provenance (`0x10`) alongside file-location provenance (`0x08`).
The metadata producer, frontend bootstrap role, worker/cache identity and catalog
consume the same source-role fact. This does not encode a compiler filesystem
root as target-runtime authority. The physical layout has a 56-byte header and
48-byte rows. Namespace-parent rows carry an ordered span of length-prefixed
UTF-8 directory portions in a separate blob table. They have the package flag,
no initializer, no file location and an empty serialized origin. Other kinds
cannot carry portions. The producer, JSON/digest projections and runtime parser
validate this boundary. Namespace identity comes only from the row kind.
The compiled finder returns a loader-less spec with origin `None` and the
existing machinery `_NamespacePath(name, portions, PathFinder._get_spec)`;
module-from-spec binds `NamespaceLoader` and shares the exact locations object
with `__path__`. Namespace packages contribute no generated source, scan, cache
entry or compiled body. Required typed entry field `has_location`
projects into flag `0x08` and participates in the same JSON/digest/blob
projections. Only actual source-file producers and file-location factories
admit this fact; a descriptive origin or generated native source-kind row
does not. Alias lookups use the terminal provider's flag. Runtime admission
rejects older schema/magic, so artifact mixing cannot silently default the
new fact. Native/LLVM/WASM emit the authority's opaque blob and relocations;
they do not parse origin text or derive another file-location policy.

One projection checker (`tools/check_module_registry.py`, same family as
`gen_wasm_abi.py`/`gen_op_kinds.py`) consumes the closure plan that
`module_graph.py` already produces and emits **one logical artifact** with
per-consumer projections:

```
registry.rows[i] = {
  id:            u32            # dense, sorted-name order
  name:          &'static str   # canonical dotted name
  name_hash:     u64            # for the PHF
  parent:        ModuleId|NONE  # package parent ("a" for "a.b")
  leaf_attr:     &'static str   # "b" for "a.b" (parent-binding attr)
  kind:          Source | Extension { init_symbol, phase } |
                 Alias { target: ModuleId } | NamespaceParent { portions } |
                 RuntimeBuiltin (sys, builtins)
  init:          table index into MODULE_INIT_TABLE (fn() published per target)
  deps:          &'static [ModuleId]   # E-edges incl. C-scan dynamic deps
  flags:         eager | lazy | reinit_policy (source=reexec, ext=resurrect)
}
```

Projections, all emitted by the same generator run and stamped with the same
`registry_digest`:

Every admitted `RuntimeBuiltin` row owns an executable initializer, even when
user code has no explicit import of it. The canonical module-kind authority
supplies this root to native, LLVM and WASM retention; declaration and JSON
admission reject a runtime builtin without its initializer/body flag. Other
source rows still require the admitted import closure or generated entry roots.
Core, spawn, package-parent and explicit static-import discovery preserve the
dependency imports from their existing scan receipts. Canonical core/provider
inventory seeds also retain callable initializer lanes; they are kept separate
from source-discovered imports and do not introduce eager module execution.
Foreign builtin-provider execution remains gated by the app symbol resolver.

| Projection | Consumer | Replaces |
|---|---|---|
| `module_registry_generated.rs` (const tables + PHF) | molt-runtime | `HashMap<String,u64>` keying, `known_absent_module` scans |
| `MODULE_INIT_TABLE` (native fn-ptr array / wasm table segment) | backends | `_build_isolate_import_ops` string_eq chain, per-init preambles |
| `module_registry.json` | CLI diagnostics, browser manifest | ad-hoc manifest fields |
| wasm callable/poll/dispatch slot assignments | molt-backend-wasm `module_abi/*` | hand-synced slot/import pairs |
| `INIT_ORDER` (topo order of the provably-acyclic eager set) | molt_main prelude | `module_code_ops` ordering logic |

**Digest discipline (kills incident 7 as a class):** every generated file
embeds `registry_digest` (and the wasm files additionally embed the existing
ABI-manifest digest). The runtime crate `const`-asserts that the digest baked
into `MODULE_INIT_TABLE` equals the digest in `module_registry_generated.rs`;
the CLI asserts the JSON digest before packaging; the browser loader asserts
the manifest digest against the custom section (§7). A stale consumer is a
*compile error or immediate build failure*, never a runtime panic mid-import.

The registry is per-build (it encodes the user's closed graph), so unlike
`op_kinds.toml` it is generated into the build directory, not checked in; the
*generator and schema* are the checked-in frozen authority.

---

## 4. Runtime design

### 4.1 ModuleTable

`ModuleTable` owns `dictionary: AtomicU64`,
`initialization: OnceLock<ModuleInitialization>` and one lazy execution-lock
manager. The dictionary retains the ordinary Python dict; initialization lazily
sizes compiled states, initializer slots and original-spec pins from the registry.
The execution manager owns synchronization-only records for active registered or
arbitrary names, with one reentrant owner/wait graph and Condvar wakeups. It keeps
no Python object and removes completed name records.
There is no separate module-object HashMap, overflow map, journal or dict backing
tag. Names outside the compiled registry remain ordinary dictionary entries.

States `Uninit`, `Initializing`, `Ready`, `Tombstone` and `Replaced` describe
ordinary import admission. `Ready`/`Replaced` retain no namespace or spec owner.
An active ordinary initializer retains its partial namespace and original spec
through its finally setter. Public dictionary deletion/replacement is observed
without adopting or replaying a shadow owner. Actual public spec truth gates
foreign waits; a completed replacement cannot inherit the original's wait.

Fresh runpy/loader execution owns its namespace in `ExecutionContext`, without
reserving or restoring global import rows. Its tagged cold execution identity
and ordinary imports enter the same typed native/WASM body dispatcher. Imports
performed by that private body still use public membership and ordinary import
admission; name-wide suppression and the reserved-row state are removed. The
optional run-name public swap remains independently owned and restored.
Publication and rollback commit metadata before releasing displaced Python edges.

### 4.2 Name resolution (string → id), at most once per dynamic site

`module_id_of(name: &[u8]) -> Option<ModuleId>` — generated minimal perfect
hash (or sorted-array binary search; N is O(10²–10³), both are ~ns-scale; PHF
chosen for O(1) worst case and zero comparisons on miss-by-hash). Used by:

- `molt_importlib_import_transaction` after CPython-exact argument validation
  and relative-name resolution (both unchanged, already transaction-owned);
- `hook_import_module` (C extensions);
- the isolate host boundary (wasm `molt_isolate_ensure(id)` replaces
  `molt_isolate_import(name_bits)`);
- the dict view for registry-name key lookups.

Compiled *literal* import sites skip it entirely: the frontend already proves
literal absolute names (import_system_contract.md §3.3); lowering emits
`module_slot(canonical_name) → box → ensure(ID)`. Reusable function IR carries
the canonical name, so adding a module earlier in registry order does not
invalidate unrelated objects. Final native/LLVM application emission defines
immutable code-owner and module-slot binding cells in distinct symbol domains;
reusable objects import those cells. Whole-program WASM emission resolves the
same module slots to immediate IDs. The runtime still receives a boxed dense
integer and performs no additional name lookup.

The Python `ModuleRegistry` remains the sole ID and blob producer. Its backend
projection includes every canonical name in ModuleId order and an optional
initializer per row; aliases retain their distinct row indices. Shared IR
admission checks row order, uniqueness, the u32 count bound, initializer roots
and symbolic references. The serialized blob and typed rows share the existing
trusted-producer boundary: backends do not parse the blob to reconstruct a
second registry, and arbitrary conflicting blob/row payloads are not claimed
to be independently cross-validated there.

### 4.3 Initialization transaction and dictionary ownership

Normal ensure first consumes one owned canonical dictionary observation.
Present arbitrary values return unchanged; `None` raises the versioned import
failure; absence enters the admitted parent-first initialization transaction.
One all-name execution lease coordinates compiled initialization, arbitrary fresh
spec loads and existing-target `_exec`; dense CAS state describes only compiled
initializer namespace custody. False-flag completed imports bypass the lock map.
A same-thread recursive import returns a partial module only while the public dictionary
contains it. If its initializer deletes that entry, a same-thread import starts
a fresh transaction through the same initializer. The existing execution
snapshot retains the outer temporary namespace, original spec and state while
the inner transaction runs; the reentrant name lease remains scoped outside the
snapshot. Restoration changes only that initializer custody; it never restores
the public dictionary. This follows CPython 3.12.15
[`_find_and_load`](https://github.com/python/cpython/blob/v3.12.15/Lib/importlib/_bootstrap.py#L1349-L1376).
Initializer publication installs the dictionary
entry and temporary namespace owner before body execution. Successful ordinary
completion releases that temporary owner; explicit execution releases its
separate context-owned namespace after its own body and public run-name cleanup. Ordinary import-body
failure removes the current name, including a nested replacement or `None`, as
[`_load_unlocked`](https://github.com/python/cpython/blob/v3.12.15/Lib/importlib/_bootstrap.py#L911-L952)
does. Extension construction rollback instead removes only its exact admitted
publication; direct loader/runpy execution preserves its separate public-cache
policy. Neither cleanup policy repeats dictionary equality to reconcile
quiescent metadata: only a later ordinary Missing observation admits a new
initialization.
Parent attribute callbacks run while an independent return owner is pinned.

### 4.4 `sys.modules`: one ordinary interpreter dictionary

The interpreter owns an ordinary Python dictionary before sys bootstrap and
exposes that same object as `sys.modules`. This follows
[CPython 3.12.15 `Python/import.c` lines 168-246](https://github.com/python/cpython/blob/v3.12.15/Python/import.c#L168-L246).
Dictionary primitives own hashing/equality callbacks, deletion, arbitrary public
values, insertion order, copying, views and GC edges. Import metadata never owns a
second completed object projection. Rebinding the sys attribute does not replace
the interpreter's internal dictionary, as in CPython.

The earlier `Materialized`/`ModuleTableView` backing-tag proposal is retired.
It would require every dict operation, iterator, C view, GC projection and layout
consumer to recognize a new storage form, while the maintained ordinary dict
already provides the needed semantics. No measured gain justifies that cost.
The removed Rust map, adoption helpers and bootstrap replay are not preserved as
adapters. Semantic module lookup/publication/removal operations remain live and
use the one dictionary; their identities still have real producers.


### 4.5 The name-resolution family: one kernel, three miss policies

`module_get_attr` / `module_get_global` / `module_import_from` collapse onto
one kernel:

```rust
fn module_name_lookup(py, module_ptr, name_bits, policy: MissPolicy) -> u64
enum MissPolicy { Attr,          // AttributeError "module 'm' has no attribute 'n'"
                  Global,        // builtins fallback, then NameError
                  ImportFrom }   // PEP-562 §5.7 → sys.modules["m.n"] recovery →
                                 // ImportError (+ "partially initialized module"
                                 // wording when states[id]==Initializing §5.4)
```

The three exported symbols become 3-line wrappers (thinnest-ABI-entrypoint
wrappers per the agent contract). The frontend op-selection decision (AST
context → op kind) is a single generated decision table with a differential
gate over the exception-class matrix (G8). Wrong-exception-class drift
(incident 10) requires editing one table protected by one gate.

---

## 5. CPython 3.12 parity matrix

Each supported-subset behavior below is pinned to the CPython source of truth
(all quotes verified against docs.python.org/3.12 and the 3.12 branch of
python/cpython on 2026-07-02; full quotes in §5-bis) and becomes a
differential gate row (G8).

| # | Behavior | Molt mechanism | CPython authority |
|---|---|---|---|
| 5.1 | `sys.modules` is *the* first-consulted module cache; "if present, the associated value is the module satisfying the import" — even an arbitrary non-module object is returned as-is; replacing the *dict object* itself "will not necessarily work as expected" (machinery keeps its own reference) | Canonical interpreter dictionary lookup; arbitrary present values pass through | import reference §5.3.1 "The module cache"; `PyImport_ImportModuleLevelObject` (import.c) returns the sys.modules value whenever non-NULL and not None [A1] |
| 5.2 | Cached entry returned without re-exec; entry `None` → **`ModuleNotFoundError`** `"import of {name} halted; None in sys.modules"` | owned canonical dictionary observation plus public spec probe; a present None raises the exact import failure | `_bootstrap._find_and_load` fast path [A2] |
| 5.3 | `del sys.modules['x']` then `import x` → full re-execution, **new** module object; old references keep the old module; (`importlib.reload` by contrast reuses the same object) | `Tombstone` → Source reinit path; reload uses the Exact existing target under its name lease, without changing `_initializing` | import reference §5.3.1 [A3] |
| 5.4 | The module is placed in `sys.modules` **before** loader exec ("crucial because the module code may … import itself"); cyclic importers see the partial module; `from x import n` on a partial module raises `ImportError` `"cannot import name %R from partially initialized module %R (most likely due to a circular import)"` — wording is 3.9+ (bpo-20490) and lives in `import_from` in **Python/ceval.c**, keyed on `_PyModuleSpec_IsInitializing`; plain attribute access on a partial module raises the 3.8+ AttributeError variant (bpo-33237) | publish-before-exec (I6); import diagnostics observe the actual module spec's initializing truth with the target-version error policy | `_bootstrap._load` (`sys.modules[spec.name] = module` before `exec_module`, `del` on failure); `import_from` (ceval.c); commits 65366bc8bd, 3e429dcc24 [A4] |
| 5.5 | Importing `a.b` imports `a` first (`if parent not in sys.modules: import_(parent)`); after child init, importlib does `setattr(parent_module, child, module)` — `AttributeError` there degrades to `ImportWarning`; `import a.b` binds top-level name `a` in the importer; `sys.modules['a.b']` must appear as attribute `b` of `sys.modules['a']`; `_handle_fromlist` deliberately swallows `ModuleNotFoundError` for fromlist submodules whose sys.modules entry is not None | parent dep edge + `bind_parent_attr` post-init (with the ImportWarning degradation); transaction return picks top-level for empty fromlist (existing `importlib_transaction_return_value`, now id-keyed); existing fromlist-swallow behavior kept | `_bootstrap._find_and_load_unlocked`, `_handle_fromlist`; import reference §5.4.2; simple_stmts import-statement binding rule [A5] |
| 5.6 | Failed module exec removes the module from sys.modules (`_load`'s except: `del sys.modules[spec.name]`); next import retries fully | unwind path: unpublish → `Uninit` | `_bootstrap._load` [A6] |
| 5.7 | PEP 562: `object.__getattribute__` (module `__dict__`) first, then module-level `__getattr__`; **direct global access (LOAD_GLOBAL) is explicitly unaffected by `__getattr__`** | unchanged `module_attr_lookup` shared by the one kernel; `MissPolicy::Global` correctly bypasses `__getattr__` — now a *cited* behavior, not an accident | PEP 562 Specification; datamodel §3.3.2.1 [A7] |
| 5.8 | Single-phase C extensions (`m_size == -1`): import.c keeps a per-process `extensions` cache; "a copy of the module's dictionary is stored" (`m_copy`, via `_PyImport_FixupExtensionObject`) immediately after init succeeds; re-import after `del sys.modules[x]` creates a fresh module whose dict is updated from that snapshot — **PyInit is not re-run**; post-init dict mutations do NOT survive re-import; multi-phase (PEP 489) modules are "not singletons" and are re-created normally | `ext_dict_copy[id]` snapshot at init success; `Tombstone(kind=Extension)` → fresh module + `dict.update(snapshot)`; multi-phase support = future registry `phase` flag with Source-like reinit | c-api/module.html (single/multi-phase, PyState_FindModule borrowed ref); import.c `extensions` comment, `import_find_extension`, `_PyImport_FixupExtensionObject` [A8] |
| 5.9 | Name validation at API boundary: C path (`builtins.__import__` → `PyImport_ImportModuleLevelObject`) raises `TypeError("module name must be a string")`; Python path (`_bootstrap._sanity_check`) raises `TypeError(f'module name must be str, not {type(name)}')`, plus level/package/empty-name errors | validation stays in the transaction prologue with the per-API exact message (already transaction-owned per import_system_contract.md §3.3); interior is typed `ModuleId` — the interior can no longer emit this error at all | import.c `PyImport_ImportModuleLevelObject`; `_bootstrap._sanity_check` [A9] |
| 5.10 | Concurrent import: per-module recursive locks (`_ModuleLock`, `_blocking_on` wait-graph, `_DeadlockError`); waiting for an in-progress module goes through `_lock_unlock_module`, which **catches `_DeadlockError` and accepts the partially initialized module** ("Concurrent circular import, we'll accept a partially initialized module object") | one transient all-name recursive execution lock and event wakeup; linear wait-graph cycle check accepts an already observed public partial without reviving a deleted entry | `_bootstrap._ModuleLock`, `_get_module_lock`, `_lock_unlock_module`; import.c `import_ensure_initialized` [A10] |
| 5.11 | `PySys_GetObject`: "Return the object *name* from the `sys` module or `NULL` if it does not exist, **without setting an exception**"; borrowed reference | hook reads the interpreter-owned sys namespace; public import replacement/deletion cannot revoke its lifetime; NULL-not-exception and exact incoming error preservation are maintained | c-api/sys.html `PySys_GetObject` [A11] |

---

## 6. The extension lane rides the same transaction

- **Static PyInit registration:** registry row `kind=Extension` carries the
  init symbol; `ensure` calls `prepare_static_extension` + PyInit + to-bits —
  the same three calls `_build_static_native_module_init_ops` emits today,
  minus the hand-rolled preamble/parent/exports ops, which all move into
  `ensure`/`bind_parent_attr`.
- **`PyImport_ImportModule`/`PyImport_Import` from C:** `hook_import_module`
  → `module_id_of` → `ensure`. Because the C-source scan
  (`external_native.py`, `import_scan_mode="module_init"`) feeds `row.deps`,
  every literal dynamic import from extension init is *in the registry by
  construction*; the runtime miss diagnostic names the requesting extension
  and the custody channel (`"module 'math' requested by extension 'X' init is
  outside the compiled closure; admit it via the extension import scan"`).
  Non-literal (computed) C-side names fail closed with the same diagnostic —
  honest AOT boundary. [Incident 9]
- **Capsule/attr-export dependencies** (`module_attr_exports`) become ordinary
  `deps` edges: `ensure(provider)` before reading provider attrs, inside the
  consumer's transaction. No second init-invocation path exists for aliases or
  providers. [Incident 2]
- **Module state / `PyState_FindModule`:** the existing
  `module_capi_register/get_state` hooks key by module identity; they now key
  by `ModuleId`, making state lookup an array index as well.
- **`PySys_GetObject`:** canonical initialization establishes one interpreter
  sys namespace owner independently of mutable public/private import projections.
  Warm C lookups read only its dictionary without importing or invoking module
  `__getattr__`; only cold bootstrap enters the existing initializer. Native
  bootstrap facts and Python-shaped metadata/API definitions are published by
  completion of that initializer. Reads and retained-namespace republication do
  not reconstruct deleted exports. ModuleRetirement releases the role with sys.

---

## 7. WASM: every table is a projection, and the manifest is extracted, not asserted

- **One layout plan:** the backend computes the callable/poll/dispatch layout
  once (as `callable_table/layout.rs` does today) but sources slot sets from
  the registry + ABI manifest projections, and **serializes the final layout
  (table base, segment extents, registry digest, ABI digest) into a custom
  section of the emitted `.wasm`**.
- **The browser manifest is generated FROM the binary** post-link by reading
  that custom section — the manifest is a projection of the artifact, so
  `wasm_table_base` drift is not detectable-at-load, it is *inexpressible*:
  there is no second computation to disagree. `loader_bridge.js` keeps its
  check as defense-in-depth against artifact mixing (stale .wasm + fresh
  manifest from different builds), now comparing digests, and the same check
  runs at **build time** as a required post-link gate (G6). [Incident 5]
- **Poll/callable import declarations are derived by iterating the same
  generated spec that assigns slots** (`POLL_TABLE_IMPORTS` already exists;
  the fix is that the backend's import-declaration pass iterates it rather
  than maintaining a parallel set), so `"missing poll import for io_wait"`
  becomes a generator-time impossibility; a crate unit gate instantiates every
  generated table spec against the import registry on every build (G5).
  [Incident 6]
- **Split-runtime / isolates:** each instance owns its own `ModuleTable`; the
  registry consts are identical in both because both are compiled from the
  same generated artifact and digest-checked at instantiation (the runtime
  cdylib and app module exchange `registry_digest` during the existing
  handshake alongside `molt_set_wasm_table_base`). The isolate import boundary
  is `molt_isolate_ensure(id: u64) -> bits` — no strings cross the boundary.
  [Incidents 5, 11]

---

## 8. Performance

### 8.1 Current warm-import cost boundary

Warm import uses the existing callback-safe Python dictionary hash/probe and
returns an independently owned result. Compiled initialization then uses its
existing state/owner admission where necessary. It does not acquire the removed
Rust module-cache mutex or keep a second completed-slot owner. Python equality
callbacks, subclass keys and public mutation remain real semantics; an array-only
or zero-hash cost claim is not applicable to this implementation.

Compare `tests/benchmarks/bench_builtin_import_slice.py` and
`tests/benchmarks/bench_import_time.py` on identical baseline/candidate
source/profile/target coordinates. Record execution time, allocation/retention,
startup and shutdown separately. No new measurement or speedup is implied here.

### 8.2 Cold init

O(1) dispatch to `MODULE_INIT_TABLE[row.init]` (indirect call / wasm
`call_indirect` on a registry-assigned slot) replaces the O(modules)
`string_eq` chain. Total startup = Σ module body costs + O(N) table walk —
O(modules), the information-theoretic floor.

### 8.3 Eager-init flattening (verified optimization only)

For the subgraph the compiler proves acyclic, `molt_main` may call init
functions in generated topological `INIT_ORDER` directly (no ensure state
checks) — but only under a proof obligation (no back edge, no cross-thread
exposure before completion) checked by the generator; `ensure` remains the
semantic definition. This mirrors Go exactly: the linker computes the init
schedule (breadth-first over `R_INITORDER` edges, lexicographic tie-break)
into a flat pointer array, and the runtime walker (`doInit1`) still tracks
`state uint32 // 0 = uninitialized, 1 = in progress, 2 = done` and hard-fails
on `case 1: throw("recursive call during initialization - linker skew")` —
the flattened order is an optimization; the state machine is the law [B1].
GraalVM-style build-time init + heap snapshotting is a *compatible future
extension* (a `Ready`-at-image-start state), explicitly out of scope now
[B2].

### 8.4 Binary size / tree-shaking

The registry is the reachability spine: `compile_modules` (existing closure
authority) rows only. Dead-module elimination shrinks N. The PHF and name
table are the only string data; init preambles, string_eq chains, per-import
name literals at call sites, and trace scaffolding all net-delete.

---

## 9. Prior art (verified against primary sources, 2026-07-02)

- **Go** — the direct ancestor of §4.3. Spec: "If a package has imports, the
  imported packages are initialized before initializing the package itself.
  If multiple packages import a package, the imported package will be
  initialized only once"; init runs "in a single goroutine, sequentially, one
  package at a time." Mechanism: each package gets a `p..inittask` record;
  "if package p imports package q, then package p's inittask record will have
  a R_INITORDER relocation pointing to package q's inittask record"; the
  linker orders all inittasks respecting dependencies (lexicographic
  tie-break), and `runtime.doInit` iterates the flat schedule with a per-task
  state byte {0=uninitialized, 1=in progress, 2=done} — **no string lookups
  anywhere; the DAG is resolved entirely at link time into a pointer array.**
  [B1]
- **GraalVM native-image**: closed-world class-init with build-time/run-time
  split and image-heap snapshotting ("values written to static fields by this
  code are saved in the image heap"; "when native image starts up, it copies
  the initial image heap from the binary"). Their init-policy churn —
  `--initialize-at-build-time`/`--initialize-at-run-time` per class, then the
  JDK 21 `--strict-image-heap` redesign ("all classes are now allowed to be
  used and initialized at build time"), made default in JDK 22 — is the
  cautionary tale motivating the freeze contract: init policy that isn't
  nailed down keeps getting redesigned for years. [B2]
- **MicroPython**: frozen modules are found by `mp_find_frozen_module` doing a
  **linear `memcmp` scan** of `mp_frozen_names[]`, hooked into sys.path via
  the `.frozen/` path-prefix sentinel — i.e., string-scan dispatch, the same
  shape as molt's incident-11 chain. Molt's fully closed world lets it do
  strictly better (dense ids, PHF only at dynamic boundaries). [B3]
- **Codon** (AOT Python): compile-time import resolution; anything outside the
  compiled/native set must go through explicit `from python import` CPython
  interop (requires `CODON_PYTHON` shared library). Precedent for molt's
  fail-closed closed-world diagnostic. *Flag: Codon's docs never state "no
  runtime import machinery" verbatim — that is an inference from its AOT
  pipeline docs; treated as corroborating, not authoritative.* [B4]
- **PyOxidizer `oxidized_importer`**: packed-resources binary format — global
  header, blob index, resources index, then data, "designed such that the
  index data is at the beginning so a reader only has to read a contiguous
  slice" with 0-copy reads; `OxidizedFinder` parses it to power importing.
  Precedent for the generated registry-as-index artifact. [B5]
- **WASM dynamic linking (`dylink.0`)**: the custom section carries the
  memory/table sizes and alignments a module needs; **the loader assigns
  `__memory_base`/`__table_base` and reserves the regions** — bases are read
  from the binary's own declaration, never computed in parallel by the host.
  Precedent for §7's extract-from-binary manifest. [B6]

---

## 10. The eleven incidents, made structurally impossible

| # | Incident (2026-07-02 corpus) | Why it cannot recur |
|---|---|---|
| 1 | `TypeError: module name must be str` on plain `import guardpkg` | Compiled import sites bind symbolic module slots to final `ModuleId: u32` — there is no string argument to corrupt. The *only* string-accepting surfaces are the CPython-parity API boundaries, which validate with CPython's exact error (§5.9) before converting to id. The `trace_bad_module_name_arg` debug lattice is deleted. |
| 2 | Static-native init re-ran via alias bypass ("cannot load module more than once") | Aliases have no init of their own — `kind=Alias{target}` resolves inside `ensure`; init-exactly-once is the CAS `Uninit→Initializing` in one function. Per-init preambles no longer exist to be bypassed. Extension re-init is additionally impossible via the `ext_dict_copy` snapshot path (§5.8): PyInit has exactly one call site, guarded by the CAS. |
| 3 | Two stores + sync/replay/first-init-wins | There is one ordinary interpreter dictionary, exposed unchanged as `sys.modules`. The private Rust map, adoption fallback, bootstrap replay and permanent completed-slot owners are removed; semantic module lookup/publication/removal operations remain live. |
| 4 | `molt_sys_modules()` bootstrap-empty dict | Nothing read the intrinsic, and HF-110 deleted it (2026-10-09). `sys.modules` is the singleton table view, created during sys's own `ensure`, which precedes every other module by a registry dependency edge. An "empty sys.modules while modules exist" state is unrepresentable. |
| 5 | `manifest wasm_table_base 4135 above binary table base 2475` | The manifest is extracted from the linked binary's layout section — one computation, two projections. Digest-checked at build (G6) and load. |
| 6 | `missing poll import for io_wait` panic | Import declarations are derived by iterating the same generated slot spec; a slot without an import cannot be expressed. Crate gate G5 instantiates every table spec per build. |
| 7 | Generated-file staleness (`molt_PyArg_ParseTuple` missing) | Every projection embeds the authority digest; consumers const-assert it. Stale = compile error, enforced by existing generator `--check` gates extended to the registry (G7). |
| 8 | `PySys_GetObject` re-entering import machinery | The warm hook reads the interpreter-owned sys namespace directly, independently of public import state; cold bootstrap uses the existing initializer. |
| 9 | C-extension dynamic imports invisible to the graph → tree-shaken → ImportError | Scan results are registry `deps` edges (same artifact as everything else); the runtime miss is a fail-closed diagnostic naming extension + channel. Gate G9 compiles a fixture extension with a `PyImport_ImportModule` literal and asserts admission. |
| 10 | `module_get_attr` vs `module_get_global` wrong exception class | One lookup kernel + `MissPolicy` enum + one frontend decision table + differential exception-class gate (G8). The behavior difference is data, not duplicated code. |
| 11 | Runtime string-comparison chains in isolate dispatch | `molt_isolate_ensure(id)`; the chain generator is deleted. Strings never cross the isolate boundary. |

---

## 11. Recursive adversarial review

Historical design review (July 2026): Round 1 attacked the initial draft with
the corpus (§10), and Round 2 below records the original design responses.
Current dictionary, execution-lock and initializer ownership in §4 and the
canonical cutover contract supersede older per-slot owner/parking proposals.
These retained questions explain the constraints, not current source layout.

**R2.1 — Threads: two threads import the same module concurrently.**
Attack: both CAS `Uninit→Initializing`? No — CAS admits one; the loser parks
on the per-slot lock. Attack refined: thread A inits `x`, which imports `y`,
which thread B is initializing, and B's `y` imports `x` — cross-thread cycle,
both park forever. Answer: `owners[]` forms a wait-for graph; before parking,
walk it (CPython `_blocking_on` analog). On a detected cycle, do exactly what
CPython 3.12's `_lock_unlock_module` does — catch the deadlock and **accept
the partially initialized module** (verified: the except-`_DeadlockError`
branch is commented "Concurrent circular import, we'll accept a partially
initialized module object", §5.10/[A10]). Gate: a two-thread cyclic-import
differential test (G10). Residual honesty: molt's GIL means bodies interleave
only at yield points, narrowing but not eliminating the window — the lock
discipline is still required and specified, not assumed away.

**R2.2 — Re-entrancy: extension PyInit imports the module being initialized.**
`hook_import_module` → `ensure(id)` → state `Initializing`, owner == self →
returns the partial module. Identical to CPython single-phase behavior (the
module is in sys.modules during init). No special case needed — falls out of
publish-before-exec.

**R2.3 — Partially-initialized module visibility via sys.modules view.**
Attack: user code iterates `sys.modules` during a cyclic init and sees a
partial module — divergence? No: CPython exposes partial modules in
sys.modules during exec too (§5.4). The view makes molt *match* CPython here,
where today's replay machinery could show a module in the Rust map but not in
sys.modules or vice versa.

**R2.4 — `del sys.modules['x']` + reimport, but `x` is mid-init.**
Attack: `del` during `Initializing` → Tombstone → owner finishes init and
blindly sets `Ready`, resurrecting a deleted entry. Amendment: the finishing
transition is `CAS(Initializing→Ready)`; if the state changed underneath
(Tombstone/Replaced), the owner completes its module object (callers holding
the partial ref keep a coherent module — CPython behaves the same: exec
continues on a module object even if unlinked) but does **not** republish;
journal untouched. This CAS rule is part of the frozen invariant I4.

**R2.5 — `sys.modules['x'] = None` then `import x`.**
`Replaced(None)` → `ModuleNotFoundError("import of x halted; None in
sys.modules")` per §5.2/[A2] — note the exception class: ModuleNotFoundError,
not bare ImportError (a draft of this design got that wrong; the primary
source corrected it — which is exactly why the matrix pins message-level
citations). The *view* stores None as the user value (visible to
`sys.modules.get('x')`), while `ensure` raises — exactly CPython's split.
Covered by gate row.

**R2.6 — WASM split-runtime: runtime cdylib and app module disagree on ids.**
Both are compiled from the same generated registry, but an operator could mix
artifacts from two builds. Answer: `registry_digest` exchanged in the
instantiation handshake (§7); mismatch fails instantiation with a
which-artifact diagnostic. Same defense as the table-base check, now covering
module identity itself.

**R2.7 — Overflow-lane aliasing: user inserts `sys.modules['pkg.sub']` for a
non-registry name, then a registry module's `import_from` recovery reads it.**
The `ImportFrom` miss policy's `sys.modules["m.n"]` probe goes through the
same view (registry lane first, then overflow) — user-registered synthetic
submodules are honored, matching CPython where sys.modules is the recovery
authority. No second lookup path to drift.

**R2.8 — Registry id instability across builds breaking caches.**
Ids are sorted-name-dense; adding a module shifts ids. Attack: stale
object-cache artifacts embedding old ids link against a new registry.
Answer: ids are *internal to one build*; every artifact embedding ids also
embeds `registry_digest`, and the backend object-cache key includes it
(existing cache-key discipline extends). Cross-build id reuse is inexpressible
at link time (digest const-assert).

**R2.9 — `importlib.reload`.**
Reload mutates a module in place and re-execs — a state transition. It rides
`ensure`'s owner path with a `Reload` entry: `Ready → Initializing (same
object, dict preserved) → Ready`, per CPython reload semantics; gated as part
of the existing reload gating policy (import_system_contract.md §4). Not a
second path — an additional labeled transition in the same machine.

**R2.10 — Fork/spawn (`multiprocessing`).**
The table is process state; spawn re-runs bootstrap with the spawn entry
override (existing `ENTRY_OVERRIDE_SPAWN` lane becomes a registry-selected
entry id, removing that `string_eq` too). Fork (unix): table transfers as
memory, coherent by construction — no cross-process sync existed to break.

**Verdict of the review:** no attack in either round required adding a second
store, a reconciliation pass, or a per-callsite guard — each was answered by a
rule *inside* the single transaction or a digest on the single artifact. That
is the structural signature this design was chosen for.

---

## 12. Freeze contract

### 12.1 Invariants (the bedrock)

- **I1 (One identity):** within a build, a module's identity is its
  `ModuleId`; strings appear only at CPython-parity API boundaries and are
  converted by the generated resolver exactly once per dynamic call.
- **I2 (One artifact):** every table, manifest, dispatcher, or doc that
  encodes module identity/layout is a generated projection of the
  `ModuleRegistry` (wasm ABI surfaces additionally of `wasm_abi_manifest.toml`)
  and embeds the authority digest.
- **I3 (One store):** `ModuleTable` owns one ordinary interpreter dictionary,
  exposed unchanged as `sys.modules`. No map, adoption cache or completed slot
  retains a second module-object store. Typed initializer owners and interpreter
  sys/builtins semantic roles have separate explicit lifetimes.
- **I4 (One transaction):** all state transitions occur in
  `molt_module_ensure` + the two view-mutation entry points, under the
  per-slot CAS/lock discipline of §4.3/R2.4.
- **I5 (Init exactly once):** `Uninit→Initializing` CAS is the only init
  admission; init bodies are reachable only through `MODULE_INIT_TABLE`.
- **I6 (Publish before exec):** module objects are visible in the store for
  the entirety of their body execution (CPython cycle parity).
- **I7 (Parity rows are law):** the §5 matrix rows are individually gated;
  changing observable import semantics requires changing the matrix + gate in
  the same arc.
- **I8 (One kernel, three policies):** module-name resolution has one lookup
  kernel; miss behavior is the `MissPolicy` table.
- **I9 (Extension = same lane):** every extension-origin import/init/state
  operation enters through `ensure`/table APIs; C-scan deps are registry
  edges.
- **I10 (Artifact-derived manifests):** the browser/embed manifest is
  extracted from the linked binary's layout section, never computed in
  parallel.
- **I11 (Fail closed, named channel):** any name outside the closed world
  raises a diagnostic naming the requesting site and the admission channel.

### 12.2 Gates (each invariant has teeth)

| Gate | Enforces | Kind |
|---|---|---|
| G1 `test_module_registry_projection_digests` | I2, I10 | projection `--check` + const-assert compile gate (`tools/check_module_registry.py --check`) |
| G2 `test_module_table_single_store` | I3 | runtime unit: grep-free structural gate — the only `HashMap`-of-modules symbol is gone; plus behavior: Python-side `sys.modules` mutation observed by `ensure` without any sync call |
| G3 `test_init_reachable_only_via_table` | I5 | backend gate: no direct `call` op targets `molt_init_*` symbols except the table/INIT_ORDER emitters (mirror of the existing `canonical_lowering_default_kinds_are_natively_handled` gate style) |
| G4 `test_ensure_state_machine` | I4, I5, I6 | synthetic runtime unit driving every transition incl. R2.4 CAS races |
| G5 wasm table-spec instantiation gate | I2 | molt-backend-wasm crate unit over every generated slot spec (extends `test_wasm_runtime_export_no_mangle.py` family) |
| G6 post-link layout parity | I10 | build-time: extract custom section, diff manifest; fails the build not the browser |
| G7 registry digest staleness | I2 | extends existing generated-file `--check` gates (`test_wasm_runtime_call_signature_authority.py` family) |
| G8 parity differential matrix | I7, I8 | `tests/differential/` rows per §5 line + exception-class matrix for the three miss policies (extends `tests/differential/stdlib/importlib_*` and `tests/test_native_import_bootstrap_regressions.py`) |
| G9 extension dynamic-import closure | I9, I11 | fixture C extension with `PyImport_ImportModule` literal → graph admission asserted (extends `tests/cli/test_cli_import_collection.py`) |
| G10 two-thread cyclic import | I4 | differential/stress lane (proof-queue owned) |
| G11 split-runtime digest handshake | I2 | wasm E2E: mixed-build artifacts must fail instantiation with the named diagnostic |

### 12.3 Generated-artifact authorities

- `tools/check_module_registry.py` (+ schema doc) — registry and all §3
  projections; per-build output, checked by G1/G7.
- `tools/gen_wasm_abi.py` + `wasm_abi_manifest.toml` — unchanged authority for
  the ABI import surface; the registry consumes its digest, not its contents.
- The linked binary's layout custom section — sole source for embed manifests
  (G6).

### 12.4 Freeze text for CLAUDE.md / AGENTS.md (verbatim insert)

> ## Frozen Bedrock: Module Import Layer
>
> The module identity/init/import/sys.modules layer is FROZEN. Its invariants
> (I1–I11) and gates (G1–G11) are defined in
> `docs/spec/areas/compat/contracts/import_system_contract.md` §Bedrock.
> Binding rules:
>
> - Module identity is a compile-time `ModuleId` from the generated
>   `ModuleRegistry`; `ModuleTable` is the only store; `sys.modules` is its
>   view; `molt_module_ensure` is the only state-transition owner. Do not add
>   a module cache, mirror, name-keyed map, init preamble, string-comparison
>   dispatch, or parallel manifest computation ANYWHERE, for any reason,
>   including "temporarily."
> - Any change to this layer requires the full invariant-proof gauntlet in the
>   same arc: every gate G1–G11 green, the §5 parity matrix re-proven
>   differentially, and the contract doc updated. Incremental patches,
>   workaround lanes, and compatibility shims in this layer are defects by
>   definition and must be reverted to the invariant, not accommodated.
> - If a requirement genuinely cannot be expressed inside I1–I11, the correct
>   action is a design revision of the bedrock document with operator sign-off
>   — never a side channel.

---

## 13. Senior-engineer verdict

The original incident and adversarial sets define proof obligations. Source
implementation, structural gates and retained historical evidence do not close
all native/WASM/extension/concurrency cells. The current storage contract names
the changed ownership controls; full qualification remains separately recorded.

**Residual risk, named honestly:**
1. *Dictionary ownership and callback lifetime.* Reuse the maintained ordinary
   dict protocol and prove owned lookup, replacement/deletion, callback reentry,
   semantic role retention and ordered shutdown. No custom backing tag remains.
2. *Thread/init interleaving.* R2.1's lock discipline is specified but
   concurrency bugs are found by stress, not review — G10 must be a real
   proof-queue lane before freeze is declared.
3. *CPython drift.* 3.13+ changes import internals (e.g., per-interpreter
   state). The parity matrix pins 3.12; version bumps re-run the matrix
   against new primary sources — that is a planned unfreeze-point, not churn.
4. *The moving wasm module_abi/ directory.* Another engineer is actively
   editing callable/poll/imports; PR3 must rebase onto their end state and
   land the projection unification with them, not around them.

**Migration sequencing (each PR is a complete end-state subsystem cut):**
- **PR1 — Registry + ensure + init table (native).** Generator, runtime
  `module_table.rs`, `molt_module_ensure`, `MODULE_INIT_TABLE`; bind explicit
  internal initializer sites and delete per-init preambles, the isolate string_eq chain
  generator, and `molt_isolate_import(name)` in favor of
  `molt_isolate_ensure(id)`. Gates G1, G3, G4, G7. (Runtime + backend_ir.py
  + frontend lowering in one arc; no old lane left.)
- **PR2 — One store.** `ModuleTable` owns the interpreter dictionary, exposed
  as `sys.modules`; delete the private module map, adoption, replay and permanent
  completed-slot owners. Preserve live semantic module operations and explicit
  sys/builtins roles. The current cutover contract defines ownership and gates;
  native/WASM execution and measured costs remain release obligations.
- **PR3 — WASM projection unification.** Registry-driven callable/poll/
  dispatch slots, layout custom section, extract-derived manifest, digest
  handshake, build-time parity gate. Gates G5, G6, G11. Coordinate with the
  in-flight module_abi work.
- **PR4 — Extension lane + freeze.** Extension rows through `ensure`,
  `ext_dict_copy` snapshots, C-scan deps as registry edges, G9/G10; land the contract-doc
  §Bedrock section and the CLAUDE.md/AGENTS.md freeze text last, once G1–G11
  are green.

---

## §5-bis. Citation appendix (primary sources, fetched and verified 2026-07-02)

**[A1]** https://docs.python.org/3.12/reference/import.html#the-module-cache —
"The first place checked during import search is `sys.modules`. … During
import, the module name is looked up in `sys.modules` and if present, the
associated value is the module satisfying the import, and the process
completes." https://docs.python.org/3.12/library/sys.html#sys.modules —
"replacing the dictionary will not necessarily work as expected and deleting
essential items from the dictionary may cause Python to fail."
`PyImport_ImportModuleLevelObject`,
https://github.com/python/cpython/blob/3.12/Python/import.c — returns the
sys.modules value whenever `mod != NULL && mod != Py_None` (arbitrary-object
passthrough is code-confirmed; the docs never restrict values to modules).

**[A2]** `_find_and_load`,
https://github.com/python/cpython/blob/3.12/Lib/importlib/_bootstrap.py —
`module = sys.modules.get(name, _NEEDS_LOADING)`; and
`if module is None: message = f'import of {name} halted; None in sys.modules';
raise ModuleNotFoundError(message, name=name)`.

**[A3]** https://docs.python.org/3.12/reference/import.html#the-module-cache —
"if you keep a reference to the module object, invalidate its cache entry in
`sys.modules`, and then re-import the named module, the two module objects
will *not* be the same. By contrast, `importlib.reload()` will reuse the
*same* module object."

**[A4]** https://docs.python.org/3.12/reference/import.html#loading — "The
module will exist in `sys.modules` before the loader executes the module
code." `_load` in `_bootstrap.py` — `sys.modules[spec.name] = module` before
`exec_module`, `del sys.modules[spec.name]` in the except block. `import_from`
in https://github.com/python/cpython/blob/3.12/Python/ceval.c —
`_PyModuleSpec_IsInitializing(spec) ? "cannot import name %R from partially
initialized module %R (most likely due to a circular import) (%S)" : …`.
History: ImportError variant is 3.9+
(https://github.com/python/cpython/commit/65366bc8bd, bpo-20490); the
AttributeError variant on plain attribute access is 3.8+
(https://github.com/python/cpython/commit/3e429dcc24, bpo-33237).

**[A5]** `_find_and_load_unlocked` in `_bootstrap.py` —
`if parent not in sys.modules: _call_with_frames_removed(import_, parent)`;
post-load `setattr(parent_module, child, module)` with
`except AttributeError: _warnings.warn(msg, ImportWarning)`.
https://docs.python.org/3.12/reference/import.html#submodules — "the latter
must appear as the `foo` attribute of the former."
https://docs.python.org/3.12/reference/simple_stmts.html#the-import-statement —
"the name of the top level package … is bound in the local namespace."
`_handle_fromlist` in `_bootstrap.py` — "Backwards-compatibility dictates we
ignore failed imports triggered by fromlist for modules that don't exist"
(when the sys.modules entry is not None).

**[A6]** `_load` in `_bootstrap.py` — bare `except:` →
`del sys.modules[spec.name]; raise`.

**[A7]** https://peps.python.org/pep-0562/ — "`__getattr__` is searched in
the module `__dict__` [only after] normal lookup, i.e.
`object.__getattribute__()`, fails"; and the compiler-critical caveat:
"directly accessing the module globals … is unaffected."
https://docs.python.org/3.12/reference/datamodel.html#customizing-module-attribute-access.

**[A8]** https://docs.python.org/3.12/c-api/module.html — multi-phase modules
"are not singletons: if the *sys.modules* entry is removed and the module is
re-imported, a new module object is created"; `PyState_FindModule` returns a
borrowed reference and "will not work on modules created using multi-phase
initialization." https://github.com/python/cpython/blob/3.12/Python/import.c —
"To prevent initializing an extension module more than once, we keep a static
dictionary 'extensions' …"; `_PyImport_FixupExtensionObject` ("a copy of the
module's dictionary is stored … immediately after the module initialization
function succeeds"); `import_find_extension` re-creates the module and
`PyDict_Update(mdict, m_copy)` for `m_size == -1` without re-running init.

**[A9]** `PyImport_ImportModuleLevelObject` in import.c —
`"module name must be a string"`; `_sanity_check` in `_bootstrap.py` —
`TypeError(f'module name must be str, not {type(name)}')`, plus
`ValueError('level must be >= 0')`, `ValueError('Empty module name')`,
`ImportError('attempted relative import with no known parent package')`.

**[A10]** `_ModuleLock` / `_get_module_lock` / `_blocking_on` /
`_lock_unlock_module` in `_bootstrap.py` — per-module recursive locks with
deadlock detection; `_lock_unlock_module` catches `_DeadlockError`:
"Concurrent circular import, we'll accept a partially initialized module
object." `import_ensure_initialized` in import.c — waits only while
`__spec__._initializing`. (Margin note: CPython main replaced this with
hierarchical module locking in 2025, GH-83065/GH-137196 — post-3.12; a
planned re-pin point, see §13.3.)

**[A11]** https://docs.python.org/3.12/c-api/sys.html#c.PySys_GetObject —
"Return the object *name* from the `sys` module or `NULL` if it does not
exist, without setting an exception." Borrowed reference; Stable ABI.

**[B1]** https://go.dev/ref/spec#Package_initialization and
#Program_initialization — quotes in §9.
https://github.com/golang/go/blob/master/src/cmd/link/internal/ld/inittask.go —
"If package p imports package q, then package p's inittask record will have a
R_INITORDER relocation pointing to package q's inittask record"; `inittasks()`
computes the dependency-respecting lexicographic schedule.
https://github.com/golang/go/blob/master/src/runtime/proc.go — `initTask`
(`state uint32 // 0 = uninitialized, 1 = in progress, 2 = done`), `doInit`,
`doInit1` (`case 1: throw("recursive call during initialization - linker
skew")`).

**[B2]** https://www.graalvm.org/latest/reference-manual/native-image/optimizations-and-performance/ClassInitialization/ ;
https://www.graalvm.org/latest/reference-manual/native-image/basics/ ("values
written to static fields by this code are saved in the image heap"; "when
native image starts up, it copies the initial image heap from the binary");
https://www.graalvm.org/release-notes/JDK_21/ (`--strict-image-heap`
introduced); https://www.graalvm.org/release-notes/JDK_22/ (made default).

**[B3]** https://docs.micropython.org/en/latest/reference/manifest.html ;
https://github.com/micropython/micropython/blob/master/py/frozenmod.c
(`mp_find_frozen_module` linear `memcmp` scan of `mp_frozen_names[]`);
https://github.com/micropython/micropython/blob/master/py/builtinimport.c
(`MP_FROZEN_PATH_PREFIX ".frozen/"`, `stat_path`, sys.modules-equivalent
`mp_loaded_modules_dict` checked first in `process_import_at_level`).

**[B4]** https://github.com/exaloop/codon/blob/develop/docs/start/faq.md ;
docs/language/overview.md ; docs/integrations/python/python-from-codon.md
(`from python import`, `CODON_PYTHON`, `pyobj` refcount handling). UNVERIFIED
inference flagged in §9.

**[B5]** https://gregoryszorc.com/docs/pyoxidizer/main/oxidized_importer_packed_resources.html —
global header + blob index + resources index + blobs; "the index data is at
the beginning so a reader only has to read a contiguous slice"; consumed by
the `OxidizedFinder` meta-path finder.

**[B6]** https://github.com/WebAssembly/tool-conventions/blob/main/DynamicLinking.md —
`dylink.0` custom section; loader reserves memory/table regions and assigns
`env.__memory_base` / `__table_base`; shared
`env.__indirect_function_table` across modules.

**Repo evidence (live tree, main @ 781a18202):**
`runtime/molt-runtime/src/builtins/modules.rs` (import inner loop 830–1035;
sys.modules vivification 1879–1915; cache_set/first-init-wins/replay
1954–2117; import_from 2510–2584; get_global 2586–2740);
`runtime/molt-runtime/src/builtins/exceptions.rs:303` (module store's current
home); `runtime/molt-runtime/src/builtins/sys_ext.rs:1162` (empty-dict
`molt_sys_modules`); `runtime/molt-runtime/src/cpython_abi_hooks.rs:428–458,
550` (cache-first `sys_module_attr_borrowed`, `hook_import_module`);
`src/molt/cli/backend_ir.py:409–494` (string_eq isolate dispatcher), 686–840
(static-native init ops incl. the init-exactly-once preamble comment);
`src/molt/cli/module_graph.py` + `external_native.py:1264`
(`import_scan_mode="module_init"` C-scan);
`runtime/molt-backend-wasm/src/wasm/module_abi/poll_table.rs:75`;
`wasm/loader_bridge.js:302–320` (table-base drift check);
`docs/spec/areas/compat/contracts/import_system_contract.md` (0213);
commits `c6216a2d0`, `7f4d32934` (import-custody rework), `781a18202`
(init-exactly-once preamble).
