# Verified namespace premises

Status: tooling source closures, user-program discovery and frontend premise transport
are implemented (2026-10-30). Target qualification remains a separate obligation.
Owner finding HF-226.

## Problem

By default the binding flow (`src/molt/compiler_analysis/python_binding_flow.py`) assumes that any callback
(a call, an import, an iteration or context-manager callback) may rebind every global and canonical
member of the module under analysis. It applies `EXECUTES_ARBITRARY_PYTHON`, expires every module slot
through the taint epoch and invalidates every member. Only a fixed list of canonical imports
(`_CANONICAL_IMPORT_IDENTITIES`) escapes. After the first callback the analysis forgets two static facts:

1. **`TYPE_CHECKING` is False.** Graph discovery then compiles type-only imports. A program with
   `from typing import TYPE_CHECKING`, `import os`, `if TYPE_CHECKING: import decimal` compiles `decimal`.
2. **The module's `__package__`, `__spec__` and `__name__`.** A later relative import projects to
   `unknown_package`, and closures with `unknown_relative_sources="local_inventory"` cover every local
   module. The M35 frontend lowering scope covered all 1,409 `src/molt` files, so any edit invalidated the
   persisted lowering cache.

This default is stricter than Molt's language contract and inconsistent with Molt's own consumers:

- The [dynamic-semantics policy](../spec/areas/compat/contracts/dynamic_execution_policy_contract.md)
  (spec 0216) admits scoped mutation and excludes unrestricted `exec`/`eval`, arbitrary runtime
  monkeypatching and unrestricted reflection.
- The frontend consumes the graph's source authority. Callable candidates use
  `CallableId(module, definition site)`; an actual call keeps the existing runtime
  code/context guard unless a complete body and execution-context proof permits folding.
