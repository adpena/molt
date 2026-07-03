# Import Bedrock PR2 Cutover: sys.modules Table View

**Status:** PR2 preparation map. Runtime implementation is blocked until the
R1 native call lane lands and the import/bootstrap/module-state files are
released by the orchestration board.
**Authority:** `69_import_bedrock_frozen_module_layer.md`, especially PR2,
invariants I3/I4/I8, gates G2/G8, and the `sys.modules` dict-view design in
section 4.4.
**Scope:** make `ModuleTable` the only module store and make `sys.modules` a
real dict-shaped view over it. This document is an execution map, not a second
design authority.

## Non-Negotiables

- No runtime edits while the board marks the import/bootstrap/module-state lane
  frozen or in surgery.
- No compatibility shim. PR2 deletes the legacy module store in the same arc
  that wires the table-backed view.
- No second Python, Rust, or backend cache of module objects. Module state lives
  in `ModuleTable` plus the view overflow lane.
- No generated consumer may reference a symbol that the same edit did not
  generate or preserve.
- PR2 proves both structure and behavior: the store split must be impossible to
  reintroduce silently, and CPython-visible `sys.modules` mutations must be
  observed by the import transaction without a replay pass.

## Current PR1 Bridge Inventory

These are the bridge surfaces PR2 must delete, replace, or invert into gates.

| Surface | Current owner | PR2 action |
|---|---|---|
| `RuntimeState.module_cache: Mutex<HashMap<String, u64>>` | `runtime/molt-runtime/src/state/runtime_state.rs` | Delete the field and initializer. `ModuleTable` becomes the only module-object store. |
| `exceptions::internals::module_cache` | `runtime/molt-runtime/src/builtins/exceptions.rs` | Delete the accessor and all import-layer dependence on the exceptions module. |
| `molt_module_cache_get` | `runtime/molt-runtime/src/builtins/modules.rs` | Delete the exported cache read. Dynamic import paths resolve string to `ModuleId` and call the table/view APIs. |
| `molt_module_cache_set` | `runtime/molt-runtime/src/builtins/modules.rs` | Delete replay, first-init-wins, and mirror sync. Publication is owned by `molt_module_ensure`; Python `sys.modules[name] = obj` is owned by the view replace entry point. |
| `molt_module_cache_del` | `runtime/molt-runtime/src/builtins/modules.rs` | Delete mirror deletion. Python `del sys.modules[name]` is owned by the view tombstone entry point. |
| `sys_modules_dict_bits` / `sys_modules_dict_ptr` | `runtime/molt-runtime/src/builtins/modules.rs` | Stop auto-vivifying a plain dict as the import store. The sys module exposes the singleton table view. |
| `legacy_cache_lookup`, `legacy_cache_set`, `legacy_cache_del` | `runtime/molt-runtime/src/builtins/module_table.rs` | Delete the PR1 bridge helpers. Table state must not consult or backfill a legacy map. |
| `publish_from_cache_set`, `unpublish_from_cache_del` | `runtime/molt-runtime/src/builtins/module_table.rs` | Delete once init publication and Python dict mutations enter the table directly. |
| `module_table_view_replace`, `module_table_view_tombstone` | `runtime/molt-runtime/src/builtins/module_table.rs` | Promote from PR1 seam to production mutation entry points called by the dict-view write/delete primitives. |
| `molt_sys_modules()` empty-dict intrinsic | `runtime/molt-runtime/src/builtins/sys_ext.rs` | Return the singleton table-backed dict view, not a fresh materialized dict. |
| `PySys_GetObject` cache-first path | `runtime/molt-runtime/src/cpython_abi_hooks.rs` | Resolve `sys` and its attributes through the same table/view authority; no cache fallback or import retry loop. |
| `hook_import_module` | `runtime/molt-runtime/src/cpython_abi_hooks.rs` | Validate the C API payload, resolve string to `ModuleId`, enter `molt_module_ensure`, and fail closed for non-admitted names. |
| `module_cache_get/set` backend IR ops | `src/molt/cli/backend_ir.py` | Stop emitting cache probes and publication ops. Generated init bodies execute only through the init table/ensure path. |
| PR1 structural gate allowances | `tests/test_module_registry_gates.py` | Invert allowances: `publish_from_cache_set` and `unpublish_from_cache_del` become forbidden, and no legacy cache op may survive in native backend IR. |
| PR1 identity regression text | `tests/test_native_import_bootstrap_regressions.py` | Replace "PR2 later" expectations with table-view identity and mutation behavior. |

