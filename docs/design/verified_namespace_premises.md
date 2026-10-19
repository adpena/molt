# Verified namespace premises

Status: implemented for tooling source closures and user-program discovery (2026-10-16);
operation-wide materialized-graph convergence and source/topology custody are implemented;
frontend premise adoption and target qualification remain open.
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
- The frontend already assumes the narrower premise: `_module_stable_funcs`
  (`frontend/lowering/sema_state.py`) treats a module function as stable when only its own module
  rebinds it.
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
   `(receiver spelling, name, construct, line, execution owner)`. It is conservative by spelling.
   The existing read-only AST admission owns these syntax facts by target language
   version and relative-import package. Binding-core policies, graph premises,
   activated bodies and arguments do not change syntax. Graph and binding queries
   therefore share that source generation's summaries; intrinsic classification
   consumes the full scan's summary directly. Unadmitted mutable trees recompute.
   Canonical private-name resolution precedes publication, so a class's
   `target.__member` and `global __name` retain CPython's mangled bindings while
   literal strings passed to `setattr` retain their exact spelling. Summary schema
   4 rejects records from before this normalization. This uses the admitted AST's
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
   its actual premise to runtime-import policy. Frontend premise adoption remains separate work.

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

| | main 42e2cf65f | this design |
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

A named writer delta wakes only its prior target sources and reached body contexts.
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
reached Python import operation. Initialization, full inventory and deferred
owner/argument scans therefore use their actual execution regions and namespace
premise for both requests and the protocol bit.

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