- The import flow (`python_imports.py`) trusted foreign code at calls ("Ordinary foreign code has its own
  global mapping") but widened metadata at iteration and context callbacks.

## Prior art (primary sources, 2026-10-15)

| Tool | `TYPE_CHECKING` | Imports and importer globals |
|---|---|---|
| Nuitka | `typing` is a hard module; `TYPE_CHECKING` is a trusted constant bound to the import (`nuitka/HardImportRegistry.py`, `nodes/ImportNodes.py`) | Import nodes do not escape module variables; only unknown calls and `import *` do (`Variables.py`) |
| Meta Lifeguard (PEP 810 lazy-import safety) | evaluated False (`src/traits.rs`) | Per-module effect summaries flag imported-module assignment, `sys.modules` access, `builtins.__import__` overrides and `exec`; unproven modules are unsafe |
| Cinder strict modules | `typing` stub sets it False | Top-level code is simulated; writing another module's values is rejected (`StrictModuleModifyImportedValueException`) |
| mypy, pyright, Ruff, grimp, Gazelle | matched by spelling | no binding model |
| PyInstaller modulegraph, cx_Freeze, pydeps, Pants | includes type-only imports (PyInstaller #8860 open) | no binding model |

Every surveyed tool resolves relative imports from the module identity found at discovery time; mypy
rejects a file reachable under two names.

## The premise

Under spec 0216 a module's globals change outside its own statements only through:

- **its own deferred code**: `global` declarations and own-namespace item writes with literal keys.
  Initialization stores such as `globals().pop("_require_intrinsic", None)` affect the current
  binding state; they do not authorize later callbacks to replace that name;
- **named writes from other modules**: an attribute store, `setattr`/`delattr`, `__setattr__`, or a
  `__dict__`/`vars()` item write on a module reference (an import binding, its attribute chain, or a
  `sys.modules` entry);
- **the import system**, which binds every imported submodule as an attribute of its package.

Anything else (a module object handed to a generic writer, frame `f_globals` writes, `exec`/`eval` on
foreign globals, `gc` walks) is unrestricted reflection or monkeypatching, outside the contract. A module
whose own code reflects its namespace dynamically (an escaping `globals()`, a computed key, `exec`/`eval`
on its own globals) keeps the whole-namespace model for itself.

A star import writes the importing module during initialization; it does not grant later callbacks
ownership of every global. [CPython restricts it to module scope](https://github.com/python/cpython/blob/3bb231a6a5dc02b95658877318bf61501a7209e9/Python/symtable.c#L2375-L2389)
and [publishes exported values sequentially into current locals](https://github.com/python/cpython/blob/3bb231a6a5dc02b95658877318bf61501a7209e9/Python/intrinsics.c#L34-L117).
Binding flow invalidates the current module slots directly, including undeclared global/builtin
fallback reads, independently of the callback domain. Import flow preserves unknown metadata and
partial-publication effects. A star-exported receiver can name or shadow a module or import-hook
object: actual attribute, namespace, item and method mutations retain conservative foreign-write
and hook effects. A plain imported-function call or read does not by itself publish those writes.

Star imports can initialize or shadow module-global lookups. They cannot
initialize function parameters, optimized locals, comprehension targets or
resolved closure cells. The syntax summary derives that lookup custody from
the canonical lexical declaration authority, including `global`, `nonlocal`,
exception and pattern captures. Definition headers and the first comprehension
iterable retain their enclosing scope. Class mappings keep conservative lookup
effects, including their ability to shadow a closure cell. This follows
CPython's [scope analysis](https://github.com/python/cpython/blob/v3.15.0/Python/symtable.c)
and [name code generation](https://github.com/python/cpython/blob/v3.15.0/Python/codegen.c).
The projection visits each lexical scope once and uses enter/restore events
instead of copying enclosing name environments. Only sources containing a star
import require it. This is lookup provenance, not a proof that a receiver is
not a module; explicit import references retain their conservative named-write
classification. Native hook mutations, including augmented assignment, retain
conservative effects through parameters and closure cells: a lexical binding
can still carry an actual hook container. Summary versioning invalidates
records from the former lookup rule.

An unbounded callback premise also cannot prove that a fresh class mapping
stays private. An importer receives the active locals and can install a value
whose release during a later class store changes module metadata. A class-local
import alias still binds the class namespace; its callback and release effects
are a separate fact. Closed-inventory tests must supply the premise established
by their actual sources. Unknown imports and custom preparation mappings retain
the existing conservative effects, including own writers and exposed globals.

Canonical members follow the same rule: a callback changes `typing.TYPE_CHECKING`, `sys.modules`,
`sys.meta_path` and the other `PythonMember` facts only if some code in the graph writes them by name,
or mutates an import-hook object (`sys.modules`, `sys.meta_path`, `sys.path_hooks`,
`sys.path_importer_cache`).

Import-cache entry mutation and replacement of the cache object are distinct facts.
[CPython consults the cache by module name](https://docs.python.org/3/reference/import.html#the-module-cache).
An entry write can redirect a canonical acquisition only when its key may name one of the providers
in the generated identity catalog. Literal keys retain their actual names; formatted keys can prove
disjointness through fixed prefix, suffix and minimum-length bounds. Other computed keys remain
unknown. The write stays in the inventory even when disjoint; this fact does not prove callback
freedom. Operand dispatch transports source origins through the existing protocol/release demands,
including formatting, hashing, comparisons and contained values. Replacing or escaping a hook object
retains the broader invalidation contract.

Native list/dict methods use the builtin-shape authority's direct receiver-write facts, including
slot methods. These follow CPython's [list implementation](https://github.com/python/cpython/blob/3bb231a6a5dc02b95658877318bf61501a7209e9/Objects/listobject.c)
and [dictionary implementation](https://github.com/python/cpython/blob/3bb231a6a5dc02b95658877318bf61501a7209e9/Objects/dictobject.c).
A method that does not directly mutate its receiver can still invoke callbacks. A reached custom
replacement invalidates the native-hook premise independently; a method name alone proves no custom
receiver behavior.

## Design

1. **One summary authority.** `compiler_analysis/python_namespace_writes.py` collects candidate sites
   in one AST traversal and adds lexical lookup projection only for sources with star imports.
   It summarizes the own names its deferred code rebinds
   (`None` on dynamic own reflection) and its named writes to other modules' namespaces, as
   `(receiver spelling, name, construct, line, execution owner, source site)`.
   It is conservative by spelling; definition/source identities remain typed.
   The existing read-only AST admission owns these syntax facts by target language
   version and relative-import package. Binding-core policies, graph premises,
   activated bodies and arguments do not change syntax. Graph and binding queries
   therefore share that source generation's summaries; intrinsic classification
   consumes the full scan's summary directly. Unadmitted mutable trees recompute.
   Canonical private-name resolution precedes publication, so a class's
   `target.__member` and `global __name` retain CPython's mangled bindings while
   literal strings passed to `setattr` retain their exact spelling. Summary schema
   6 also retains each write's source site for deterministic same-line ordering.
   This uses the admitted AST's
   existing lifetime, adds no global or persistent cache, and adds no runtime work
   to compiled programs. Already resolved binding and host-source trees query
   the same internal projection without repeating private-name normalization or
   acquiring an otherwise unused AST digest. A cold normalized query costs one syntax traversal; a repeated
   admitted query costs one context lookup, with no hash, I/O or process launch.
   This follows LLVM's [analysis ownership and invalidation guidance](https://llvm.org/docs/NewPassManager.html#using-analyses):
   repeatedly computing an outer-unit analysis from inner-unit requests can make
   compile time quadratic. Molt's source generation is the owner here; a new or
   mutable AST cannot reuse an old generation's facts.
2. **The binding flow takes a premise.** `PythonBindingFlowPolicy.foreign_namespace_writes` lists the
   names other graph code writes into this module (`None`, the default, means any name);
   `callback_member_writes` lists the canonical members code in the graph writes (default: all). With a
   premise, the taint domain holds only the module's own callback-writable names plus the foreign
   names, so a callback leaves every other global, the module metadata and unwritten members intact.
   `python_canonical_member_writes` maps a summarized write to members through the same
   `_member_write_mask` the flow uses for its own direct stores.
   The compiler-only `python_identity_catalog.toml` generates closed `PythonIdentity` and
   `PythonMember` types, stable raw-mask assignments, canonical acquisition tables and member-write
   guards in `python_identity_catalog_generated.py`. Builtin shape names project from those same
   declarations. `python_value_identity.py` owns queries on those masks, while binding flow owns
   source-ordered transfer; neither maintains a second acquisition or guard registry. The runtime's
   builtin-specialization guard bits remain their separate `runtime/builtin_binding_guards.toml`
   contract. Catalog generation adds no work to emitted binaries.
   Class mappings and exposed frame locals retain their own conservative storage taint. That taint
   does not imply a writer to every module-global name. Dynamic mapping stores preserve actual
   callable source origins at their existing escape boundary even when storage identity becomes
   unknown; source-owned metaclass and mapping alternatives use the same value worklist.
   A supplied graph premise also seeds `entry_invalid_members`: a reached foreign writer may have
   run before this module's first canonical acquisition. Future callback writes and initial member
   uncertainty are separate policy facts. Standalone analysis retains its explicit initial premise;
   an unknown graph owner cannot supply a pristine-entry proof. Policy identity includes both facts.
   `python_namespace_binding_policy` in the existing graph authority applies
   foreign names, callback members and entry members together. User-program
   scans, compiler-host tooling closures and their closed-graph fixtures use
   this projection; target, module, deferred-body and discovery settings remain
   with their caller. Without a graph premise it returns the caller's policy.
   Tooling graph cache schema 21 rejects earlier request records, which could
   omit an import when another module changed `typing.TYPE_CHECKING` before
   the reader's first acquisition. An isolated CPython execution and the real
   eager closure distinguish that failure; a poisoned schema-20 record exercises
   persistent rejection without a remembered correct row masking it. The binding
   core already keys its entry mask, so its schema and the already-correct
   user-program scan records do not change. Applying a graph premise adds one
   immutable policy copy, with constant field count and no I/O, locking or
   process launch. A bounded macOS/CPython 3.12 control measured about 1.17 us
   added per closed-policy construction; it establishes no build-time gain.
   These are compiler/development costs, with no emitted-program work.
   The proof plan's `python-source-dependencies` rule selects the existing
   namespace, tooling-closure, source-request and scoped-key consumer families
   for their source authorities and tests. Targets join the existing local
   pytest cohort; toolchain admission and memory envelopes remain owned by the
   current planner. No Cargo command or second test runner is introduced.
   This follows [rustc's explicit query-input and invalidation model](https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation-in-detail.html):
   every fact that changes a query's result must participate in its authority.
   Further premise slicing must account for implicit module metadata, export
   protocols such as `__all__` and `__getattr__`, dynamic namespace reads and
   published source-value facts. Lexical name reads alone do not prove that
   those outputs are unchanged; current member masks remain intact.
3. **The import flow asks the binding flow.** `PythonBindingFacts.callbacks_rewrite_metadata` says
   whether any callback can rebind `__name__`, `__package__`, `__spec__` or `__path__`. Without it,
   every callback summary in the import flow leaves the metadata states unchanged.
4. **One graph projection.** `compiler_analysis/python_namespace_graph.py` owns
   `PythonNamespacePremise`, `PythonGraphNamespaceWrites` and `graph_python_namespace_writes`.
   It aggregates the existing summaries through the caller's module resolver; source capture, package
   children and graph discovery stay with the caller. `local_python_import_closure` walks the graph
   assuming no named foreign write, summarizes every reached module (cached in its analysis row), and
   walks again until the premise it assumed is the one it found. Writes only add names, so the premise
   grows monotonically. A package's premise always includes its child module names from the
   filesystem, so it never depends on which children the graph reaches.
   User-program discovery uses the same summaries and aggregation through
   `PythonGraphNamespaceIndex`. Each admitted module publishes one writer summary, with foreign
   writes attached to their source execution owner. Module initialization joins immediate writes;
   reached deferred bodies join their own writes once, independently of argument contexts. Full
   inventories retain all owners. Headers and class bodies keep their enclosing execution owner;
   lazy annotations without a schedulable body remain conservative under that owner. An unused
   function's namespace escape cannot widen the entire graph before it is reached. A delta
   wakes only affected module projections and already reached deferred contexts; the existing
   source-value worklist retains their dependencies. Canonical-member or unknown-target deltas
   wake all affected sources. Reprojection reuses captured bytes and cached source facts, and
   identical context/premise pairs do no further scan. Uninspected native/artifact/catalog owners
   contribute unknown writes rather than an empty proof. The resulting scan authority carries
   its actual premise to runtime-import policy, frontend contexts, cache identities
   and workers. The five-field premise carries foreign names, future canonical-member
   writes, entry member uncertainty, noncanonical providers and opaque source activations.
   Unknown opaque custody is distinct from a proved empty set. Source bodies whose
   real function/class values escape, are recreated or have visible attribute mutations
   retain conservative activation. Exact callees transfer arguments through their
   existing formal-parameter worklist; merely passing an argument does not expose
   its whole namespace. Literal `getattr`/`hasattr`/`delattr` select real member
   sources; unknown selectors retain protocol and escape effects. Class allocation
   and method folding additionally require unexposed definition/method identities.
   A declined constructor proof calls the actual class protocol.

   Inventory scans collect deferred writes without asserting they ran before another
   module's first acquisition. Activating an execution owner contributes its entry
   writes and wakes dependent projections. The source-demand fixpoint also wakes
   changed opaque-source premises. Import scan schema 40 rejects older owner/value
   records. Target platform and version scalars, cache entry provenance and provider
   exclusions participate in the same scan authority; none is inferred from a host
   spelling or supplied by a second frontend registry.

   Package search paths belong to the same source-scan authority. A regular package
   starts at its original `__init__.py` directory; a generated namespace package
   retains all real namespace portions in search order, and a native support slice
   retains its original package directory. A generated filename cannot supply this
   identity. Ordinary authority joins reject a changed root sequence. The child inventory is cached by that canonical root
   tuple, independently of generated byte generations. Immutable authority updates
   reuse the canonical tuple without resolving the roots again. Generated namespace
   and importer sources join the same operation-wide discovery before publication,
   capturing their actual byte SHA and converged writer premise.

   This follows [PEP 420](https://peps.python.org/pep-0420/)'s namespace portions and
   [CPython's path finder](https://github.com/python/cpython/blob/v3.15.0/Lib/importlib/_bootstrap_external.py).
   CPython's namespace `__path__` can change with its parent path; Molt captures the
   admitted roots for one build operation. A root change requires fresh admission,
   never reuse of an older precise premise. This host-side topology work adds no
   analysis to emitted binaries.
   The same operation now retains raw-source leases and semantic filesystem
   observations through frontend cache/worker transport and artifact publication.
   Entry selectors, root discovery and graph lookups share the topology owner;
   byte changes, new shadows or changed package portions require fresh admission.
   The [source custody contract](../internals/source_scan_authority.md) defines
   the cache generations, worker receipts and prepublication fences. This is a
   prerequisite for precise frontend premises, not evidence of their adoption.
5. **Dead relative imports are lexical edges.** When the import flow proves a relative import never
   runs (a statically false `TYPE_CHECKING` branch), the full lexical graph resolves it from the
   module's identity, as it keeps a dead absolute import. Before this, such a statement failed the
   executable-tool closure with an unknown anchor (also on main).

## Results

Lowering scope on the real tree (isolated `MOLT_CACHE`, Apple M-series):

| | main aedec0934 | this design |
|---|---|---|
| files in scope | 1,409 (whole-tree coverage) | 242 (import graph only) |
| fixpoint rounds | n/a | 1 |
| cold | 4.09 s | 4.02-4.14 s |
| warm | 0.14 s | 0.04-0.05 s |

An edit outside the 242 files no longer invalidates the persisted lowering cache.

## Operation-wide discovery

`_ModuleGraphDiscovery` owns one namespace index, source-value worklist and source
job queue from entry preparation through native/runtime/importer materialization.
Every production extension carries this explicit owner. A frozen graph snapshot
does not contain the execution provenance needed to resume discovery.

Source state moves from admission to projection, then back to projection only
when its mode, native selection, namespace premise or source context changes.
The drained queue is the convergence boundary; final publication verifies captured
bytes. Each source keeps its original resolution roots and admission context.
A full-depth runtime custody promotion adds that permission to the original
context rather than interpreting application imports under support-library roots.

Full source permission grants availability, not execution. Inventory projections
retain the runtime-import protocol and dynamic-anchor requirements of deferred
code. Execution projections admit imports, cache writes and source execution
requests only when their source owner is demanded. Runtime catalog custody covers
every body;
only that full-owner scan retires prior partial body jobs. Scoped foreign writes
likewise join the namespace premise when their owner activates. Native module
cache storage retains values without exposing their namespaces, including the
implicit current-module provenance of `sys.modules[__name__]`; key callbacks
remain a separate incidence even when a value is also held.
Stdlib intrinsic classification explicitly requests full import-site inventory:
another module's deferred import can read a private intrinsic binding even when
that binding's defining module never loads it itself. This availability proof
does not admit that deferred import as executed application code.
Lowering analysis also requests inventory to scope dependencies of available
function bodies. Its raw-source inventory cannot replace an execution scan:
purpose travels through the resolver and persisted request keys and payloads,
and precomputed projections carry a typed purpose checked before graph reuse.

Literal member selection passes typed parameter-source argument facts into the
canonical binding fixpoint. Its control flow proves branch selection and abrupt
completion, including a return before a lazy hook's tail. Parameter stores and
callbacks follow the same fact lifetime as all other bindings. There is no
consumer-side reconstruction of that activation's control flow.

A named writer delta wakes only its prior target sources and reached body contexts.
Revisions of completed scans wait until pending admissions and first body demands
drain, then run against the latest premise in one wave. New opacity facts still
revise their full five-field authority immediately. This avoids repeatedly
walking every completed body while a wave discovers more exposed owners; the
drained revision queue remains part of the convergence boundary.
Active rows are also indexed by logical receiver name: a later namespace package
can acquire a generated source path without losing writes observed before that
path existed. A closure result separates its own module/reason membership from
revised prior authority rows. Publication validates and merges only those deltas,
without copying the enclosing graph prefix for every extension. Repeated requests
with identical projection inputs retain the same worklist and perform no scan.

Native helpers use a semantic view of their original captured source. The existing
pruner selects roots before import/value/default/kind analysis and frontend lowering;
there is no intermediate unparse/file/reparse or generation-transfer lane. Original
source spans keep every deferred owner identity stable as roots grow. Explicit raw
Python initialization dominates a helper selection, and scan depth never decreases.
Selected native views provide dependency inventory before graph ingestion.
The native artifact's actual callable exports separately seed canonical source
demands; retaining a transitive lowering binding does not execute it. Export
demands survive promotion to raw source. Resolver keys include inventory purpose
even when selecting every definition leaves the native and raw AST identical.
The resolver retains one current selected AST per source/target. Syntax/request and
runtime-policy caches distinguish raw SHA, target, depth and selection. Root planning
and views reuse the same original parse; telemetry counts actual parse/prune work.

This follows the explicit dependency identities and demand-driven reuse described in
[rustc's incremental query design](https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation-in-detail.html)
and [MLIR's analysis preservation contract](https://mlir.llvm.org/docs/PassManagement/#analysis-management).
Molt uses the existing monotone value solver because admitted source views only grow
and keep original identities; a retracting incremental engine adds no benefit to
this operation. Changed source bytes fail custody and start a fresh operation.
All of this is compiler-side analysis; emitted programs perform none of it.

Runtime-import support is a projection of that same reached execution walk.
The import visitor records ordinary import statements and possible canonical
import or `find_spec` calls after its reachability check, before static-target,
helper or runtime-catalog resolution. Intrinsic and future-import exclusions
retain their existing semantics. Synthetic support imports do not imply a
reached Python import operation. Initialization and deferred owner/argument scans
use their actual execution regions and namespace premise for requests. Full
inventory also records protocol requirements of available deferred regions,
without treating them as executed import requests.

This removes the former protocol prepass: one additional O(R) execution-region
walk, its evaluate-node tuple and a binding-core lookup for every computed scan
of R regions. Existing persisted hits already contain the bit and do not pay
this computation. There is no new memo, changed cache protocol or narrowed
premise. Sharing the existing operation-local analysis follows
[LLVM's analysis reuse and pass locality](https://llvm.org/docs/NewPassManager.html#using-analyses).
The bounded matched production-projection measurements and remaining graph
work are recorded under HF-285 in the findings ledger; they do not establish
whole-build or emitted-program performance.

## Remaining consumers

- **Package topology**: package-child topology changes must be fenced before frontend
  premise adoption; root identity is not a directory-generation proof.
- **Frontend codegen**: pass the build graph's premise to the per-module lowering analysis, so codegen
  facts and `_module_stable_funcs` share one premise.

Source scan authority retains the raw SHA from its existing byte snapshot;
precomputed scans reference that same field. Ordinary joins reject conflicting
generations. A source-byte change requires a fresh build operation; native
helper selection never creates a replacement source generation. Runtime-import policy fences every identified
source, including foreign writers, before and after its cached or fresh query.
Neither a matching path nor unchanged size/mtime proves unchanged bytes.
Unknown/uninspected records grant no captured-generation proof.

This follows the dependency identity principle in
[rustc incremental compilation](https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation-in-detail.html)
and [LLVM content addressing](https://llvm.org/docs/ContentAddressableStorage.html).
It reuses the current snapshot and file-hash owners rather than adding another
source store or AST pass. The frontend now carries the same raw-source lease
through cache/worker transport and prepublication fences; a runtime-policy fence alone does
not qualify that boundary. Guarded function dispatch candidates remain distinct
from immutable binding facts.

Standalone source scans without a closed graph remain conservative; they cannot invent an empty
foreign-writer premise. Their type-checking and reentrant-import controls continue to test that
boundary. User discovery now supplies the actual graph premise instead of changing that default.

## Frontend premise adoption: exact builtin certificates (HF-226, HF-79)

Status: implemented on lane `lane/family-b-premises` (base origin/main
f1d450c4f1529fd00d11780c748a2af8ca403dfa), 2026-11-05; acceptance results
are under "Results of this lane". This section closes the builtin part of HF-226's
frontend premise adoption and of HF-79's proven bindings. Source callable,
class and import certificates remain open (see "Scope cut").

### Problem (baseline, main f1d450c4f1)

The frontend already analyzes each module under the graph's actual premise
(`statement_scope.py` passes `namespace_premise` to `analyze_python_bindings`).
Under that closed premise the flow proves `str` and `range` exact in a function
body: for the canonical `isinstance` primitive the read facts are
`identities == BUILTIN_STR` and `BUILTIN_RANGE`, not invalidated. The frontend
ignored that proof. It treated every builtin read in a function as a live
lookup (`expressions.py` sent `BUILTIN_TYPE_TAGS` names to `MODULE_GET_GLOBAL`)
and opened a runtime guard for every read with a guard bit
(`_project_builtin_guard_bits` gave a bit to every read the module never binds).

The baseline release IR of the canonical `isinstance` primitive
(`molt build --build-profile release --emit-ir`, macOS arm64, 29 admitted
modules, binary 35,707,440 bytes with symbols kept) shows, per loop iteration:

- `molt_guard_builtin_bindings(isinstance)`, a runtime call, opens a fast and a
  slow arm around `isinstance(objs[i & 63], str)`;
- `module_get_global('str')` runs in both arms: the subscript ends the guard's
  coverage walk, so the fast arm does not cover `str`. It misses the module
  dictionary and then hits the builtins dictionary.

The `for i in range(n)` guard also duplicates every loop body into a fast arm
(counted loop) and a slow arm (iterator loop), in all four measured primitives.
`isinstance` and `issubclass` had no catalog identity, so the flow read them as
`OTHER | UNBOUND` and could never prove them.

The baseline has two related defects, found by mechanism:

- The guard's fast arm trusts the binding as a proof of call shape:
  `def f(): return len()` crashes the frontend (`IndexError` in
  `_emit_proven_shape_builtin_call`).
- Named builtin lowerings replace the live read without a proof. A guard's slow
  arm still emits the `ISINSTANCE` operation, so a `builtins.isinstance` or
  `__builtins__` writer cannot replace it, as CPython allows.

### Prior art

- CPython 3.12 specializes `LOAD_GLOBAL` into `LOAD_GLOBAL_BUILTIN` behind dict
  keys-version guards on the globals and builtins dictionaries
  ([`Python/specialize.c`](https://github.com/python/cpython/blob/v3.12.0/Python/specialize.c),
  `specialize_load_global`; PEP 659). Molt's existing HF-F177 guard is that design.
- Ahead-of-time compilers that close the program drop the guard. mypyc treats
  builtins as fixed and documents monkeypatching them as unsupported
  ([mypyc differences from Python](https://mypyc.readthedocs.io/en/latest/differences_from_python.html)).
  Nuitka lowers an unshadowed builtin name to a builtin reference node
  (`nuitka/nodes/BuiltinRefNodes.py`). GraalVM Native Image folds values its
  closed-world points-to analysis proves constant
  ([Native Image basics](https://www.graalvm.org/latest/reference-manual/native-image/basics/)).
- Molt's spec 0216 closes the program, and this document's premise bounds every
  writer. The flow's exact identity is therefore a proof, not a speculation,
  and needs no guard. Unproved reads keep CPython's guard model.

### Reconciliation with the retired HF-79 bundle (e407112cc01b0bf5ea1a9f5d5f73a9dc60f57446)

| Bundle element | Decision | Reason |
|---|---|---|
| `PythonExpressionFact.exact_builtin_name`, `BUILTIN_NAMES_BY_IDENTITY` projection | Reuse, rewritten | Same typed query; the projection moves into the generator with the other identity tables, and the query requires a global lookup. |
| `_builtin_binding_name`/`_builtin_binding_proven` replacing `_builtin_guard_covers` | Reuse, rewritten | One authority for "this read selects the builtin": the flow's proof or the guard's fast arm. |
| Exact reads get no guard bit | Reuse | A proved read needs no guard. |
| Proved value read lowers to `BUILTIN_TYPE`/`BUILTIN_FUNC` | Reuse, narrowed | Types only. `BUILTIN_FUNC` resolves the published name again (`molt_func_new_builtin_named`, `lookup_builtin_name`), so a function value gains nothing and keeps the live read. |
| `__builtins__` writer excludes proofs | Reuse, moved | The bundle checked it only in its callable projection. Here it sits in the builtin fallback of `_resolve_name` (`_builtins_mapping_rebindable`), the one place that produces builtin identity. |
| `builtin_specializations` call-shape mask | Reuse | A value proof does not prove invocation shape; see the `len()` defect above. The flow's `builtin_call_shape` is the shape authority. |
| `isinstance`/`issubclass` identities | Reuse, narrowed | Only these two are needed to remove the guard from the `isinstance` loop. The bundle added 29 builtins at once. A catalog identity turns an `OTHER` read into an exact one, which changes call effects, operand escape and source demand wherever the builtin appears, so every new identity needs its own census. |
| Exact `isinstance` call: protocol-only operands | Drop | Ordinary unknown-callee operand escape stays, so discovery does not change. Precision belongs to HF-181. |
| Source callable/class/import certificates, direct-call ABI, class slots, receiver custody (03ffde220d54dcc906d763cbfb06f0157a7ce54e, d8a34f545a4356d306e37b891bce4cfe88c8bcb3) | Drop from this lane | See "Scope cut". |
| Uncertain-read `escape` removal, `vars()` projection, dict literal-key selection, `source_value_receivers` | Drop | Discovery precision changes (HF-181), with their own failing census and dataclass discovery. This mechanism does not need them. |

### Mechanism (this lane)

1. **Catalog.** `python_identity_catalog.toml` gains the `BUILTIN_ISINSTANCE`
   and `BUILTIN_ISSUBCLASS` identities and their `BUILTINS_*` member guards. The
   generator projects `BUILTIN_NAMES_BY_IDENTITY` for every catalog builtin.
2. **Builtins mapping custody.** CPython takes a frame's builtins from
   `globals["__builtins__"]` when the function is created
   (`_PyEval_BuiltinsFromGlobals`); Molt's runtime does the same
   (`frames.rs`: `frame_effective_builtins_bits`,
   `frame_stack_active_builtins_bits`). `_resolve_name` adds `OTHER` to the
   builtin fallback when `_builtins_mapping_rebindable()`: this module binds
   `__builtins__` anywhere, or the premise lists it as a foreign write. The
   check is module-wide, so a read before the write is not proved either. Its
   soundness rests on the closed premise's foreign-writer list being complete,
   as every other premise fact does. An open premise is unchanged: its
   callbacks already invalidate reads.
3. **Guard bits.** `_project_builtin_guard_bits` gives no bit to a read whose
   fact is exact for the same name (its docstring states the rule).
4. **One frontend authority.** `_proven_builtin_name(node)` returns the builtin
   when the fact is exact (`exact_builtin_name`: not invalidated, global
   lookup, identity is exactly one catalog builtin) or when the enclosing
   guard's fast arm covers the read. `_builtin_read_replaces_guard(node)` is
   the subset with the fast arm's specialization policy: a covered read, or an
   exact read in a Python function activation, where the guard model applies.
   Only that subset lowers a shape call without retaining the live callable
   across argument evaluation; module code keeps the call fact's own elision
   proof (`callee_elision_safe`), as on main. The runtime guard admits only when the
   frame's builtins dictionary is the sealed builtins namespace
   (`binding_guards.rs`, `active_frame_admits`), so a fast arm also excludes a
   replaced `__builtins__` mapping. Every former `_builtin_guard_covers`
   consumer (shape calls, `_specializable_builtin_name`, `range` loops, the
   invalidation query, `super`) uses it.
5. **Call shape.** A proved shape call also requires the call fact's
   `builtin_specializations` bit, which the flow computes from
   `builtin_call_shape(...).specialization_valid` for each possible shape
   builtin. An invalid shape calls the builtin, which raises its own error.
6. **Named lowerings need the proof.** `_named_builtin_unproved` sends a
   catalogued, non-reflective builtin whose read is not proved to the live
   read and a dynamic call, before any named lowering. Names without a catalog
   identity keep today's lowering (see "Open siblings").
7. **Value lowering.** `_load_name_expression` calls
   `_emit_proven_builtin_value`, which emits `BUILTIN_TYPE(tag)` for a proved
   type: the runtime's canonical type object, which is the object `builtins`
   publishes. A proved function value keeps the live read.

Module-scope reads use the same facts: before any callback, and under a closed
premise after callbacks, an exact module-scope read lowers the same way. In a
loop that mixes proved and unproved reads, the proved reads lower to their
values in both arms of the guard that the unproved reads still open.

There is no new op, runtime entry, cache or registry. Native, LLVM, WASM, Luau
and Rust backends already lower `BUILTIN_TYPE`, `BUILTIN_FUNC` (with the
manifest ABIs `molt_isinstance`, `molt_issubclass`), `ISINSTANCE` and the
counted loop; the frontend already emitted these operations before this change.

### Invariants

- An exact read implies, over the closed graph: no writer of the module slot
  (own, `global`, literal reflection or named foreign write), no write of the
  `builtins` member, no writer of `__builtins__`, and a stable activation
  namespace. A source body whose code reaches another `FunctionType`
  activation is in `opaque_activation_sources`, and its reads are `OTHER`.
  Any one writer keeps the guard.
- An open or unknown premise proves no read in a function after a callback;
  standalone compilation therefore keeps today's guards.
- A slow arm honors exact facts but never opens or trusts a guard.
- A named lowering of a catalogued builtin implies a proof at its callee read.
  This lane covered catalogued builtins only; the HF-776 section below
  extends the invariant to every builtin read.

### Costs

Compile time: one fact lookup per name read and one `builtin_call_shape` query
per call whose callee may be a shape builtin, both inside existing passes.
Emitted code: per proved type read, one `BUILTIN_TYPE` call replaces up to two
dictionary lookups, and a proved callee needs no guard call; a proved `range` loop is emitted once instead
of twice. No runtime state, allocation or synchronization is added. An
unproved catalogued call in a guard's slow arm becomes a dynamic call; it runs
only after its guard fails.

### Scope cut, with evidence

- **Source callable/class/import certificates.** Removing the lookup of a module
  function or class needs a value source other than the namespace: either a
  value-free direct-call entry (the bundle's 03ffde220d: native/LLVM/WASM call
  ABI, recursion admission, invocation publication) or a static module slot (a
  new IR op in every backend). The findings ledger's HF-79 row records that
  version at `dynamic_call` 0.9977x of base and primitive binaries 22.8 to
  30.9 MB, with a failed census. Today's guarded
  call (`call_guarded`) needs the callee object for invocation publication
  (`emit_invocation_publish`), so removing admission alone cannot drop the
  lookup. A typed pure read of a proved binding, which LICM and GVN may hoist,
  is the smallest next design. It is a separate decision.
- **Acceptance axes without binding work.** The per-iteration IR of `loop`
  (`inplace_add`, counted index) and `dynamic_call` (`call_func` of a local)
  contains no namespace operation on main. Premise adoption cannot change their
  per-op slope; their costs are the counted-loop and call protocols. The
  `dataclass_construction` loop has one `module_get_global('Point')` per
  iteration, which needs the class certificate above.

### Open siblings

Named lowerings of builtins without a catalog identity (`type`, `id`, `ord`,
`chr`, `print`, `object` and others in `call_dispatch_builtin_*.py`) still
replace the live read without a proof. A program that assigns `builtins.type`
or replaces `__builtins__` therefore still calls the canonical `type` where
CPython calls the replacement. Each needs a catalog identity, as this lane
gives `isinstance` and `issubclass`; that changes discovery for every program
and needs its own census. The lane reports this family to the integrator for
its own ledger row (HF-776; see "Every named builtin lowering requires a
proof" below).

Main also fails to persist the frontend lowering record of a module that
defines a dataclass: `local_classes` carries `dataclass_init_projection`, a
tuple of `DataclassInitField` dataclasses, and `_write_persisted_module_lowering`
raises `TypeError: Object of type DataclassInitField is not JSON serializable`
(`cache_keys._json_ir_default`). Any cold build of the
`dataclass_construction` primitive with the lowering cache enabled fails. This
is outside this lane's mechanism; the lane reports it to the integrator.

### Acceptance and evidence rules

The five-pair primitive A/B (`tools/perf_scoreboard.py primitives`, exact
CPython 3.12.16) passes when no axis ratio rises beyond its ratchet noise bound
and the `isinstance` per-op ratio falls beyond its bound. The prototype
evidence before review was deterministic: the `isinstance` loop has no guard
and no lookup (182 to 82 IR ops in `run`), 29 admitted modules, and a binary of
35,161,472 bytes against 35,707,440. One unpaired instruction sample
(5,952 to 5,165 per operation) is anecdotal; the paired A/B is the claim.
The census passes when no program of the 144 admits a module that main does
not.

### Test plan

- Modify `tests/test_frontend_builtin_binding_guards.py`: under a closed
  premise an exact read has no guard bit, no `module_get_global` and one loop;
  a `global`, `__builtins__`, foreign or member writer keeps the live read;
  standalone compilation keeps both guards; `len()` in a function (proved and
  guarded) and at module scope calls the builtin; `globals` keeps its lowering;
  a guard's slow arm calls the live `isinstance`.
- `tests/test_gen_python_identity_catalog.py` and the generator check cover the
  catalog projection.
- Create, under `tests/differential/basic/`, run against CPython 3.12.16 on
  native, LLVM and WASM: `proven_builtin_values.py` (identity
  `is builtins.str`, metaclass `__instancecheck__`/`__subclasscheck__`, tuples,
  aliases, call-shape errors, zero-trip loops, comprehensions, module scope),
  `proven_builtin_builtins_mapping.py` (`__builtins__` replaced between two
  `def`s) and `proven_builtin_function_rebind.py` (`types.FunctionType` with
  custom builtins). The existing `builtin_binding_guards*.py` programs keep
  covering member, `global` and `globals()` writers.
- Census of the 144 programs, public stdlib dataclass discovery, the primitive
  A/B, and module counts and binary sizes for hello and the loop program.

### Review record

Raw reviews are in the lane's evidence directory
(`logs/agents/lane-family-b-premises/artifacts/reviews/`).

Round 1 (egm-design and egm-codebase-details, Sonnet, design file only): ten
and seven findings, no blockers. Fixed: status and tense, file markers and
paths, the `len()` regression test, `FunctionType` and module-scope coverage,
evidence and acceptance rules, code names, the slow-arm invariant, and scope
naming. The reviewers' `FunctionType` finding led to a differential; the
`__builtins__` mapping differential then exposed the named-lowering defect,
which item 6 repairs for catalogued builtins.

Round 2 (egm-implementation-readiness and egm-zoom-out, Sonnet, design file
only), covering items 2 and 6: eight findings each, no blockers. Fixed: full
hashes and a tracked citation for the bundle figures, guard coverage of a
replaced `__builtins__` mapping (the runtime guard requires the sealed
builtins namespace), the premise dependency of `__builtins__` custody, mixed
loops, the catalogued-only invariant, the user-visible consequence of the open
siblings, and results recorded below. Not adopted: removing helper names and
prior art (round 1 asked for exact code names; the prior art carries the
design decision).

Implementation changes after review, both narrowing: function values keep the
live read (`BUILTIN_FUNC` is itself a name lookup), and the fast arm's forced
callee elision applies only to activation reads. The existing module-scope
tests (`tests/test_frontend_builtin_shape_authority.py`) showed that module code
relies on the call fact's elision proof: a proved `str(source)` there would
otherwise lower to `STR_FROM_OBJ` typed exact `str`, although `__str__` may
return a subclass. The guard's fast arm has that typing on main; it is an open
sibling of this mechanism.

### Results of this lane

Measured 2026-11-05 on macOS arm64 against origin/main f1d450c4f1, release
builds with symbols kept, CPython 3.12.16 as oracle and denominator.

- **Primitive A/B** (`tools/perf_scoreboard.py primitives`, five pairs per
  leg, base and head legs run separately; frontend lowering cache disabled in
  both legs because main fails to persist a dataclass record, see "Open
  siblings"). `isinstance` per operation: 5,976.4 to 5,187.8 Molt
  instructions (0.868x; ratio to CPython 7.516 to 6.524, noise bound 1.0%).
  Every other axis stays within its ratchet noise bound. `reduce_ex` per
  operation is bimodal in both legs (samples near 12.6 and 14.1 times
  CPython); its median moved with the sample mix (bound 26.1%). The `loop`,
  `dynamic_call` and `dataclass_construction` per-op slopes do not move
  (1.0084x, 0.9989x, 1.0001x), as "Scope cut" predicts.
- **Emitted code.** In the canonical `isinstance` primitive, `run` has one
  loop instead of a guarded pair and no `molt_module_get_global` or
  `molt_guard_builtin_bindings` call in it (machine code 3,932 to 1,613
  instructions). Binaries shrink: hello 35,640,008 to 35,110,584 bytes, loop
  35,724,712 to 35,161,432, `isinstance` 35,707,440 to 35,177,984,
  `dataclass_construction` 38,782,584 to 38,170,328.
- **Modules.** hello 29 to 29, loop 29 to 29, `isinstance` 29 to 29,
  `dataclass_construction` 32 to 32.
- **Differentials.** The three new programs and the five
  `builtin_binding_guards*.py` programs pass on native, LLVM and WASM against
  CPython 3.12.16 (24 cells). On main, `proven_builtin_values.py` fails to
  build (`len()`) and `proven_builtin_builtins_mapping.py` prints
  `after (True, 3)` where CPython prints `after ('custom', -1)`.
- **Discovery.** `test_public_stdlib_deferred_demand_graph` passes all eight
  programs, including the dataclass program.
- **Census.** Module-graph discovery of the 144 census programs (CLBG
  kernels, bench programs and a differential sample) admits the same modules
  on main and on this lane: 2,226 admissions each, no program gains or loses
  a module, no refusals.

## Every named builtin lowering requires a proof (HF-776)

Status: design on lane `lane/hf776-builtin-proofs` (base origin/main
6cbf288f590b10cac4e13b2d18027f9829d391c1), 2026-11-05. This section closes the
HF-226 open sibling "Named lowerings of builtins without a catalog identity".
The HF-226 sibling about fast-arm typing of `str(source)` at module scope stays
open, and so does `__build_class__` (see "Open siblings" below).

### Problem (base 6cbf288f59)

The frontend picks an operation from a builtin's spelling in three places, and
only one of them asks for a proof:

1. `_try_emit_named_builtin_call` (`call_dispatch_named_builtins.py`) checks
   `_named_builtin_unproved`, but that check covers catalogued names only.
   The scalar, iterator and fallback dispatchers then lower the uncatalogued
   names in the inventory below with no proof.
2. `_try_emit_named_call` (`call_dispatch_named.py`) lowers ten exception
   constructors (`EXCEPTION_NEW_*`), `abs` (`ABS`), `globals`, `locals`,
   `vars`, `dir`, `getattr`, `setattr`, `delattr`, `hasattr`, zero-argument
   `super()` (`molt_super_from_frame`), `classmethod`, `staticmethod`,
   `property` and `print` before it reaches item 1, so even the catalogued
   names among them have no proof check.
3. `_load_name_expression` (`expressions.py`) lowers a read of `NotImplemented`
   or `Ellipsis` to a constant, a read of any builtin exception to
   `EXCEPTION_CLASS` (the canonical class, by spelling), and `locals` and
   `__import__` to a `builtins` attribute read.

A read that the binding flow marks invalidated already takes the live path
(`visit_Call`). The defect is every other unproved read: a guard's slow arm,
and reads after a `builtins` member writer, a `__builtins__` writer or a
foreign writer that does not invalidate them. Measured on base (macOS arm64):
with `builtins.type` and `builtins.ord` replaced, the HF-226 repro prints
`replaced <class 'int'> 97`; CPython 3.12.16 prints `replaced replaced-type -1`.
This lane commits the repro as
`tests/differential/basic/named_builtin_replaced_repro.py`.

A probe on base measured the runtime side: dynamic calls of `globals`,
`locals`, `vars` and zero-argument `super` through a local alias give
CPython's results inside a method. The runtime callables read the caller's
frame. The probe covers one scope only, so the test plan adds a differential
that calls every frame-reading name through its live read in each scope kind.

### Inventory (search by mechanism)

The mechanism is "an operation or runtime callable chosen from `node.id`".
The search covered every comparison of `node.id`, `func_id` or `name` with a
builtin spelling or a builtin table (`BUILTIN_FUNC_SPECS`, `BUILTIN_TYPE_TAGS`,
`BUILTIN_EXCEPTION_NAMES`, `BUILTIN_SHAPE_NAMES`, `_name_resolves_to_builtin`)
under `src/molt/frontend`, and every caller of `_emit_builtin_function`,
`_emit_builtin_type_value`, `_emit_exception_class` and
`_emit_exception_new_from_args`.

| Family | Names without a catalog identity on base | Named lowering |
|---|---|---|
| Scalar calls | `type`, `object`, `id`, `ord`, `chr`, `repr`, `callable` | `TYPE_OF`, `OBJECT_NEW`, `ID`, `ORD`/`ORD_AT`, `CHR`, repr, `IS_CALLABLE` |
| Iterator calls | `slice`, `aiter`, `anext`, `any`, `all`, `sum`, `map`, `zip`, `min`, `max`, `iter` | `SLICE_NEW`, `ANEXT`, inline reductions, runtime callables |
| Named calls | `abs`, `dir`, `super`, `print` | `ABS`, `BUILTIN_FUNC`, `molt_super_from_frame` |
| Fallback calls | `ascii`, `bin`, `compile`, `divmod`, `enumerate`, `filter`, `format`, `hash`, `hex`, `next`, `oct`, `reversed`, `sorted` | `BUILTIN_FUNC` and `CALL_FUNC`/`CALL_BIND` |
| Constants | `NotImplemented`, `Ellipsis` | `CONST_NOT_IMPLEMENTED`, `CONST_ELLIPSIS` |
| Exceptions | the 69 builtins exception spellings present on every supported target | `EXCEPTION_CLASS`, `EXCEPTION_NEW_*` |

`BUILTIN_FUNC_SPECS` has 45 names: 12 are catalogued, `pow` and `round` have no
named lowering (they always keep the live callable), and the other 31 are in the
table. The 4 other callables (`type`, `object`, `slice`, `super`) come from
`BUILTIN_TYPE_TAGS`. `input`, `breakpoint`, `eval` and `exec` have no named
lowering, so they get no identity.

These catalogued names keep their identity and only gain the gate: `globals`,
`locals`, `vars`, `getattr`, `setattr`, `delattr`, `hasattr`, `classmethod`,
`staticmethod`, `property` (item 2), and `locals` and `__import__` (item 3).

Supported targets are CPython 3.12 and newer, so every exception added before
3.12 (`ExceptionGroup`, `EncodingWarning`) exists on every target. The
frontend's availability authority (`_builtin_exception_is_available`) gates two
spellings: `PythonFinalizationError` (3.13 and newer) and `WindowsError`
(win32). They get no identity, so their reads stay live on every target, as
they are today on targets without them; on 3.13 a `PythonFinalizationError`
read becomes a live read. A catalog row for them would claim a binding that is
absent on some targets.

Sites that test a builtin spelling for another reason keep their behavior:

- `_maybe_record_local_intrinsic_wrapper` (`callable`, `globals`) recognizes a
  wrapper shape and lowers nothing from the builtin.
- `attribute_access.py` and `analysis_collect_static.py` read
  `globals`/`locals`/`vars`/`setattr`/`delattr` calls only to stay
  conservative.
- `assert` raises through CPython's `LOAD_ASSERTION_ERROR`, which loads the
  canonical class with no namespace read, so it is not a builtin read.
- `raise X` and `except X` read `X` as an ordinary name: item 3 covers them.
- Loop, comprehension and fused consumers already ask
  `_specializable_builtin_name`, the HF-226 authority.

### Prior art

- CPython 3.12 specializes builtin calls by object identity, not by name:
  `specialize_c_call` (`Python/specialize.c`) compares the callable with
  `interp->callable_cache.len` and `.isinstance` before it selects
  `CALL_NO_KW_LEN` or `CALL_NO_KW_ISINSTANCE`, and `LOAD_SUPER_ATTR`
  (`Python/bytecodes.c`) checks that the global `super` is the builtin type. A
  replaced builtin is a different object, so it gets the generic call.
- Molt's HF-226 section above: a closed-world exact identity or a guard's fast
  arm proves the read, and the named lowering keys on that proof. This
  section extends the same authority to every name; it adds no second one.

### Mechanism

1. **Catalog.** `python_identity_catalog.toml` gains one `[[builtin]]` row for
   each name in the inventory: 35 callables and types, 2 constants and 69
   exceptions. Each row appends one identity bit `BUILTIN_<NAME>` and one
   member guard bit `BUILTINS_<NAME>` (`<NAME>` is the spelling in upper case,
   for example `BUILTIN_VALUEERROR`), so no existing bit moves. `shape =
   false` for all; the shape family is unchanged. The generator already
   projects `BUILTIN_IDENTITIES` and `BUILTIN_NAMES_BY_IDENTITY` from the rows,
   and `_member_write_mask` already maps `builtins.<name>` writes to the row's
   guard and a computed member write to every guard. The generator gains one
   check: every name in the binding-guard table
   (`runtime/builtin_binding_guards.toml`) has a row, because the guard's fast
   arm proves a name and the dispatch below needs its identity.
2. **One typed proof query.** `_proven_builtin_identity(node)` returns the
   `PythonIdentity` that a closed-world exact read or an enclosing guard's
   fast arm proves, or None. `_proven_builtin_name` becomes its projection
   through `BUILTIN_NAMES_BY_IDENTITY`; both keep HF-226's two authorities.
3. **Call dispatch keyed on the proof.** `_try_emit_named_call` computes the
   proved identity of the callee once and passes it on. The dispatchers take
   `builtin: PythonIdentity` instead of `func_id: str`, and each branch tests
   that typed identity:

   | File | Function | Base test | New test |
   |---|---|---|---|
   | `call_dispatch_named.py` | `_try_emit_named_call` | `func_id == "abs"`, `in {"BaseException", ...}` and 14 more | `builtin is PythonIdentity.BUILTIN_ABS`, exception identities |
   | `call_dispatch_named_builtins.py` | `_try_emit_named_builtin_call` | `_named_builtin_unproved(node, func_id)` | dispatch only when `builtin` is not None |
   | `call_dispatch_builtin_scalar.py` | `_try_emit_named_builtin_scalar_call` | `func_id == "type"` and 8 more | `builtin is PythonIdentity.BUILTIN_TYPE` and 8 more |
   | `call_dispatch_builtin_iter.py` | `_try_emit_named_builtin_iter_call` | `func_id == "slice"` and 10 more | identity tests |
   | `call_dispatch_builtin_fallback.py` | `_try_emit_named_builtin_fallback_call` | `func_id in BUILTIN_FUNC_SPECS` | the proved name is in `BUILTIN_FUNC_SPECS` |
   | `builtin_guards.py` | `_named_builtin_unproved`, `_FRAME_REFLECTIVE_BUILTINS` | catalogued-only gate | deleted |

   A read without a proof has no identity, so no builtin branch can match.
   It falls through to the class, function and local paths, which select a
   callee only from a source-class or source-function fact (`_class_reference`,
   `Func:` hints), never from a builtin spelling, and then to the live read and
   dynamic call that the chain already ends with. A proved read of `globals`,
   `locals` or `vars` keeps its frame-aware lowering; an unproved one is the
   dynamic call, which the probe shows is correct. Because the dispatchers no
   longer receive a spelling, a later branch cannot test one: the type checker
   (`ty`) rejects a `str` comparison against `PythonIdentity`.
4. **One value authority.** `_emit_proven_builtin_value` lowers a proved read:
   a type tag to `BUILTIN_TYPE` (as today), an exception to `EXCEPTION_CLASS`,
   `NotImplemented` and `Ellipsis` to their constants, and a function to the
   live read. `_load_name_expression` deletes its spelling branches for these
   names (including `locals` and `__import__`); an unproved read is the live
   `MODULE_GET_GLOBAL`.
5. **Guards.** The guard model does not change. A guard-table name in a
   function gets a guard only when the flow does not prove it, and its slow
   arm is now the dynamic call. Bare `super()` keeps its exclusion from the
   call guard.

There is no new op, runtime entry or cache. Every backend already lowers the
operations named above. Persisted frontend records key on
`_frontend_semantic_tooling_fingerprint`, which digests all of
`src/molt/compiler_analysis` and `src/molt/frontend`
(`_FRONTEND_COMPILER_INPUTS`), so a catalog change invalidates them with no
schema bump, as HF-226's catalog change did.

### Invariants

- Every named lowering of a builtin read, call or value, implies a proved
  identity at that read. No name list decides correctness: a builtin without
  a row can never be proved, so it can only lose speed. (`assert` is not a
  builtin read; see the inventory.)
- Under a closed premise with no writer, outside
  `opaque_activation_sources` and `__builtins__` custody, every inventory read
  except the two target-gated exceptions is exact, so it lowers as on base and
  a function needs no guard for it.
- A catalog identity only adds proofs; it never removes the live path.
- The new identities are not shape identities, so they never set
  `callee_elision_safe` lowering (`CallFact.exact_builtin_name` projects shape
  identities only). Their call effects stay `UNKNOWN_EFFECTS` (the default arm
  of `_call_semantics`), and their operand escape is the default, as for
  `OTHER`. No other flow consumer tests them. The census checks the claim
  that discovery does not change.

### Costs

Compile time: per builtin read, the flow's builtin fallback computes a guarded
member identity instead of `OTHER | UNBOUND`: one dict lookup and two mask
operations. Identity masks widen from 82 to 188 bits and member masks from 51
to 157 bits (CPython integers of seven and six 30-bit digits). Persisted masks
are integers in the records, so no key layout changes.

Emitted code: a proved read in a function loses its guard call and slow arm.
An unproved read costs one namespace lookup and a dynamic call, as CPython's
generic path does. Reads become unproved only with a writer, in an opaque
activation, or under an open premise. A normal build has a closed premise for
every admitted module; standalone compilation has none and keeps the guard
for guard-table names, while its other inventory reads (exceptions, `dir`,
`getattr` and the rest) become live. No runtime state is added.

### Test plan

- `tests/test_frontend_builtin_binding_guards.py`: for one name per family
  (`type`, `ord`, `sum`, `sorted`, `abs`, `print`, `getattr`, `ValueError`
  called and read, `NotImplemented`), a `builtins` member writer gives the
  live read and no named operation; the closed premise with no writer gives
  the named operation with no guard. The repro's slow arm emits `TYPE_OF` on
  base, so the new test fails there.
- `tests/test_gen_python_identity_catalog.py`: every guard name has a row; the
  rows cover every `BUILTIN_FUNC_SPECS` name except `pow` and `round`, and
  exactly the `BUILTIN_EXCEPTION_NAMES` spellings that are present on every
  target, each present in `dir(builtins)` of the running interpreter.
- Differentials against CPython 3.12.16 on native, LLVM and WASM:
  - `named_builtin_replaced_repro.py`;
  - one program per newly catalogued callable, type and constant
    (`replaced_builtin_<name>.py`, 37 programs). Each replaces the builtin at
    module level, through a `builtins.` attribute and through a
    `__builtins__` mapping, and calls or reads it from module and function
    scope;
  - `replaced_builtin_exceptions.py`: each of the 69 exception spellings is
    read and checked against `builtins.<name>` unreplaced, then replaced in
    the three forms. The spellings share one lowering, so one program covers
    them;
  - `unproved_frame_builtins.py`: a writer that stores the same object
    (`builtins.locals = builtins.locals`) makes the reads unproved; `globals`,
    `locals`, `vars`, `dir` and `super` are then called at module scope, in a
    class body, a method, a comprehension, a generator, a coroutine and a
    closure.
- Census of the 144 programs, binary sizes and module counts for hello and
  `loop`, the five-pair primitive A/B, and a count of named-lowering operations
  against dynamic calls in the emitted IR of hello and `loop` on base and head
  (every admitted stdlib module included), which shows how many sites lose a
  proof in a normal build.

### Acceptance

The primitive A/B passes when no axis ratio rises beyond its noise bound. The
census passes when no program admits a module that base does not. Binary sizes
and the hello and `loop` module counts must not grow, and the IR count must
show no named-lowering site that becomes a dynamic call in a normal build
without a writer. Every differential cell must pass. If the census or the IR
count shows a change, the lane splits the catalog by family (callables,
constants, exceptions) to find its source before it lands.

### Open siblings

- A class statement reads `__build_class__` from the frame's builtins in
  CPython (`LOAD_BUILD_CLASS`); Molt lowers it directly. It is the same
  mechanism without a source spelling, so it needs a proof query for an
  implicit read. Not in this lane.
- `BUILTIN_FUNC` and `EXCEPTION_CLASS` still find their object by its
  spelling at run time (`lookup_builtin_name`, `exception_type_bits`). A typed
  runtime handle for each catalogued builtin would remove those lookups; it is
  a runtime design, not part of this proof.

### Review record

Round 1 (egm-design and egm-implementation-readiness, Sonnet, design file
only): ten and fourteen findings, no blockers. Fixed in this text: the
changed-function table, the inventory method and the `BUILTIN_FUNC_SPECS`
accounting, the version and platform rule for exceptions, the qualified
exactness invariant, `callee_elision_safe` and operand escape, cache
invalidation through the tooling fingerprint, the corrected mask widths,
named differential files with a frame-builtin differential for every scope
kind, the IR count of sites that lose a proof, the staged fallback if the
census changes, the HF-226 text that this section supersedes, the narrowed
status and the `__build_class__` sibling. Answered, not adopted:

- A source lint against spelled comparisons: the dispatchers now take a
  `PythonIdentity`, so the type checker enforces it, and project policy
  removes scraping audits that the type system covers.
- A per-name exception program: the 69 spellings share one lowering, and the
  one program reads each spelling by name and checks it against
  `builtins.<name>`, which catches a wrong table entry per name.
- Staging the catalog over several lanes: HF-226's census rule is per change,
  not per identity; this lane runs it once and splits only if it fails.
- A version field per exception row: every non-gated spelling exists on every
  supported target (3.12 and newer); the generator test checks it.

## Tests

- `tests/test_python_namespace_writes.py`: every recognized construct, reads that are not writes, and
  object writes that are not namespace writes.
- `tests/test_python_binding_flow.py`: facts survive calls, imports and loops under a premise; an own
  `global` writer, own reflection, a foreign name and graph-written members each keep the conservative
  answer; the member mapping shares the direct-store authority.
- `tests/cli/test_cli_python_source_closure.py`: type-only imports stay out; a named foreign write and a
  package child named like the guarded flag each bring the import back; cached rounds re-analyze
  nothing; a dead relative import is a lexical edge.
- `tests/cli/test_cli_lowering_cache_tooling_scope.py`: the real lowering scope needs no whole-tree
  coverage and excludes the backend and command layers.
- `tests/test_python_namespace_graph.py`: admission-order-independent incremental joins, targeted
  deltas, unknown absorption and immutable premise snapshots.
- `tests/cli/test_cli_module_graph_discovery_authority.py`: exact CPython import oracles, late foreign
  and canonical-member writes, star exports, package children, uninspected owners, deferred-context
  wakeups and source-generation custody; warm scans reuse target parses.
- `tests/cli/test_source_scan_authority_closure.py`: original package-root custody
  across generated/native source views, strict joins and generation rejection,
  two-root namespace identity against an isolated CPython oracle, and cold/warm
  child inventory reuse without interpreting unused children.
