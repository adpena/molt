# Native default import finders (HF-737)

**Status:** design, reviewed 2026-11-04 (one round, two reviewers; record in section 11).
**Owner contract:** [import system contract](../../spec/areas/compat/contracts/import_system_contract.md), section 1.
**Related:** [frozen module layer](import_bedrock_frozen_module_layer.md), HF-693 (finder code size), HF-297 (loader details).

## 1. Problem

At bootstrap the runtime seeds `sys.meta_path = [BuiltinImporter]` and
`sys.path_hooks = []` (`runtime/molt-runtime/src/builtins/modules.rs`,
`sys_populate_bootstrap_metadata`). CPython's `PathFinder` and its two path
hooks are Python classes in `src/molt/stdlib/importlib/machinery.py`. They do
not exist until that body runs. So the first meta-path walk
(`importlib_find_spec_impl`, `platform_importlib_ffi/find_spec.rs:78`) first
calls `importlib_complete_initial_defaults` (`reload_bootstrap.rs:342`). That
function executes the machinery body, and the body calls
`molt_importlib_ensure_default_meta_path(globals())`. The intrinsic appends
`PathFinder` and the default hooks, but only while each list keeps its bootstrap
backing epoch (`extend_if_epoch`, `object/list_mutation.rs:1015`; cancellation
at `reload_bootstrap.rs:565-581`).

This has two defects.

- **Cost.** Every program with a cold import runs the machinery body, even when
  the catalog finder (`BuiltinImporter`) serves the module. The HF-737 ledger row
  attributes 15.2M instructions to the machinery body (macOS arm64 release,
  2026-11-02 profile; 36% of it is function setup). Step 1 (8d3e1e3147) already
  removed the eager `importlib.util` import, so the remaining admission cost is
  the machinery body plus what that body imports. The A/B in section 8 measures
  base and head on one base commit; older ledger numbers come from different
  commits and are context only.
- **Semantics.** `sys.meta_path` before the first import holds only
  `BuiltinImporter`; CPython holds three finders. A program that mutates
  `sys.meta_path` or `sys.path_hooks` before its first import never gets
  `PathFinder` or the default hooks, because the epoch check cancels
  completion. A later trigger cannot fix this: design reviews rejected it because
  it loses `PathFinder` after `sys.meta_path.append(F)`. A microbinary without the
  machinery source never gets `PathFinder` at all
  (`initial_machinery_capability`, `reload_bootstrap.rs:335`).

## 2. Prior art