Secondary readers of the legacy cache must be audited in the same PR2 arc:
`runtime/molt-runtime/src/call/dispatch.rs`,
`runtime/molt-runtime/src/object/ops_sys.rs`,
`runtime/molt-runtime/src/object/weakref.rs`,
`runtime/molt-runtime/src/builtins/functions/function_abi.rs`,
`runtime/molt-runtime/src/builtins/frames.rs`,
`runtime/molt-runtime/src/builtins/platform.rs`,
`runtime/molt-runtime/src/builtins/platform_importlib_ffi/reload_bootstrap.rs`,
`runtime/molt-runtime/src/builtins/platform_importlib_ffi/find_spec.rs`,
`runtime/molt-runtime/src/builtins/modules/runpy.rs`,
`runtime/molt-runtime/src/state/lifecycle.rs`, and
`runtime/molt-runtime/src/builtins/platform_importlib_support.rs`. Each use
must either become a table/view read or stop depending on module storage.

## Dict-View Integration Surface

The view belongs at the dict storage layer, not in import-specific wrappers.
Current hot primitives and bypass candidates are:

- `runtime/molt-runtime/src/object/ops/dict_set_tables.rs`
  - `dict_set_in_place`, `dict_set_inline_int_in_place`,
    `dict_set_in_place_preserving_pending`, `dict_update_set_via_store`
  - `dict_get_in_place`, `dict_get_inline_int_in_place`,
    `dict_get_str_bytes_borrowed`
  - `dict_del_in_place`, `dict_clear_in_place`,
    `dict_clear_in_place_shutdown`
- `runtime/molt-runtime/src/object/ops_dict.rs`
  - public externs `molt_dict_set`, `molt_dict_get`,
    `molt_dict_setdefault`, `molt_dict_clear`, `molt_dict_copy`,
    `molt_dict_getitem_borrowed`, `molt_dict_update`
- `runtime/molt-runtime/src/builtins/containers.rs`
  - `dict_order_ptr`, `dict_table_ptr`, `dict_hashes_ptr`, `dict_order`,
    `dict_table`, `dict_hashes`, `dict_len`
  - dict view object helpers `dict_view_dict_bits`, `dict_view_len`,
    `dict_view_entry`, `dict_view_as_set_bits`
- `runtime/molt-runtime/src/object/builders.rs`
  - `alloc_dict_with_pairs`, which allocates today's materialized dict layout.
- direct `dict_order` consumers in iteration, formatting, equality, arithmetic
  union, list conversion, GC, weakref, and attribute helper paths.

PR2 adds one dict backing tag with two storage modes:

- `Materialized`: today's storage, including the existing `order/table/hash`
  vectors.
- `ModuleTableView`: the singleton `sys.modules` object. Registry names are
  backed by `ModuleTable`; non-registry names live in the overflow lane.

The primitive contract is:

- `get(k)`: for string registry names, return the visible table value for
  `Initializing`, `Ready`, or `Replaced`; return no value for `Uninit` or
  `Tombstone`; return the stored `None` object for `Replaced(None)`. For
  non-registry names, read overflow.
- `set(k, v)`: for string registry names, call
  `module_table_view_replace(id, v)`; for non-registry names, write overflow.
- `del(k)`: for string registry names, call
  `module_table_view_tombstone(id)` and preserve CPython-shaped miss behavior;
  for non-registry names, delete overflow.
- `len`, `iter`, `keys`, `values`, `items`, `copy`, and dict views merge
  visible registry rows with overflow through a deterministic order journal.
- `clear` tombstones visible registry rows and clears overflow; it must not
  deallocate the table.
- `type(sys.modules) is dict` stays true. Rebinding the `sys.modules`
  attribute itself does not replace the import machinery store.

The implementation should keep one primitive dispatch point for the backing
tag. Public externs and methods must call the primitives; they must not learn
about `ModuleTableView` independently.

## Cutover Sequence

PR2 is one coherent store cutover. The order below is the implementation
sequence inside that arc, not permission to land partial states.

1. Add the dict backing tag and materialized helpers with zero behavior change
   for ordinary dicts.
2. Allocate the singleton `ModuleTableView` for `sys.modules` during sys/module
   bootstrap and make `molt_sys_modules()` return it.