- **CPython 3.12.15.** `pyinit_core` runs `_bootstrap._install`
  (`Lib/importlib/_bootstrap.py:1538-1543`), which appends `BuiltinImporter` and
  `FrozenImporter`. `init_importlib_external` then runs
  `_bootstrap._install_external_importers` (`_bootstrap.py:1546`), which calls
  `_bootstrap_external._install` (`Lib/importlib/_bootstrap_external.py:1744-1749`):
  `sys.path_hooks.extend([FileFinder.path_hook(*supported_loaders)])` and
  `sys.meta_path.append(PathFinder)`. `_PyImportZip_Init`
  (https://github.com/python/cpython/blob/v3.12.15/Python/import.c) inserts
  `zipimport.zipimporter` at `sys.path_hooks[0]`. All of this runs from frozen
  code before user code, so no user mutation can precede it. The loader classes
  are frozen too, so `FileFinder.path_hook` captures them at bootstrap.
  `PathFinder` is at `_bootstrap_external.py:1438-1559`; `FileFinder.path_hook`
  at `:1680-1697`; `_NamespacePath._epoch` at `:1334` and its increment in
  `PathFinder.invalidate_caches` at `:1455`.
- **CPython 3.13+.** `_find_spec` copies `meta_path` before the walk
  (gh-130094); the import contract cites the pinned lines.
- **Bun and Deno.** Module resolution is native code in the runtime; no script
  runs to install the resolver.
- **GraalVM Native Image and Go.** Initialization that the build can fix is
  fixed at build or link time (family W in `docs/design/findings_prior_art.md`).

Adopt CPython's ordering: the default finders and hooks exist before any user
code, so nothing completes them later. Adopt Bun's mechanism: the finders are
native, so seeding them runs no script.

## 3. Mechanism

### 3.1 Classes

The runtime owns two more heap classes. Their method bodies live in
`runtime/molt-runtime/src/builtins/platform_importlib_ffi/path_finder.rs`, next
to the helpers they call (`importlib_resolve_spec_in_paths`,
`importlib_find_spec_object_bits`, `importlib_enforce_extension_spec_object_boundary`).
The classes are built in `builtins/types/compiled_loader.rs`, beside
`BuiltinImporter`, and cached in two new slots of the `types` runtime state
(`path_finder_class`, `file_finder_class`), built once by
`init_cached_runtime_class_configured` with `ClassSemanticPolicy::heap(false, true)`
(mutable, subclassable, instances carry a `__dict__`), as `BuiltinImporter` is.

| Class | `__module__` | Members |
| --- | --- | --- |
| `PathFinder` | `_frozen_importlib_external` | `find_spec`, `_get_spec` (`classmethod` objects), `invalidate_caches` (`staticmethod` object) |
| `FileFinder` | `_frozen_importlib_external` | `__init__(self, path, *loader_details)`, `__repr__`, `find_spec(self, fullname, target=None)`, `invalidate_caches(self)` (method descriptors); `path_hook` (`classmethod` object) |

Each `classmethod`/`staticmethod` member wraps a `Function`-kind native callable
in a real `classmethod`/`staticmethod` object (`alloc_classmethod_obj`,
`alloc_staticmethod_obj`, `object/builders.rs:2030,2045`), so
`type(PathFinder.__dict__["find_spec"]) is classmethod` as in CPython.

`FrozenImporter` gains `find_spec` as a `classmethod` object that returns
`None`: Molt has no frozen bytecode, and the catalog serves CPython's frozen
names through `BuiltinImporter`.

A new intrinsic `molt_importlib_path_finder_types()` returns
`(PathFinder, FileFinder)`. `importlib.machinery` binds both from it, as it binds
`BuiltinImporter` from `molt_importlib_compiled_loader_types`.

### 3.2 Bootstrap seeding

`sys_populate_bootstrap_metadata` seeds, in CPython order:

- `sys.meta_path = [BuiltinImporter, FrozenImporter, PathFinder]`
- `sys.path_hooks = [zip_hook, FileFinder.path_hook()]`

**`zip_hook`** is a native function (`__name__` `_zip_path_hook`). For a `str`
path it applies today's rule: lower-case the path, map `\` to `/`, and raise
`ImportError("not an archive path")` unless it ends with or contains a `.zip`,
`.whl` or `.egg` segment. A non-`str` path raises the `TypeError` or
`AttributeError` today's `path.lower()` raises, through the same attribute call.
An archive path imports `zipimport` through the ordinary import transaction
(`importlib_import_module_impl` in `import_transaction.rs`) and returns
`zipimporter(path)`. `machinery.py:598` was the compiler's only static edge to
`zipimport`, so `zipimport` joins `_RUNTIME_IMPORT_SUPPORT_ROOT_MODULES`
(`src/molt/cli/module_import_scanner.py:32`). The closure of an importing
program keeps the same module set as today.

**`FileFinder.path_hook(*details)`** returns a native function
(`__name__` `path_hook_for_FileFinder`) whose owner-context slot
(`FunctionCallAbi::OpaqueContextFirst`) holds the tuple `(cls, details)`. It
raises `ImportError("only directories are supported", path=path)` unless `path`
is a `str` for which `molt_path_isdir` returns `True`, and otherwise returns
`cls(path, *details)`. `molt_path_isdir` is the same native predicate that
today's `os.path.isdir` call reaches: it admits operation `OsStat`, a refusal
propagates as `PermissionError`, and an ordinary OS error means "not a
directory".

The seeded default hook passes no details, so cached default finders have
`_loader_details == ()` (today a 3-tuple of Python loader classes, which do not
exist at bootstrap). `_loader_details` is a Molt-private attribute and lookup
ignores it today. When HF-297 makes lookup honor explicit details, an empty
tuple means "the declared default loaders" (section 3.4).

### 3.3 Method bodies

The bodies port today's Python bodies in `importlib/machinery.py`. Each rule
below is explicit because a port can lose it silently.

- **Fresh reads.** Every use reads `sys.path_importer_cache`, `sys.path_hooks`
  and `sys.path` from the interpreter `sys` module again, as the Python body
  does. No borrowed pointer or Rust reference is held across a callback; each
  held value has an owned reference (`ExceptionValue`).
- **Iteration.** `path` and `sys.path_hooks` are iterated with Python's live
  iterator protocol (`OwnedIterator`), so a hook that mutates them sees the same
  effects as in Python.
- **`PathFinder._get_spec(cls, fullname, path, target=None)`.** For each
  entry: skip non-`str`. Map `""` to the current directory: admit operation
  `OsGetcwd`, then read the directory; a `NotFound` error skips the entry and
  caches nothing, and any other error raises the `OSError` that `os.getcwd`
  raises. Look up `sys.path_importer_cache[entry]` with a generic subscription
  that catches only `KeyError` (`lookup_namespace_item`). On a miss, call each
  hook in order; only `ImportError` rejects a hook; store the result, `None`
  included, with a generic item assignment. Skip a `None` finder. Call
  `finder.find_spec(fullname, target)` through attribute lookup; skip a `None`
  spec; apply extension admission
  (`importlib_enforce_extension_spec_object_boundary`); return a spec whose
  `loader` is not `None`; raise `ImportError("spec missing loader")` when
  `submodule_search_locations` is `None`; else extend the portion list. At the
  end return `ModuleSpec(fullname, None)` with `submodule_search_locations` set
  to the portion list.
- **`PathFinder.find_spec(cls, fullname, path=None, target=None)`.** Use
  `sys.path` when `path` is `None`. Call `cls._get_spec(fullname, path, target)`
  through attribute lookup, so subclass overrides apply. Return `None` when it
  returns `None` (CPython `_bootstrap_external.py:1532`). Return a spec with a
  loader. Return `None` for an empty portion list. Otherwise set `origin` to
  `None` and wrap the portions in `_NamespacePath(fullname, portions,
  cls._get_spec)` (section 3.4).
- **`PathFinder.invalidate_caches()`.** Take a list snapshot of
  `sys.path_importer_cache.items()`. For each pair: delete the key when the
  finder is `None`; else delete it when the key is not an import root; else call
  the finder's `invalidate_caches` when it has one. An exception stops the loop
  and propagates; earlier effects stay; the epoch does not advance. After the
  loop, increment the namespace epoch. The root rule is one pure Rust function
  with a flavor parameter. The POSIX flavor calls the key's `startswith("/")`
  method, so a bytes key raises `TypeError` and a non-string key raises the
  error its missing method raises, as today. The Windows flavor applies today's
  facade rule over the native `path_splitroot` components (drive plus root,
  `/` mapped to `\`, absolute when longer than one character and either UNC or
  ending in a separator; bytes and str both). The flavor comes from the runtime
  value of `sys.platform == "win32"`, not from `cfg`, so a macOS unit test covers
  both.
- **`FileFinder.__init__(self, path, *loader_details)`** stores
  `self.path = str(path)` and `self._loader_details = tuple(loader_details)`.
  `__repr__` returns `<MoltFileFinder path=...>` as today.
- **`FileFinder.find_spec(self, fullname, target=None)`** reads `self.path`,
  requires `str` arguments as today, admits operation
  `ImportlibFilefinderFindSpec`, and calls `importlib_resolve_spec_in_paths`.
  Only a physical hit builds a spec, through `importlib_find_spec_object_bits`
  with the declared loader classes (section 3.4). `invalidate_caches` returns
  `None`.

### 3.4 Loader classes and `_NamespacePath`

The file loader classes (`ExtensionFileLoader`, `SourceFileLoader`,
`SourcelessFileLoader`, `_ZipSourceLoader`) and `_NamespacePath` stay Python
classes in `importlib.machinery`. CPython's `FileFinder` captures its loaders
from its defining module, and today's Molt passes the defining `globals()`. A
public `sys.modules` lookup would break that rule (`platform.rs:1336`: "A public
sys.modules spelling is never rediscovered"). So the defining body declares
them:

- The machinery body calls
  `molt_importlib_file_loader_types_declare(ExtensionFileLoader, SourceFileLoader,
  SourcelessFileLoader, _ZipSourceLoader, _NamespacePath)` once. The intrinsic
  requires that the caller is the admitted body (`module_body_owns_globals`, as
  the frozen-external payload does), stores an owned 5-tuple in one platform
  slot (a later execution of the body replaces it, releasing the old tuple),
  and marks `ExtensionFileLoader` with `ClassDeclaration::ExtensionLoader`. It
  replaces `molt_importlib_extension_loader_type_declare`, so that fact keeps
  one authority.
- A consumer that needs the tuple (`FileFinder.find_spec` after a physical hit;
  `PathFinder.find_spec` for a namespace result; `compiled_namespace_spec` for a
  catalog namespace package) reads the slot. When it is empty, it runs the
  admitted machinery body with the sequence that `importlib_complete_initial_defaults`
  uses today: hold `ModuleExecutionGuard` for the provider; when the public
  cache has no entry, import through `catalog_import_attempt`; when the slot is
  still empty (the public entry is `None` or a stand-in), execute the body
  privately (`execute_compiled_module`). Either execution declares. If the
  catalog has no machinery provider, or the slot is still empty, the consumer
  raises the existing `RuntimeError("runtime support module unavailable:
  importlib.machinery")`.

So a catalog hit of an ordinary module, a cached hit, a miss and any custom
finder never run the machinery body. A catalog namespace package does, as today.

The namespace epoch moves to one native counter in the platform state
(`AtomicU64`, `Relaxed`: the GIL orders every reader and writer).
`_NamespacePath` reads it through the new intrinsic
`molt_importlib_namespace_path_epoch()` in `__init__` and `_recalculate`. The
`_NamespacePath._epoch` class attribute goes away; it is a CPython-private
counter (bpo-45703), recorded as a deviation in section 4.

### 3.5 Retirement

The import-lock refusal after shutdown keeps its behavior and message
("interpreter import state is retired"). The state becomes one `AtomicBool`
(`import_retired`) in the platform state. `platform_clear_runtime_state` stores
`true` with `Release` ordering at the point where it retires the default
generation today: before it releases the environment mapping and the object
slots, so no finalizer can import. `importlib_acquire_import_lock` loads it
with `Acquire` before and after taking the lock. The flag holds no object, so it
adds nothing to the function's returned "released owners" bit; the declared
loader tuple does.

## 4. Settled decisions

**CPython-visible identity.**
- `[(f.__module__, f.__qualname__) for f in sys.meta_path]` before any import
  equals CPython 3.12: `_frozen_importlib.BuiltinImporter`,
  `_frozen_importlib.FrozenImporter`, `_frozen_importlib_external.PathFinder`.
- `importlib.machinery.PathFinder is sys.meta_path[2]`, and
  `_frozen_importlib_external.PathFinder is importlib.machinery.PathFinder`.
- `type(sys.path_hooks[1](d)) is importlib.machinery.FileFinder` for a
  directory `d`, so `isinstance` holds.
- `type(PathFinder.__dict__[name])` is `classmethod` for `find_spec` and
  `_get_spec`, `staticmethod` for `invalidate_caches`; `FileFinder.path_hook`
  is a `classmethod`.
- Known deviations that this change keeps: `sys.path_hooks[0]` is Molt's lazy
  archive hook, not `zipimport.zipimporter`; `PathFinder` lacks
  `find_distributions`, `_path_hooks` and `_path_importer_cache`; the empty
  `sys.path_hooks` warning is not emitted; `FileFinder` ignores explicit
  `loader_details` in lookup (HF-297) and keeps Molt's `repr`.
- Known deviations that this change adds, each Molt-private or cosmetic: the
  native hook functions are `builtin_function_or_method` objects, not Python
  functions (their `__name__` matches CPython; `__qualname__` and `repr` follow
  Molt's native callables); cached default finders have
  `_loader_details == ()`; `_NamespacePath._epoch` is gone.
- Gained: `FrozenImporter.find_spec` exists; programs that mutate the finder
  lists before their first import keep `PathFinder` and the default hooks;
  microbinaries have `PathFinder`.

**What `importlib.machinery` imports and re-exports.** It binds `PathFinder`
and `FileFinder` from `molt_importlib_path_finder_types()`. It deletes
`_default_path_hooks`, `_zip_path_hook`, its own `PathFinder` and `FileFinder`
bodies, the `_MOLT_IMPORTLIB_ADMIT_SPEC`, `_MOLT_IMPORTLIB_FILEFINDER_FIND_SPEC`
and `_MOLT_PATH_SPLITROOT` bindings, and the call to
`molt_importlib_ensure_default_meta_path`. It declares its loaders (section 3.4).
`__all__` is unchanged. `_frozen_importlib_external` receives both classes from
the runtime through its existing payload (`payloads.rs`), which now reads the
runtime classes instead of the facade; `_frozen_importlib_external.py` itself
does not change.

**The 3.13+ copy of `meta_path`** (`find_spec.rs:97-104`) is unchanged. The walk
no longer runs a completion step first, so the copy sees exactly the list the
bootstrap seeded or the user mutated, as in CPython.

**Molt-only errors.** The change deletes seven Molt-only error sites ("initial
import finders failed to initialize", "admitted machinery initializer is
unavailable", "admitted machinery body did not complete initial finders",
"default finder producer has no admitted source body", "default finder producer
does not own its defining globals", "default path hooks must be a list",
"default path hooks snapshot allocation failed"). It adds no user-facing error.
Three internal refusals guard runtime invariants that no Python program can
violate through the public API: the declare intrinsic refuses a caller that is
not the admitted machinery body (it replaces the deleted "does not own its
defining globals" refusal) or a declared name that is not a class, and the
Windows root rule refuses a malformed split. One existing user-reachable site
stays: `RuntimeError("runtime support module unavailable:
importlib.machinery")` when a consumer in section 3.4 needs the loaders and the
catalog has no machinery body. The compiler admits machinery whenever reached
Python code imports or calls an import entry point
(`_RUNTIME_IMPORT_SUPPORT_ROOT_MODULES`). The public import transaction is
reached only from compiled import sites and import calls (both scanned) and from
C-API imports of an admitted extension (whose program imports it). A typed,
site-naming error for excluded finder code belongs to HF-693, which owns dropping
that code. No new authority is needed.

**Capability outcomes.** A catalog miss now reaches `PathFinder` in every
program. Its hook calls `molt_path_isdir`, so under a tier that refuses `os.stat`
a miss raises `PermissionError` instead of `ModuleNotFoundError`. Every program
that admits machinery has this behavior today. A program with no import
statement gains it; its earlier `ModuleNotFoundError` came from the missing
`PathFinder`, not from a decision about capabilities.

## 5. Invariants

1. `sys.meta_path` and `sys.path_hooks` hold their defaults from the moment
   `sys` exists. No later code appends, repairs or cancels them.
2. A catalog hit of a module, a cached hit, a miss and any custom finder never
   execute `importlib.machinery`. Only a filesystem hit or a namespace package
   acquires it (section 3.4).
3. One class identity per finder: the runtime class is the object in
   `sys.meta_path`, in `importlib.machinery` and in `_frozen_importlib_external`.
4. Finder callbacks run under the same import lock and ordering as today. The one
   new ordering is the on-demand machinery execution of section 3.4, which runs
   at the consumer's point, after the finder that produced the hit returned.
5. Every path-entry, cache and hook operation happens in the order of today's
   Python body; each user callback runs once per visit.

## 6. Consumers changed

Paths are relative to the repository root; `R` is `runtime/molt-runtime/src`.

Deleted with the replaced lane, in the same change:
- `R/builtins/platform_importlib_ffi/reload_bootstrap.rs`:
  `importlib_seed_default_generation`, `importlib_take_default_generation`,
  `importlib_retire_default_generation`, `importlib_complete_initial_defaults`,
  `importlib_finish_initial_defaults`, `pending_default_owners`,
  `release_default_owners`, `initial_machinery_capability` (its provider lookup
  moves to section 3.4), `molt_importlib_ensure_default_meta_path`.
- `R/builtins/platform_importlib_ffi.rs`: their re-exports.
- `R/builtins/platform.rs`: `DefaultFinderGeneration`, `PendingDefaultFinders`,
  `import_defaults`, `ImportDefaultsTestSnapshot`, `importlib_machinery_member`,
  `importlib_machinery_loader_instance`; their users in `R/test_support.rs`
  (`EmptyImportResolver`) and `R/builtins/module_table.rs`
  (`InterpreterSysTestSnapshot`).
- `R/object/list_mutation.rs`: `extend_if_epoch` (its only caller is deleted).
- `R/builtins/platform_importlib_ffi/find_spec.rs`: the completion call,
  `molt_importlib_filefinder_find_spec` and `molt_importlib_admit_spec`.
- `R/builtins/platform_importlib_ffi/bootstrap_loader.rs`:
  `molt_importlib_extension_loader_type_declare` (folded into the new declare).
- `runtime/molt-ir/src/tir/runtime_names.toml`: `PrivateDefaultPathHooks`.
- `R/intrinsics/manifest.pyi`: entries for the four deleted intrinsics; every
  generated projection through `tools/generators.py write` (runtime resolvers and
  `generated.rs`, `src/molt/_intrinsics.pyi`, `src/molt/_intrinsic_symbols.py`,
  `runtime/molt-backend-wasm/src/wasm_abi_manifest.toml` and its generated
  tables, `src/molt/_wasm_abi_generated.py`, `wasm/wasm_abi_generated.json`,
  the `molt-ir` runtime callable ABI, `R/state/names_generated.rs`).

Changed:
- `R/builtins/modules.rs`: bootstrap seeding.
- `R/builtins/types/compiled_loader.rs`: `FrozenImporter.find_spec`; the
  `PathFinder` and `FileFinder` classes; `__module__` values by `NameId`.
- `R/builtins/types.rs`: two state slots.
- `R/builtins/platform_importlib_ffi/path_finder.rs` (new): the method bodies,
  both hooks, the epoch, types and declare intrinsics, the loader-tuple
  acquisition and the import-root rule.
- `R/builtins/platform.rs`: `importlib_find_spec_object_bits` takes the typed
  loader tuple and builds `ModuleSpec` from the runtime class;
  `compiled_namespace_spec` uses the tuple and the runtime `PathFinder`;
  platform state gains `import_retired`, `namespace_path_epoch` and the loader
  slot.
- `R/builtins/platform_importlib_ffi/payloads.rs`: the frozen-external payload
  reads the runtime classes.
- `R/builtins/platform_tests.rs`: the default-generation test is deleted; the
  two import-lock retirement tests read `import_retired`; new unit tests for the
  root rule (both flavors, the cases of today's pytest) and seeding.
- `R/builtins/types/runtime_class_policy_tests.rs`: lists the two classes.
- `R/builtins/modules/tests.rs`: the bootstrap test asserts the seeded lists.
- `src/molt/stdlib/importlib/machinery.py`: section 4.
- `src/molt/cli/module_import_scanner.py`: `zipimport` support root; its
  fixture in `tests/cli/test_source_scan_authority_closure.py`.
- `src/molt/stdlib/importlib/_bootstrap_external.py`: forwards attribute reads
  to `_frozen_importlib_external` (PEP 562) instead of star-importing it. The
  eager completion used to import `importlib` before user code, which hid an
  import cycle: `_frozen_importlib_external` imports `importlib.machinery`,
  whose package imports `importlib._bootstrap_external`. Found by the existing
  `importlib_module_spec` differential; `importlib_frozen_external_first` pins
  it.
- `runtime/molt-ir/src/tir/runtime_names.toml`: new names
  (`importlib.machinery`, `_frozen_importlib_external`, `path_importer_cache`,
  `invalidate_caches`, `_loader_details`, `path_hook`, `path_hook_for_FileFinder`,
  `_zip_path_hook`, `zipimport`, `zipimporter`).
- `tests/test_stdlib_importlib_machinery.py`: the stand-in intrinsics change;
  the Python `PathFinder.invalidate_caches` test moves to the Rust unit test.
- `tests/differential/stdlib/importlib_cached_module_identity.py`: its
  `_default_path_hooks` stand-in property goes dead and is removed.
- `tests/differential/stdlib/TESTS.txt`: regenerated with
  `tools/gen_diff_lanes.py --write`.
- `docs/spec/areas/compat/contracts/import_system_contract.md`: the cold-miss
  sentence ("pays admitted initial finder completion") and the completion
  paragraph become the bootstrap-seeding rule.

The WASM manifest needs no new `[[reserved_runtime_callable]]` rows. Those rows
name witness-frontier callables; ordinary native methods use address keys
(`runtime_fn_publish`), as the `types` module classes do.

Not changed: HF-693's size work, the loader classes, `importlib.util`,
`importlib/__init__.py` (it still imports machinery eagerly, as before; a
separate step).

## 7. Cost

- Bootstrap: two class constructions (about ten native function objects), two
  hook functions, two more list entries. No script, no I/O.
- Cached import: unchanged.
- Catalog hit: unchanged walk minus the completion step.
- Catalog miss: one native `FrozenImporter.find_spec` call, then the native
  `PathFinder` walk. Per `sys.path` entry: one cache subscription; on the first
  visit, the hook calls and one admitted `stat`; then one `find_spec` call.
  Today the same walk is Python code.
- Filesystem hit or namespace package: today's cost, plus a one-time machinery
  execution if no body has declared yet.

Pass criterion: `import tiny` drops by at least 10M instructions retired
(the machinery body is 15.2M) with peak RSS not higher; hello world stays within
1% (it has no cold import).

## 8. Phases and test plan

Phase 1 (one atomic change; splitting it leaves zero or two `PathFinder`
entries): names and intrinsics, native classes and hooks, seeding, the
machinery rewrite, the declare, deletion of the completion lane, regeneration.
Gate: `cargo build` and the touched Rust unit tests, `tools/generators.py check`,
then the differentials below on native.

Phase 2: the contract update, LLVM and WASM differentials, the A/B, and
`land --static-only`.

New differentials in `tests/differential/stdlib/`, each run on native, LLVM and
WASM against CPython 3.12:
1. `importlib_default_finders_identity.py`: before any import, the shape of
   `sys.meta_path` and `sys.path_hooks`; each finder's `__module__` and
   `__qualname__`; then an import of a sibling package (the "import tiny from a
   path entry" case) and the same shape after it; `PathFinder` identity across
   `importlib.machinery` and `_frozen_importlib_external`; the `classmethod` and
   `staticmethod` member types; `FrozenImporter.find_spec("x") is None`;
   `isinstance` of a hook result against `importlib.machinery.FileFinder`;
   `importlib.invalidate_caches()` returns `None` and keeps the lists.
2. `importlib_default_finders_append_first.py`: `sys.meta_path.append(F)` and
   `sys.path_hooks.append(h)` before the first import, then a sibling import and
   a missing import; `PathFinder` stays at index 2 and `F` sees the miss.
3. `importlib_default_finders_insert_first.py`: `sys.meta_path.insert(0, F)`
   before the first import, where `F` serves a virtual module; the module
   imports and the default finders follow `F`.
4. `importlib_default_finders_subclass.py`: a `PathFinder` subclass that
   overrides `_get_spec` (its `find_spec` honors it, and a `None` result gives
   `None`); `FileFinder.path_hook` on a subclass returns the subclass;
   monkeypatching `PathFinder.find_spec` changes the next import.

Required existing passes: `importlib_namespace_path_protocol.py` (epoch),
`importlib_find_spec_invalidate_caches_intrinsic.py`,
`importlib_cached_module_identity.py`, `importlib_find_spec_zip_source_intrinsic.py`
and the other importlib differentials on native and WASM; the Rust unit tests
of section 6; pytest of the direct readers of every changed file.

Performance: instructions retired and peak RSS for `import tiny` and hello world,
base and head scratch checkouts built with identical settings, five interleaved
pairs with `/usr/bin/time -l`; binary size recorded.

## 9. Unverified cells

- Windows: the root rule's Windows flavor runs in a macOS unit test only; the
  native `path_splitroot` edge cases need a Windows host.
- Linux, WASI hosts other than the local WASM runner, browser hosts.
- Python targets 3.13 and 3.14 (the differentials run against 3.12).
- A `""` path entry with a deleted current directory, a refused `os.stat` or
  `os.getcwd` tier, and the machinery `None`/stand-in case before a physical
  filesystem hit: specified above, not run in this lane (an AOT build has no
  uncompiled source to hit; the stand-in case with a custom hook is covered by
  `importlib_cached_module_identity.py`).
- Free-threaded builds: they share the import lock; no new lock is added.

## 10. Out of scope

The inline-cache flush on every `def` (revisit after HF-747), `json` through
`marshal` (HF-744), the eager machinery import in `importlib/__init__.py`, and
HF-693's removal of finder code from microbinaries.

## 11. Review record

Round 1, 2026-11-04. Two fresh-context Goldfish reviewers read only this file
(first version): `egm-design` and `egm-implementation-readiness`.

| Finding | Disposition |
| --- | --- |
| Both, blocker: loaders and `_NamespacePath` read through public `sys.modules` | Fixed: the machinery body declares them; acquisition runs the admitted body (section 3.4). |
| Both, blocker: native `isdir` and `getcwd` bypass capability admission | Fixed: `molt_path_isdir` (operation `OsStat`) and operation `OsGetcwd`; outcomes stated (sections 3.2, 3.3, 4). |
| Readiness, blocker: deleting `_zip_path_hook` drops the compiler's edge to `zipimport` | Fixed: `zipimport` joins the support roots; import entry and non-`str` behavior named (section 3.2). |
| Readiness, blocker: `types/` cannot reach private helpers | Fixed: bodies live in `platform_importlib_ffi/path_finder.rs` (section 3.1). |
| Design: catalog namespace packages run machinery (`compiled_namespace_spec`) | Fixed: consumer listed; invariant 2 narrowed (sections 3.4, 5, 6). |
| Design: stale cost attribution; mixed-commit numbers | Fixed: attribution corrected; A/B on one base; pass criterion added (sections 1, 7). |
| Design: "no closed-world program reaches it" unproven | Answered: entry points listed; capability outcome stated (section 4). |
| Design and readiness: default hook drops `_loader_details` | Answered: listed as a deviation; HF-297 meaning of `()` stated (sections 3.2, 4). |
| Both: classmethod/staticmethod identity | Fixed: real wrapper objects (section 3.1). |
| Design: implicit port rules | Fixed: fresh reads, live iterators, KeyError-only lookup, no borrows, epoch on success only, `None` from `_get_spec` (section 3.3). |
| Design: test gaps | Partly fixed: subclass/monkeypatch test and required existing passes added; the cut cases are listed in section 9. |
| Readiness: new intrinsic unnamed; WASM reservations | Fixed: `molt_importlib_path_finder_types`; reservations answered (sections 3.1, 6). |
| Readiness: retirement tests and ordering | Fixed: section 3.5; tests rewritten, not deleted (section 6). |
| Readiness: Windows coverage would be lost | Fixed: flavor-parameterized root rule with a macOS unit test (section 3.3). |
| Readiness: no phases; incomplete file list | Fixed: sections 6 and 8. |
| Nits (status line, dead name, atomics, hook names, error count, line number) | Fixed. |

Scope cut after round 1 (section 9): the `""`-with-deleted-cwd, refused-tier and
machinery-stand-in-before-a-filesystem-hit differentials, and the 3.13/3.14
targets. No second round: no finding was left open as a blocker.

## 12. Discovery correction after native finder admission

Removing the old machinery body also removes its `zipimport` dependency. That
module's computed `sys.modules` mutation had accidentally hidden an imprecise
source-value transfer: dataclass initialization acquired a module dictionary,
and discovery treated acquisition as invocation of its entire namespace. The
native finder changes exposed that latent defect. Restoring the old forwarding
wrapper did not repair it; restoring the old machinery body merely masked it.

A module's `__dict__` data descriptor returns a live mapping without invoking
PEP 562. The source-demand worklist now represents that mapping separately from
its owner. Merely reading, retaining or discarding it does not invoke exported
functions. Selected lookup, mapping views and copies transport their actual
value provenance. Opaque exposure persists on the namespace owner so later
publications remain reachable. Destructive operations and owned-copy retirement
release possible contained instances through the existing destructor demand.
Unknown selectors remain conservative, and custom attribute protocols keep their
actual descriptor demands.

The intrinsic identity catalog declares the real native attribute-reader facet
and dataclass initializer facet. The returned initializer retains its globals,
builtins, defaults, annotations and closure as metadata, rather than executing
them during construction. Its existing immutable specification selects factories,
ordinary stores and post-init inputs. Invocation transports actual arguments,
defaults and selected global/builtin values into post-init formals, including
variadic formals. Member aliases and `__call__` preserve that invocation context;
late publications replay the existing candidate-demand incidences. A factory
binding is callable; an ordinary captured function default remains a value until
an actual consumer invokes it. Descriptor stores retain their conservative
callback boundary.

One catalog generates the specification field indices for Python discovery and
Rust runtime consumption. The numeric layout and runtime ABI are unchanged.
Immutable tuple publication preserves positional edges while expiring mutable
descendant contents; evaluated unbound `dict.get` uses the existing method-shape
authority rather than falling back to whole-container exposure. Unknown or rebound
callees retain the ordinary conservative analysis.

This follows the closed-world points-to approach already researched in
[findings prior art, family B](../findings_prior_art.md): availability and execution
are separate demands on the same source provenance. It introduces no source-name
exemption, import allowlist or alternative module graph. The source-record cache
schema changes with these facts, including strict decoding of selectors and class
namespace facts.

Two independent diagnosis reviews considered the reductions before implementation.
An implementation review then identified actual/default/variadic post-init
transport, builtin fallback, member reactivation, `__call__`, late namespace
publication, cleanup and factory-selection obligations. The correction addresses
those at their producers and shared worklist consumers. Regression controls use
CPython's mapping/data-descriptor and public dataclass behavior as independent
oracles, alongside cold/warm source-record graph checks. Qualification requires
the parent's 144-program census with no per-program admission additions over main,
the existing dataclass exclusion test, static landing gates and the original
hello/import-tiny instruction budget. The parent owns integration and the V1
ledger; private receipts remain in the assigned lane artifact directory.

### 12.1 Immutable membership and release custody

The publication correction retains immutable membership, not ownership of mutable
descendants. The release-custody authority remains
[lowering rules](../../spec/areas/compiler/0190_LOWERING_RULES.md): cached recursive
publication-release stability is false for an unrooted object-bearing mutable or
unknown leaf and therefore for every containing tuple/frozenset. Publication
widens release risk before rebuilding edges. Exact bytearray payloads cannot
contain Python objects and retain their independent release proof. Member
selection, joins and repeated publication must transport these facts without
inventing a retained lifetime root. Frozenset members never imply positions.

This agrees with [Python's data model](https://docs.python.org/3.12/reference/datamodel.html):
immutable containers retain their immediate referenced objects while mutable
referents can change. Erasing immutable edges is unnecessary; erasing mutable
contents at callback boundaries is mandatory. No release assertion is relaxed.

Two independent correction reviews identified consumers where publication had
been used as callback expiry. A loop could restore a mutable child's old scalar
element fact after an opaque callback inserted a finalizable object; a later
`pop()` then appeared release-safe. An earlier explicit `dict.get` argument could
similarly retain scalar key facts while a later argument installed a colliding
user key, hiding its comparison callback. The existing recursive mutable-content
expiry authority must handle loop and comprehension yields, retained temporaries,
and earlier call arguments. Ordinary binding/selection publication remains
distinct, preserving the namespace-mapping precision from section 12.
Cached iterator yields also consult the existing accumulated body-write effects:
an object write need not advance the callback epoch. The correction does not
depend on incidental unpacking or retirement callbacks to expire these facts.

The owning shape tests use CPython witnesses whose nested mutable-leaf release
runs `__del__`, and whose changed dictionary key runs `__eq__`. They retain
conservative finalizer/weakref and invocation-effect assertions, plus bytearray,
selection, repeated-publication and compact shared-DAG controls. The correction's
qualification is the whole shape suite, affected binding readers, the supplied
144-program no-gain census, the four selected native/LLVM/WASM differentials and
static gates. Earlier cost evidence and the parent-owned landing gate remain
separate facts; this correction makes no new instruction-cost claim.