3. Route dict primitive reads, writes, deletes, len, iteration, copy, and view
   object construction through the backing tag.
4. Move import readers to table/view reads: `molt_module_import_inner`,
   `import_from_sys_modules_lookup`, `molt_module_import_from`,
   `molt_module_get_global`, `PySys_GetObject`, `PyImport_ImportModule`,
   runpy/platform/importlib support, lifecycle cleanup, and secondary runtime
   callers.
5. Move import writers to the transaction: `molt_module_ensure` owns init
   publication and rollback; the dict view owns replace/tombstone. Delete
   replay, first-init-wins, mirror sync, and sys.modules auto-vivification.
6. Remove backend IR cache ops and init preambles from native generation.
   Generated module init bodies must have no module-cache probes or
   publication ops.
7. Delete the legacy store field, exceptions accessor, cache exports,
   legacy bridge helpers, and tests that only prove the old bridge.
8. Strengthen G2/G8 gates and update the import contract/status docs in the
   same commit as the implementation.

## Acceptance Gates

Structural gates:

- No `module_cache: Mutex<HashMap<String, u64>>` field exists in runtime
  state.
- No `exceptions::internals::module_cache` accessor exists.
- No `molt_module_cache_get`, `molt_module_cache_set`, or
  `molt_module_cache_del` export exists.
- No `legacy_cache_*`, `publish_from_cache_set`, or
  `unpublish_from_cache_del` helper exists.
- Native backend IR generation emits no `module_cache_get`,
  `module_cache_set`, or `module_cache_del` op.
- The only registry-name mutation entry points for Python `sys.modules`
  writes are `module_table_view_replace` and `module_table_view_tombstone`.
- Dict public methods and externs route through the tagged primitive funnel.
  A structural scan must reject new direct `dict_order`/table-vector fast paths
  that can observe `TYPE_ID_DICT` without handling `ModuleTableView`.

Behavioral gates:

- `sys.modules` is a dict object, has stable identity, and is the object
  returned by `molt_sys_modules()`.
- `sys.modules[name] = sentinel` for a registry name makes a later import of
  that name return the sentinel value when CPython would accept the cached
  value.
- `sys.modules[name] = None` for a registry name makes import raise
  `ModuleNotFoundError` with the CPython 3.12-shaped message.
- `del sys.modules[name]` tombstones a registry row and source reimport
  creates the CPython-shaped replacement behavior governed by the table state
  machine.
- `sys.modules.copy()`, `keys()`, `values()`, `items()`, iteration, `len`,
  membership, and dict view objects reflect visible registry rows plus
  overflow in deterministic order.
- Overflow entries for non-registry names are visible through sys.modules dict
  operations and import-from recovery where the closed-world policy allows the
  name; they do not bypass build-time admission.
- Rebinding the `sys.modules` attribute on the sys module does not replace the
  import transaction store.
- Failure during module body execution rolls back the table row and removes
  the visible sys.modules entry without a replay pass.

Proof lanes:

- `tests/test_module_registry_gates.py` owns the PR2 structural scans.
- `tests/test_native_import_bootstrap_regressions.py` owns focused native
  table-view behavior.
- Differential rows for CPython 3.12 import-cache semantics belong in the
  existing importlib/fromlist differential cluster and must run through
  `tests/molt_diff.py --jobs 1` when PR2 claims behavior parity.
- Expensive cargo/differential proof must use `tools/proof_queue.py` per
  `docs/agent/PROOF_QUEUE.md`; direct commands are only for cheap static/doc
  checks and queue bootstrap.

## Recursive Review Checklist

- A Python-level sys.modules mutation and a Rust-level ensure read cannot
  disagree, because they hit the same row.
- A registry row cannot be visible in one iterator/copy path and absent in
  another, because all dict views merge through one order journal.
- A generated init body cannot republish or skip publication, because the only
  publication authority is the ensure transaction.
- A C extension import hook cannot resurrect the old cache, because
  `PyImport_*` and `PySys_*` enter through table/view APIs only.
- A future dict fast path cannot bypass the view silently, because the G2 scan
  names every `TYPE_ID_DICT` fast path that reads raw dict vectors.
- A stale backend generator cannot keep old cache ops alive, because the
  structural gate scans the generated native IR operation family.
- A wasm/module_abi change is not smuggled into PR2. PR3 owns registry-derived
  wasm projections after the in-flight module_abi lane lands.
