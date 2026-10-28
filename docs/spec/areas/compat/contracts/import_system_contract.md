# Import System And Modules
**Spec ID:** 0213
**Status:** Draft
**Priority:** P1
**Audience:** compiler engineers, runtime engineers, tooling engineers
**Goal:** Define Molt's import system, module objects, and deterministic
resolution rules.

---

## 1. Scope
This spec defines:
- module loading and caching,
- import resolution and `sys.path` policy,
- package/module metadata expectations,
- compatibility boundaries for dynamic imports.

Source implementation: project-root/package builds, relative imports (explicit and
implicit) with deterministic package resolution for the currently lowered
paths, `__init__` handling, covered namespace-package stubs/basics, a Rust
import transaction for the active importlib/`builtins.__import__` runtime paths,
ordinary source import payload lowering for the focused active paths,
transaction-owned graph-proven `fromlist` child auto-import/binding for the
covered native path, static package `__all__` child discovery for source
`from package import *` and canonically identified call-form star requests, CPython 3.12 package-context resolution for the covered
relative `builtins.__import__` cases, public resolver validation for
`importlib.import_module` and `importlib.util.resolve_name`,
`FileLoader`/`SourceFileLoader.load_module` execution through the Rust
spec-execution transaction, and persisted source-only import requests keyed by
source and compiler/tooling policy inputs. Filesystem-dependent edges are
completed live on every graph walk; completed graphs and derived imports are
not persisted. Remaining transaction work is not closed:
public importlib API validation outside the covered import-module/resolve-name
resolver and load-module cases, dynamic/broader CPython `fromlist`
star/`__all__` expansion, and namespace-package edge cases still need structural
reconciliation against CPython 3.12.


The joined module/import implementation is not yet execution-qualified. Native
and WASM public imports, initialization/rollback, C-API publication, and
provenance-bound startup, import-cost and memory qualification remain required
on the final joined source and generated consumers. Earlier donor source or
static controls do not establish those results. Luau import capability remains
an explicit gap; the catalog and shared source lowering do not imply target
support. The release ledger retains these open obligations.

Compiler and tooling dependencies use the actual frontend interpreter's import
semantics; compiled program dependencies use the selected target version.
The pinned [CPython 3.15.0 `resolve_name`](https://github.com/python/cpython/blob/d220409a13b77de7a6c30abe16ea90db01528edc/Python/import.c#L3498)
retains explicit `__package__` precedence, `__spec__.parent` and name/path
fallback. The shared source resolver preserves those anchors separately from
required runtime callbacks, warnings and failures. Eager source analysis does
not require lazy-import implementation: shared AST admission refuses PEP 810
before producing eager facts or code. These developer-side dependency and
admission checks introduce no guest interpreter or guest verification work;
they do not admit Python 3.15 as a compiled-program target.

Import bedrock uses one interpreter-owned ordinary Python dictionary from
bootstrap onward. `sys.modules` exposes that object; arbitrary values, `None`,
deletions and callback mutations are authoritative. Semantic interpreter sys and
builtins roles and temporary initializer/private execution owners have separate,
explicit lifetimes. No private name-keyed module store adopts or replays public
membership.

Ordinary Python imports, including literal statements whose bodies are admitted
in the application catalog, consult the dictionary first and then the same
ordered `sys.meta_path` protocol in the runtime finder core. The frozen/bootstrap
facade and `util._find_spec` project that same protocol; rebinding the public
`util.find_spec` helper affects direct helper calls, not ordinary imports or reload. The compiled
catalog is one finder/provider; it proves executable-body custody, not an immutable
finder binding. A required source-role fact distinguishes actual Molt stdlib
providers from same-name user sources. Availability is checked only for that
provider, including shared body execution through explicit source loaders.
The fixed synthetic `__main__` row represents the compiler-selected program
entry body and is an explicit source-execution capability, not an advertised
stdlib provider; the original named row retains its provider flag. The backend
binds that row only to the selected entry initializer, so callers cannot use it
to select an arbitrary named stdlib body. Named loader execution and aliases
remain subject to their actual provider facts.
FileFinder may decline its catalog-origin candidate after actual target-filesystem
canonicalization proves the physical coordinate matches; a historical compiler
path alone grants no target ownership. Earlier/custom finders and external
backport paths retain their own admission. Execution dependency errors occur only
when the selected admitted body executes. Earlier user finders may supply any admitted loader.

The dense per-build `ModuleRegistry` (`molt.cli.module_registry`, schema4) and
`MODULE_INIT_TABLE` remain the authority for compiled body identity and execution.
`molt_module_ensure(ModuleId)` owns catalog initializer state. Entry/sys/spawn,
compiler-owned provider initialization and actual import-support module bodies
use this explicit internal capability. Python functions defined in those support
modules still perform ordinary public imports. Private loader/runpy execution
uses its scoped namespace owner and does not replace public import resolution.
The backend cannot replace a public transaction with numeric initialization merely
because its literal name has a catalog row.

A completed public cache hit keeps its existing dictionary/spec-probe policy and
avoids finder dispatch. A cold ordinary miss pays admitted initial finder completion, one
ordered finder traversal and the admitted spec transaction rather than a compiled-first
attempt followed by fallback. These callback/locking/allocation costs require
native and WASM qualification and the existing cached-import probes; source
structure alone is not a performance result. See the current
[storage and execution contract](../../../../design/foundation/import_bedrock_pr2_sys_modules_view_cutover.md)
and [catalog design](../../../../design/foundation/import_bedrock_frozen_module_layer.md).


The public `__import__` transaction preserves the original request when selecting
its result, following [CPython 3.12.15](https://github.com/python/cpython/blob/v3.12.15/Python/import.c#L2700-L2745).
A false fromlist selects the first requested component: an absolute dotted
request returns its root, while a relative dotted request keeps the resolved
package prefix. An empty or undotted relative request returns the imported leaf.
Fromlist truth is observed once, after importing the leaf. A callback that mutates
the selected cache entry retains the same protocol: absolute requests import the
root again; relative requests read the exact cache entry, return a present `None`,
and raise `KeyError` for a removed entry. The result owns one reference even when
multiple cache keys alias the leaf. Result selection uses borrowed name slices;
it adds no name allocation or dictionary read for empty or undotted requests.

An initialized cached leaf is observed before parent resolution. A missing or
`None` parent cannot invalidate that hit. The locked cold owner resolves only
the immediate parent and rechecks child publication before reading parent
metadata. Parent imports reuse the caller's Python or builtin transaction;
cache-only lookup cannot enter this owner. Recursive parent admission uses the
existing execution-stack limit and adds no charge to a warm hit.

The cold protocol follows the admitted Python target: 3.12 accepts a
parent-published child value including `None`; 3.13 and 3.14 accept non-`None`
values, inspect the retained parent spec's initialization state after its path,
and recheck the child after an initializing-parent import. The observed parent,
path and spec survive finder and loader callbacks. Fresh child publication
acquires the current parent before retiring its prior owner; a removed parent
raises `KeyError`. This preserves callback ordering even when retirement changes
the public dictionary. Private circular-child tracking and broader concurrency
qualification remain open; this contract does not qualify the 3.15 source pin.

Public argument validation and `importlib.util.resolve_name` precede finder
discovery and require no host filesystem authority. Following
[CPython's pinned import_module](https://github.com/python/cpython/blob/v3.12.13/Lib/importlib/__init__.py)
and [finder protocol](https://peps.python.org/pep-0451/), a relative empty suffix
with package `pkg` resolves to a valid request for `pkg`. Compiled-provider
absence then permits the existing spec/finder lane; it does not prove overall
absence. A host path search requires `fs.read` under the
[capability contract](../../../../CAPABILITIES.md). Authorization denial
propagates as `PermissionError`, distinct from a finder returning no spec.
Cached, compiled and in-memory finder hits do not gain filesystem requirements
from a different branch. Source discovery does not authorize interpreting an
arbitrary source file: the source/sourceless loaders execute admitted compiled
bodies, and dynamic native extensions retain their separate target, ABI and
execution admission. No host or fallback Python interpreter is introduced.

Finder dispatch follows the ordered protocol in pinned
[CPython 3.12.15](https://github.com/python/cpython/blob/v3.12.15/Lib/importlib/_bootstrap.py#L1144),
[3.13.16](https://github.com/python/cpython/blob/v3.13.16/Lib/importlib/_bootstrap.py#L1144),
[3.14.8](https://github.com/python/cpython/blob/v3.14.8/Lib/importlib/_bootstrap.py#L1147)
and [3.15.0](https://github.com/python/cpython/blob/v3.15.0/Lib/importlib/_bootstrap.py#L1103).
3.12 walks the live finder iterable; 3.13+ snapshots it first. Each finder lookup
and callback holds the same runtime-owned reentrant lock exposed by `_imp`;
post-callback cache/spec reads occur after release. `util.find_spec` retains its
separate relative-name, parent-path and cached-spec API. No import enters that
public helper to bootstrap its own implementation.

The initial runtime catalog finder exists before Python importlib facades. Actual
stdlib source-role capability and the body's defining namespace, not a cached
object with a matching spelling, own default PathFinder/hook publication. Exact
initial-list identities and backing epochs prevent replacing or repairing public
customization. Pending owners retire on completion, cancellation, failure and
shutdown. The compiler's one reached-call identity/demand authority admits this
finite support source even for public low-level intrinsic entrypoints; merely
acquiring, rebinding or leaving a call unreachable does not retain it. There is
no unconditional full-stdlib expansion for a microbinary.

The protocol uses ordered control flow; persistent initialization state stays in
the existing module registry. [V8's module state model](https://chromium.googlesource.com/v8/v8/+/refs/heads/main/src/objects/module.h)
likewise separates legal lifecycle transitions and terminal errors, although
Python's transitions and exception behavior remain governed by CPython.
[rustc's incremental query model](https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation-in-detail.html)
requires pure computations and tracked dependencies for result reuse. Arbitrary
finder callbacks have neither guarantee, so their identity cannot justify a
cached result. These are implementation constraints, not an additional runtime
state-machine framework or a development-time verifier shipped to users.
[MLIR's analysis preservation rules](https://mlir.llvm.org/docs/PassManagement/#analysis-management)
make the corresponding reuse obligation explicit: a transformation must establish
which analyses remain valid. Molt's path-to-finder cache is part of the Python
protocol; it does not establish preservation of arbitrary finder results.

| Discovery state | Next action or terminal outcome |
| --- | --- |
| Public cached module | Return its `__spec__`; a cached `None` returns `None`, while missing/`None` metadata raises `ValueError`. |
| Uncached child | Import the parent through the existing module transaction, then pass its exact `__path__` object to the finders. |
| Registered finder returns `None` | Advance only to the next registered finder. Exhaustion returns `None`; an empty path first calls `_warnings.warn("sys.meta_path is empty", ImportWarning)` and propagates its failure. |
| Finder raises | Propagate that exception after the single specified call; `TypeError` is not an arity probe. |
| Finder supplies a spec | Select any callback-published public spec under CPython's precedence rule, apply extension admission, then return it. |

Finder results are not memoized by finder identity: both absence and successful
results can change on the next invocation. `PathFinder` alone consults path hooks
and owns the public path-to-finder cache, including cached `None`. A hook returning
`None` finishes that path's hook selection; only `ImportError` rejects a hook.
`FileFinder` is a native physical lookup leaf over its explicit directory, with
no hook reentry, hidden bootstrap path injection, or module-name authorization
exception. Raw extension discovery and metadata validation share the same native
resolution owner. Invalidating a stateless `FileFinder` preserves its identity;
`PathFinder` retires negative and relative entries and calls retained finders'
invalidators.

Cache disposition is lexical bookkeeping and does not import the filesystem
facade or require filesystem authority. It follows the import-specific root
predicate in [CPython 3.12.13](https://github.com/python/cpython/blob/v3.12.13/Lib/importlib/_bootstrap_external.py)
and [3.15.0](https://github.com/python/cpython/blob/v3.15.0/Lib/importlib/_bootstrap_external.py):
POSIX requires a leading slash; Windows uses the existing native root parser,
and a single leading separator is current-drive-relative. Generic
`os.path.isabs` is not this contract. A callback exception terminates traversal
and preserves prior effects. Native Windows malformed, device, byte-path and
Unicode root/error qualification remains part of the open target matrix.

Invalidation takes one linear snapshot to tolerate callback mutation. Apart
from callbacks it performs no I/O; the Windows projection additionally creates
the native parser's root components and tuple per inspected nonempty path.
These are cold invalidation costs, with no new persistent state or work added
to ordinary discovery. Development conformance controls do not enter guests.

Protocol traversal is linear in the finders and path entries actually visited;
each selected callback runs once. It performs no whole-cache identity scan,
result serialization or speculative retry. The immutable compiled catalog reuses
the module registry's existing name index. Physical lookup and extension metadata
validation retain their separate I/O costs and admission decisions.

Package metadata comes from the generated module prologue and existing loader
transaction. The importlib facade does not reconstruct `__path__` from `__file__`
or import filesystem machinery to repair that metadata. Describing a compiled
provider therefore does not initialize `os` or acquire its filesystem providers.

The existing runtime-owned `BuiltinImporter` is registered before `PathFinder`
for Molt's compiled providers. Its read-only projection uses the admitted module
registry and the shared compiled loader; describing a provider does not execute
or publish it. Custom meta paths remain authoritative. This is Molt's existing
compiled-provider classification, not a claim that compiled source modules are
CPython builtins. Full loader-plan, namespace-refresh, concurrency and target
qualification remain tracked in the release ledger.

Gates: `tests/test_module_registry_gates.py` (G1/G3/G7, single-owner and
chain-is-gone structural gates) and the runtime G4 state machine unit
(`g4_ensure_state_machine_transitions`).

---

## 2. Module Objects
`sys.modules` entries determine cached import identity. An empty namespace,
private-only attributes, runtime bootstrap markers, or absent metadata never
trigger eviction or re-execution. This includes a package published before its
body imports a child. The initialization probe of `__spec__._initializing` retains
the module across user callbacks: importlib propagates probe failures, while
built-in `__import__` suppresses them, matching CPython. Other namespace contents
do not govern initialization. A cached `None` blocks the import.

Every module must expose:
- `__name__`, `__package__`, `__file__` (when applicable),
- `__spec__` with loader metadata,
- `__dict__` for module attributes.

`__file__`, `__spec__.origin`, a package's `__path__` and every `co_filename`
report the module's logical source path. A program module reports the absolute
file it was compiled from, as CPython does. A bundled stdlib or other
toolchain-owned module reports its place in the compiler's `molt` package, for
example `/__molt__/stdlib/json/__init__.py`, never the checkout or install that
built the program
([build-path independence](../../core/0025_REPRODUCIBLE_AND_DETERMINISTIC_MODE.md#23-build-path-independence)).

Modules may be:
- compiled Molt modules,
- standard library shims,
- source-recompiled extension modules under the libmolt ABI and capability contracts.

All execute within Molt under the [Python execution boundary](cpython_bridge_policy.md).

Deleting an admitted C extension from `sys.modules` reenters the same runtime
initializer transaction. Legacy single-phase definitions (`m_size == -1`)
produce a fresh module from the first successful initialization's dictionary;
post-initialization namespace mutations do not alter that snapshot, and native
functions retain the original receiver stored in the copied dictionary.
Reinitializable single-phase definitions (`m_size >= 0`) and multi-phase
definitions rerun their initializer. The existing C-API runtime state owns the
legacy snapshot independently of the current `PyState_FindModule` entry, so
`PyState_RemoveModule` does not discard it. Explicit legacy `create_module`
replay also honors an existing module in `sys.modules`, merging the snapshot
into that same namespace; absent/non-module entries receive a raw new module
before the merge. Existing C-API metadata remains attached. Single-phase `create_module`
publishes its already-executed result on the first call too; reinitializable
single-phase calls rerun PyInit and replace the prior public/private publication.
Multi-phase `create_module` still defers publication and execution to its caller.
All admitted extension publications own their exact identity, including
multi-phase execution when a stale private cache entry exists. Completed table
projections are reconciled at that same boundary; foreign initializer and
explicit execution custody remain intact. Rollback detaches only transaction-owned
identities, preserving a different public replacement.

Single-phase callback ordering follows the selected Python target: first import
registers extension state and its legacy snapshot before publication on 3.13+;
3.12 first imports and repeatable reloads publish before state registration.
Successful import identity survives `PyState_RemoveModule` for both legacy and
repeatable definitions. Each initializer/name/origin identity has one definition
owner even after nested initialization; transferring it preserves independent
PyState ownership and releases an orphaned snapshot. PyInit failure publishes no
snapshot. On 3.13+, a later
publication failure retains the already-registered extension state, matching the
first-import contract. Runtime shutdown detaches owners before releasing them.

Source initialization has one ordering across entry paths and targets: publish
the module object, establish its lexical frame and captured builtin namespace,
then construct loader metadata before publishing native providers or executing
the source body. Generated metadata obtains the canonical `ModuleSpec` class
and compiled-loader singleton directly from the runtime. It never imports the
machinery facade to construct them: machinery's dependencies need metadata
before its source body publishes these identities, and compiled modules must
not acquire filesystem dependencies merely to describe their loader.
Imported modules, including machinery itself and modules compiled without the
facade, use the same class; script entry points retain `__spec__ = None`.
Explicit and inherited builtin namespaces remain authoritative; initialization
does not re-import `builtins` unconditionally or refill a mutated namespace.
Generated annotation callables and module chunks run after this bootstrap.

Lexical execution does not resolve its namespace through `sys.modules`.
Functions retain their captured globals and builtins across public module-cache
replacement, deletion, and re-import. Module bodies and generated chunks carry
their held module owner explicitly. Frame ownership, exception cleanup, and
annotation publication follow the compiler's registered entry and chunk
identities; user module and function names do not select execution roles.
Raw admitted module dispatch establishes its handler baseline at the caller's
current stack depth. An initializer's explicit raise can therefore clear its
own handlers while the enclosing importer retains the handled exception for
bare re-raise. The same scalar scope covers ordinary imports, direct loader
execution, native, LLVM, JIT and WASM; chunks inherit their enclosing body.
Frontend handler cleanup owns stack depth. This boundary adds no context
snapshot, heap allocation or work to cached imports.

Generated module metadata, the machinery facade and bootstrap-free extension
initialization share one runtime-owned `ModuleSpec` class and its initializer.
The runtime likewise owns `_MoltLoader`, `BuiltinImporter`, `FrozenImporter`
and the shared compiled-loader singleton; the machinery facade publishes these
same mutable, subclassable classes. Their load and execution methods use the
canonical compiled-module import transaction, including name validation,
exception propagation and failed-publication cleanup. This is not a claim of
complete CPython builtin/frozen-loader API compatibility.
Public `BuiltinImporter`, `FrozenImporter` and `ModuleSpec` expose their
`_frozen_importlib` class origin. The loader classes inherit ordinary object
representation instead of defining loader-specific `__repr__` strings.
`ModuleSpec.__repr__` follows CPython's optional-field selection, subclass
name, field formatting and attribute-observation order; field errors propagate.
Frozen bootstrap payloads publish these runtime identities without reading
the machinery facade; internal loader classification, including reload's
compiled-loader detection, does not change when a facade alias is rebound.
Public source/spec loader callbacks continue to use their explicit Python
callables and objects. The frozen-external facade still requires machinery for
its file, source and extension loader classes.
Module-name recovery reads `ModuleSpec.name` (not `ModuleSpec.__name__`) in
loader coercion, reload and resource lookup. Coercion propagates failure to
publish a recovered module name; it does not clear a failed attribute write.
`cached` and `has_location` are runtime-owned properties, following
[CPython 3.12's ModuleSpec and cache derivation](https://github.com/python/cpython/blob/v3.12.15/Lib/importlib/_bootstrap.py).
Location assignment coerces truth once and preserves the previous value if
that coercion fails. Cache derivation is lazy: a non-`None` cached value is
returned unchanged, while assigning `None` permits derivation again from the
current origin. Source and bytecode suffix dispatch observes ordinary
`origin.endswith` calls and the defining frozen-external namespace's mutable
suffix policy. Machinery initially exports the same list objects; rebinding
only its facade attribute does not replace the defining policy.

Before that defining source body succeeds, one runtime bootstrap dictionary
owns the suffix lists. Payload preparation stages values; the verified body
commits its defining globals only after all assignments succeed. Failed
initialization keeps the bootstrap owner, and a later distinct module body
cannot retarget the first successful defining namespace. Warm cache reads do
no filesystem access or imports. Cold derivation allocates suffix tuples and
uses the existing cache-path primitive. The focused `importlib_module_spec.py`
differential receiver exercises property callbacks, mutation, shared policy,
rebinding and ZIP metadata on native and WASM; enrollment is not qualification.
Pure path discovery leaves source cache paths unset, so the first property
read observes the current implementation tag and bytecode suffix instead of
an eagerly captured path. The shared default formatter follows
[CPython's filename partition and tagged layout](https://github.com/python/cpython/blob/v3.12.15/Lib/importlib/_bootstrap_external.py#L463-L525)
while retaining Molt's declared `sys.implementation.cache_tag`.

The constructor still accepts positional `origin` and `is_package` and coerces
the name to `str`. Complete CPython signature and equality conformance,
`importlib._bootstrap_external` alias identity, and all cache-path options
remain outside this partial contract.

Cache publication borrows its name and module arguments and returns `None` on
every successful path, including first-init-wins duplicate initialization.
It never hands a borrowed cache entry to a caller as an owned result. Each
cache or module-table slot retains its own reference; re-publishing an aliased
entry acquires the replacement owner before releasing the old one. Native,
WASM, intrinsic and C-API callers share this ownership contract.

Module/code publication failures occur before any frame-entry attempt and must
not exit the caller's frame. From the frame-entry attempt onward, failure cleanup
balances that attempt exactly once, including a failed builtin capture that did
not push a Python frame, and rolls back the failed module publication. Entry
failure must be checked before modifying locals, creating metadata or running
body operations; otherwise the caller's still-active frame could be mutated.
Metadata construction and native-provider publication use that same cleanup path.

---

## 3. Import Resolution

### 3.1 Deterministic `sys.path`
- `sys.path` is deterministic for a given build.
- Compiled binaries do not read ambient `PYTHONPATH` or `VIRTUAL_ENV` during
  runtime bootstrap. Build-time `--respect-pythonpath` may include
  `PYTHONPATH` entries in the compiled module graph; runtime source roots must
  be explicit through `MOLT_MODULE_ROOTS`.
- Runtime mutation of `sys.path` is allowed only when explicitly enabled.
- Resolution order is stable and documented in build metadata.

### 3.2 Allowed Forms
- `import x`, `import x as y`
- `from x import y`
- `from x import y as z`
- `from x import *` (module scope only; honors `__all__` when present, otherwise skips underscore-prefixed names)

Import-flow analysis resolves each request from the metadata visible before its
bindings are published. Explicit aliases to `__package__`, `__spec__`,
`__name__`, and `__path__` update that same authority with their lexical scope.
Star imports invalidate metadata and callable-identity assumptions because an
owner's `__all__` may export those names. A later relative import then requires
runtime custody; intrinsic-backed forwarding does not make the anchor static.
Exception paths retain states from partially completed multi-name imports.
`ModuleSpec` parent inference consumes the canonical call-site identity,
result and effects. Aliases can preserve that proof; a familiar spelling,
class-local import or rebound constructor cannot create it. The syntax-only
effect projection does not carry a separate registry of pure callable names.

### Source demand and packaged availability

Imported sources begin with module initialization. Binding facts retain the
source owners of values through aliases, containers, rebinding and publication;
one discovery worklist admits the deferred bodies those values can invoke or
expose. Function calls, returned or escaped callables, class construction and
unknown instance use retain the relevant callable families, including base and
metaclass hooks. Type-alias values remain deferred until demanded. Explicit
execution roots retain their conservative full-depth scan contract.

Module namespace summaries are may-provenance across initialization, not an
initialization-order proof. Cyclic importers can observe earlier publications or
invoke a hook before a later export exists. A proven static member selection
binds its argument to the existing parameter facts and branch-liveness evaluator;
unknown or escaped hook use widens. A star import copies names and exposes
nothing by itself: each module-namespace read, and each member selection on the
importing module, selects its name from the star-imported modules, and only an
escaped importing namespace exposes those modules whole. A static
`importlib.import_module` or `__import__` call result is an import binding: its
member reads select from the modules the call imports, and passing the result to
unknown code exposes them. No standard-library
helper-name list governs availability. Class-family and reflection escape remain
conservative, so this contract does not promise a minimal module set.

Selected owner and argument facts travel together through scan authority,
source-request caches, runtime-support detection and binary-image diagnostics.
Read-only discovery reuses one parsed source admission and owner index, and
revalidates its captured source bytes at the fixpoint. Filesystem-dependent
resolution still runs through the existing live resolver. Graph admission
packages a dependency; the original import operation continues to determine
when its initializer executes. Native and WASM execution qualification is
separate from host graph and binding-fact validation.

### 3.3 Dynamic Imports
- Import-call dependency binding distinguishes a proven invalid call from an
  unresolved expansion. Missing, excess, duplicate, and unexpected arguments
  remain ordinary runtime calls so their argument expressions execute and their
  binding `TypeError` can be caught; they do not request an imported module.
  Calls with unresolved `*args`/`**kwargs`, dynamic `__import__` fromlists or
  levels, and star fromlists use the existing source/AST/catalog custody of
  executable graph scans. A known level and globals/package context remain
  part of the base request even when its fromlist is dynamic, invalid or star:
  level-one `child` in `pkg.entry` discovers `pkg.child`, never a bare `child`.
  Explicit foreign metadata retains its own authority; unknown foreign metadata
  does not acquire a lexical package fallback. Only unresolved binding or level
  uses a bare-name candidate. Candidates remain separate from semantic imports
  until runtime custody is established. Strict source-closure scans still require their dynamic
  import manifest. Argument expressions retain their own import edges in both
  cases. This classification applies to aliases and to `__import__`,
  `importlib.import_module`, and the scanner's `importlib.util.find_spec` calls;
  it grants no imports outside the admitted runtime catalog.
  Import call identity comes from canonical binding facts, including aliases;
  transaction or resolver spellings bound to arbitrary objects grant no import
  authority. Helper forwarding and statement/call star collection share the
  same scanner and request planner. Statically resolved call-form star bases
  feed the existing live package `__all__` child expansion. A discovery-only
  base keeps its expanded children in discovery rather than semantic imports.
  Source requests and their cache carry explicit `dynamic_star_modules`
  provenance; an ordinary import of the same base does not promote those
  children. Cache entries lacking that projection are invalidated;
  dynamic or mutated `__all__` remains outside this static expansion contract.
  Both star projections come from the same cached source scan, with the same
  resolved source path and custody admission. Runtime-support detection consumes
  canonical possible call identities and completion reachability, including
  conditional aliases. A possible globals()/locals()/vars() result retains its
  canonical namespace alternative for lexical discovery only. Strict metadata
  projection and callback-free globals mutation share the exact namespace plus
  evaluated exact built-in `dict` admission predicate. Deferred `__globals__`,
  `f_globals` and imported `globals` spellings confer no additional authority.
  Bare builtins, imported builtins and canonical module members share the same
  identity and mutation guards. Re-importing a member after callbacks or an
  explicit replacement cannot restore exact identity. A possible no-argument
  `inspect.currentframe` call retains a possible frame/globals discovery path
  with arbitrary-Python effects; it supplies no strict metadata authority.
  Explicit `__package__`/`__name__` global loads likewise borrow loader metadata
  only from stable activations with callback-clean module metadata storage at
  the read and import invocation. Both points must retain loader-pristine raw
  storage: an unbound identity after deletion is not an untouched loader slot.
  Deleted-slot tombstones survive joins and callback-domain growth. Captured
  unknown loads cannot borrow metadata written or deleted by a later argument. Exact globals mappings are checked after all
  arguments, before invocation: an earlier globals() capture does not freeze
  metadata against later callbacks. Binding flow owns this storage predicate,
  including absent-slot taint; import consumers do not classify callback syntax.
  Lexical discovery twins retain their candidates separately. Proven explicit
  metadata values remain authoritative in any scope. Namespace
  provenance alone does not authorize `__setitem__`/`__delitem__` identity,
  subscript publication, stored-argument retention, or bound-method cleanup:
  a `FunctionType` activation can use a dictionary subclass. Module bootstrap
  supplies its exact dictionary proof to eager descendant scopes through the
  shared activation-namespace ownership fact, including classes and eager
  comprehensions. Deferred activations and their eager descendants do not
  inherit the bootstrap proof. Their unproven receivers retain descriptor,
  mutation and release callbacks; class-local mappings are a separate owner.
  Relative statements use the same binding-storage proof at statement entry
  that exact current-globals calls use at invocation. Prior relative imports,
  getters and arbitrary callbacks invalidate it conservatively, including when
  no writer is visible in the source: shared references and `sys.modules` can
  reach the metadata. This precision cost does not exclude external reflection
  or justify a second callback classifier. Deferred statements retain lexical
  source-dependency candidates, including projected source-state possibilities,
  in the existing discovery fields and require exact source/AST/catalog custody
  for execution. Local analysis records an explicit semantic or development
  source-dependency purpose. Both full-depth tooling and eager lowering source
  closures retain known source-state candidates in separate discovery rows when
  callbacks or deferred activation prevent execution sealing. Unknown source
  metadata still needs the existing unresolved-import count and explicit target
  manifest; a lexical twin cannot replace that contract. Semantic local scans
  and intrinsic status consume only proven metadata. Cache policy identities and
  payloads preserve this separation. Eager traversal uses canonical lexical
  regions, including branches, defaults, decorators and class bodies while
  excluding deferred function/generator bodies and lazy annotations. External
  native support-file discovery unions graph candidates before capturing file
  custody; product execution still requires the exact source/AST/catalog gate.
  The shared import planner owns both source-state and lexical discovery, with
  their provenance retained separately. A known source branch remains a candidate
  even beside an unknown or erroneous source branch. Candidate presence does not
  establish completeness: the planner separately records whether every source
  context resolved without an error or dynamic anchor. Local dependency closure
  keeps partial source candidates in their original owner/fromlist groups and
  retains unresolved diagnostics and obligations; lexical candidates cannot
  fill those gaps. Product graph and external support-file discovery may
  union both candidate classes while retaining their exact runtime custody gate.
  Discovery never grants intrinsic status.
  Source candidates and execution metadata are separate views of the same
  canonical completion transfer. A callback-capable store can revoke execution
  custody without deleting a source-declared package choice. That choice remains
  accompanied by an unknown alternative: finalizers, descriptors and protocol
  callbacks can select metadata not declared at the import site. Dictionary
  replacement/deletion publishes its change before the previous value is released;
  source candidates follow that order while preserving the callback obligation.
  Discovery consumers request the candidate view explicitly; semantic-only
  analyses do not compute it. Completeness uses canonical binding storage proofs
  at metadata reads, import invocations, and import-statement entry. Missing or
  revoked proofs retain an unknown alternative beside source candidates; runtime
  catalog admission is a separate gate and is not a source-completeness input.
  Exact evaluated scalar facts remain authoritative independently of namespace
  storage. The candidate view reuses cached binding facts and has its own immutable
  state map in that context projection. Discovery derives from the cached strict
  context projection and adds one bounded source transfer, not another strict
  transfer, binding fixpoint or traversal per import site. The source pass also
  checks demand proofs when strict metadata uses the invariant-state fast path;
  the shared core retains canonical assignment effects, including no-effect
  stores, so their absence cannot invent callback uncertainty.
  Explicit unknown writes and unresolved sibling branches retain
  their diagnostics/manifest requirements. Statements and dynamic calls both
  consume the candidate view without sealing it as runtime metadata. Binding,
  local closure, and product scan cache schemas invalidate the older projection.
  Eager and full-depth local analysis use the same final unresolved-import
  validator. Candidate-only policies retain diagnostics. Complete tooling
  consumers explicitly select `unknown_relative_sources="local_inventory"`:
  a canonical unknown package/spec/name anchor on a literal relative statement
  or import call becomes a typed source-coverage obligation, separate from
  semantic requests and nonliteral import diagnostics. Definite invalid operands
  remain errors; dynamic names/levels/fromlists and argument expansions retain
  their exact checked manifest contract. A manifest's declared count is never
  discounted because an inventory covers local bytes.
  The existing local resolver owns the complete inventory under admitted search
  roots and the consumer's allowed prefix. It follows its forward shadowing,
  regular-package precedence and namespace search locations, retains aliases,
  and rejects enumeration errors or cyclic directory topology. Suffix-only
  matching is insufficient: any local owner may execute before a requested
  suffix fails, even when that owner is a plain module. Relative level does not
  restrict an unknown anchor to the source file's lexical ancestors.
  Inventory membership, root order, module aliases and namespace topology are
  captured afresh per operation, together with exact source bytes. Persisted
  analysis stores symbolic obligations, never a stale expanded inventory. One
  complete inventory satisfies all such obligations in that traversal. Files
  included only as speculative owners are hashed without parsing or recursively
  analyzing them; ordinary reached graph sources retain syntax/error validation
  and checked dynamic manifests. Deferred bodies remain outside eager analysis.
  Policy identity and graph schema separate this coverage from candidate-only
  and semantic queries, including warm-cache use. Coverage cannot authorize
  execution metadata, intrinsic status, or a guest host-Python fallback.
  The frontend semantic fingerprint does not use this graph. It hashes the
  declared producer set `_FRONTEND_COMPILER_INPUTS`
  ([design](../../../../design/frontend_semantic_fingerprint.md)); tests check
  that set against every import a producer can run. Installed semantic
  snapshots consume the admitted whole compiler-generation identity, with a
  distinct frontend semantic cache namespace. Verified generation
  identity proves tooling inputs, never mutable runtime package metadata.
  Namespace method identity transports possible `__setitem__`/`__delitem__`
  alternatives even when globals lookup or a replaceable activation can return
  another mapping. One argument-shape admission serves binding and source
  transfer. Only exact receiver/callable proof authorizes execution mutation;
  source projection keeps a possible publication with an unknown alternative.
  Exact builtin sequence indexing and slicing share normal-result and protocol
  facts across binding, expression and syntax-effect projections. Unknown index
  or slice-component protocols remain callback-capable; dictionary key lookup
  remains conservative. Retiring evaluated operands can run finalizers even when
  the selected scalar is exact. Index evaluation expires captured mutable-content
  facts before selection, and selecting a mutable child never grants fresh-owner
  custody. These facts prevent inert literal indexing from revoking metadata
  proof while preserving callback obligations for the full subscription path.
  Source discovery records metadata scalar reads at their evaluation points.
  An explicit dictionary keeps those captured field values through later
  argument effects, including nested literal unpacking; a real `globals()`
  mapping keeps live invocation-time contents. One dictionary transfer serves
  strict values and source alternatives, and both source consumers use the same
  captured-operand projector. A missing metadata name is not a captured absent
  dictionary member and cannot invent a `__name__` fallback. A possible current-
  globals identity contributes a candidate and an unknown operand alternative;
  only an exact identity exhausts that operand. Dictionary keys consume canonical
  expression-result facts, including clean bound strings. Unknown keys can
  replace metadata and invalidate completeness in both strict and source views;
  proven exact non-string keys cannot select a metadata member. Later explicit
  fields restore only the fields they overwrite. None of these
  source candidates relaxes exact source/path/AST/catalog execution custody.
  Discovery never authorizes replacing runtime globals/package operands with
  loader metadata. Helper forwarding retains the original import-call fact;
  the helper invocation cannot certify different captured operands or a
  deferred function's replaceable activation globals. Explicit None and other proven falsy scalar fromlists skip child
  processing; they are not confused with unknown source-point facts.
  Negative levels, missing packages/globals and other call-resolution errors
  remain runtime operations under exact source/path/AST/catalog custody, so
  their exceptions stay catchable. Canonical expression results retain exact
  builtin unary numeric values, including through clean bindings and helper
  arguments. Import levels distinguish bool/int values, known type errors and
  unknown index callbacks; metadata retains known invalid values through current
  and foreign globals. Both graph and strict local-import consumers use these
  facts. Unknown, rebound and callback-dependent values stay unknown. Graph discovery records the custody need
  without fabricating a successful import. Relative statements whose runtime
  resolution raises no-parent or beyond-top ImportError use that same exact
  custody gate and existing guest transaction. Strict scans still reject missing
  or mismatched custody; other statement resolution errors remain build-fatal.
- Build-time graph discovery separates module-init closure from future runtime
  behavior. Graph seeding does not grant full-depth scan authority: application,
  declared static, spawn, and native-support roots are full-scanned; profile core
  modules, package parents, and transitive dependencies contribute module-init
  edges plus the canonical named static helpers. The graph cache keys and
  validates this distinction. Runtime import protocol owners are full-scanned
  only under their exact source/AST/catalog custody, established after ordinary
  closure admission. Neither graph presence nor a profile grants dynamic import
  permission. One scan-mode projection serves discovery and protocol detection.
- Static branch and import pruning requires a canonical binding fact at the
  expression's execution point. A preceding import, arbitrary-Python callback,
  or reference release can populate a module binding and run its finalizer when
  a later store replaces it; that finalizer can rewrite the newly stored value.
  Such a guard or dynamic-import argument is unknown, regardless of a lexical
  literal assignment: its guarded body remains in executable closure and its
  dynamic import stays under source/AST/catalog custody. Tests interpret graph
  inclusion as an executable possibility and exclusion only as proven deadness,
  never as a normal-path prediction.
- Dynamic relative-import discovery retains lexical candidates separately from
  semantic import edges. Statement imports defer classified unknown
  package/spec/name anchors and the no-parent/beyond-top errors under exact
  custody; their other resolution errors still fail closed. Call-resolution
  errors use the exact custody rule above. Foreign
  globals or explicit package arguments never acquire lexical fallback
  semantics. Source requests carry the runtime-anchor requirement into live
  completion, precomputed scans, and graph merges. Strict persisted requests
  never carry runtime custody. Custody reaches
  a fixed point over every discovered owner, preserving admitted module names
  (including aliases), original resolution roots, fresh AST identity, and
  full-depth scans. This source custody is target-independent: native/WASM
  registry rows and source-backend dispatch retain the same finite roots.
  Source admission is not runtime capability admission. Backends still reject
  operations whose generated semantic requirements they cannot implement,
  before source/artifact publication; catalog custody does not grant Rust/Luau
  a relative-import transaction, object model, or exception protocol.
  `IMPORT_PROTOCOL` in `op_kinds.toml` owns import opcode spellings, execution
  intrinsics and first-class callable acquisition requirements. Object/cache
  support does not imply this capability. Rust/Luau/MLIR do not advertise it;
  their retained import operations fail target admission, not a late unknown-
  module error or a synthetic `sys` dictionary. Native/WASM/LLVM use the shared
  runtime protocol. Source-backend admission currently examines all retained IR
  functions; source-catalog closure alone is not a successful-build receipt.
- Build-time resolution and build-time admission are separate. Explicit
  external roots (`MOLT_MODULE_ROOTS`, `--lib-path`, respected `PYTHONPATH`, and
  auto site-packages) make modules resolvable, but only direct entry imports
  are admitted by default. Transitive closure for an external package requires
  an explicit `MOLT_EXTERNAL_STATIC_PACKAGES` package admission, and the module
  graph cache key includes that policy. Package-parent `__init__` files needed
  for an admitted leaf cannot backdoor additional external children unless the
  package is explicitly admitted.
- Finder-returned extension specs use two independent admission facts: an
  extension artifact suffix (including archive members), or the private
  extension-loader declaration carried by the real loader class and its
  current MRO. Class names, writable attributes, facade aliases, and
  `sys.modules` membership are not loader identity. A declared loader needs
  an origin even when its artifact has a nonstandard suffix. File/archive
  custody and manifest validation remain mandatory at every finder/import
  boundary; a loader declaration grants no execution capability.
- Explicit external package admission is also native-artifact custody. Any
  package-local `.so`/`.pyd` artifact discovered under an admitted package must
  have a nearby `extension_manifest.json` sidecar whose module name, extension
  path, extension SHA-256, ABI tag, target triple, platform tag, and
  capabilities match the actual artifact. Build admission fails closed before
  frontend lowering when the sidecar is missing or invalid, and graph, wrapper,
  and backend object-cache inputs include the artifact and manifest custody
  facts. Extension sidecars may declare `python_exports` as dotted package
  import names (for example a package-level function reexport) satisfied by the
  native artifact, and may declare `callable_exports` for direct native
  bindings with module/name, binding kind (`module_attr` or `direct_symbol`),
  ABI, required symbol for `direct_symbol`, effects, and determinism metadata.
  The native-artifact planner treats those names as the same reachability
  authority as the extension module name, and scoped lowering cache inputs carry
  the validated callable export map, so source package closure and native
  object closure cannot disagree. A callable export is the only authority that
  can lower a native package function such as
  `scipy.ndimage.distance_transform_edt` to native `invoke_ffi` ABI metadata;
  native package visibility alone must leave the call bound/dynamic. For WASM
  builds, an admitted external package containing native source or
  host-extension markers must publish wasm32 `static_link` `libmolt_source`
  artifacts before the graph scanner expands that package; raw
  NumPy/SciPy-style source roots are not a linkable substitute. Native builds
  must then publish the validated artifact,
  sidecar, package `__init__.py` chain, and existing runtime extension shim
  candidates under a deterministic `external_static_packages/<plan-digest>/`
  runtime root; generated native binaries must prepend that staged root to
  canonical `MOLT_MODULE_ROOTS` before runtime startup, and target modes without
  a runtime-custody consumer must fail closed. Final link reuse hashes those
  staged bytes, but runtime-loaded extensions are not appended to the linker
  command unless the extension ABI explicitly requires link-time linkage.
- Core stdlib closure must use the same explicit nested-scan exception set as
  normal stdlib discovery. Disabling those exceptions for bootstrap/core
  closure can leave compiled stdlib function bodies with dangling direct module
  symbols, even when the entry program never calls the affected method.
- Backend-facing IR must not contain direct calls to module-owned symbols whose
  modules are outside the materialized module graph. The CLI validates this
  immediately after IR finalization and before codegen/shared-cache
  publication, reporting the first function/op coordinates so graph-closure
  drift is reproducible without a slow link or runtime failure. Lazy
  `MODULE_IMPORT` remains a runtime boundary for optional code paths and must
  raise deterministically when the module is absent. Split-runtime isolate
  import dispatch is bounded by the explicit import set, plus required parent
  packages for those imports; graph-only runtime support modules must not become
  ambient isolate-loadable roots.
- Shared stdlib cache identity must use the same stdlib module-init roots as
  backend dead-function elimination. Reuse is valid only when the key, CLI
  manifest, and backend-written partition manifest sidecar match; key+manifest
  sidecars without the partition manifest are stale. The shared cache key
  includes the sorted stdlib module-symbol partition, and
  `MOLT_STDLIB_MODULE_SYMBOLS` is the canonical serialized module-symbol
  authority for that partition; all backend consumers must parse it through one
  strict parser, and malformed values must fail closed rather than reverting to
  heuristic ownership. A reachable-empty stdlib partition still publishes a real
  parseable object plus count, key, manifest, and partition sidecars; absence of
  functions is cache content, not permission to skip cache emission.
- Build-time graph materialization has one immutable binary image closure plan.
  The resolved entry scope records whether the image came from a CLI script,
  CLI module/package, or configured project entry; the final image root set
  includes the entry plus explicit static import roots. The import plan
  classifies declared roots, entry-reachable modules, runtime support, stdlib
  support, package parents, namespace/generated modules, and external native
  artifacts.
  `known_modules` is the whole admitted runtime import-visibility closure;
  `direct_call_modules` is the Python `module__function` link authority; and
  `compile_modules` is the sole authority for modules lowered into the binary.
  Native artifacts and package parents may appear in `known_modules` so imports
  can resolve, but they must not leak fake Python direct-call symbols unless
  they are also present in `direct_call_modules`. Native executable entrypoints
  are governed by validated `callable_exports`, not by `known_modules`.
  `from package import child` records `child` as a module binding only when
  `package.child` is itself an exact admitted module; calls such as
  `child.native_export(...)` may then route through validated
  `callable_exports`, while ordinary from-imported attributes remain attribute
  bindings and cannot mint direct-call symbols.
  Dead-module elimination may narrow `compile_modules`, but it must not mutate
  the known closure, direct-call authority, runtime import dispatch roots, or
  wrapper-cache dependency graph. Wrapper build manifests and diagnostics must
  carry and fingerprint the same closure plan, including dead-module-elimination
  mode, rather than exposing a selector-only payload or rediscovering a parallel
  graph.
- A persisted strict source scan is one complete, content-validated record of
  imports and static loader/runpy execution roots. Imports-only consumers project
  that record; they must not publish an uncomputed execution projection as empty.
  Discovery reads and validates a record once, computes missing records once,
  and does not rewrite a validated hit. Runtime import custody and native-owned
  source closures retain their distinct admission rules and cannot borrow strict
  scan certainty from this cache.
  Compiler tooling identity belongs to the existing reentrant source-fingerprint
  transaction, including independently invoked graph, analysis, and cache
  operations. Snapshot lookup precedes tooling closure discovery and path
  resolution. The next operation recaptures bytes, import topology, and resolved
  ownership; application payloads retain byte validation within an operation.
  No process-wide path/stat cache may substitute for these checks.
- Build diagnostics carry a versioned `binary_image_analysis` envelope beside
  the closure plan. It bridges source/AST metrics, module schedule hashes,
  lowering policy, backend IR/TIR-input shape, and final artifact/link evidence
  without becoming a second semantic authority. Cache keys use stable closure
  and toolchain identities; volatile timing/allocation samples remain evidence
  that joins back to those identities, not cache-key inputs.
- The frontend `source_identity` projection is the SourceSite digest family:
  source hashes, span-derived AST site digests, binary-image module roles, and a
  semantic identity digest that IR, TIR, backend, allocation, and binary
  projections can join against without embedding raw source text or duplicating
  TIR facts. Backend IR diagnostics now carry the matching `source_sites`
  projection from the lowered op stream: attributed-op coverage, per-line
  operation counts, and a stable digest over `source_line`/column coordinates.
  `allocation_ownership` joins that same carrier to possible heap allocation roots,
  owned frame candidates, retain/release ops, heap-exposure ops, and
  finalizer-sensitive results, so memory-pressure diagnostics share the binary
  image identity without becoming another allocation authority. Those
  allocation/refcount categories are generated from `op_kinds.toml`; frontend
  `borrow`/`release` aliases canonicalize to `inc_ref`/`dec_ref` before the
  diagnostics consume them. This is evidence over the compiler carrier, not a
  second AST parser or CLI-local allocation table.
- `__import__` and `importlib.import_module` share the same Rust-owned runtime
  import transaction. Source-language imports call
  `molt_importlib_import_transaction` directly with explicit
  `name`/`globals`/`locals`/`fromlist`/`level` payloads; the public
  `importlib.import_module` shim calls the narrower
  `molt_importlib_import_module(name, package)` wrapper so CPython public API
  argument validation and relative-name resolution stay isolated from ordinary
  import payload lowering. That wrapper must delegate into the same transaction
  path after resolving the public API name; it must not become a second module
  cache or resolved-name import authority. `importlib.util.resolve_name`
  remains a public helper over the same private relative-name rules. Public
  argument validation stays API-specific even when helpers share resolver logic;
  CPython 3.12 gives
  `resolve_name(".x", None)` and `import_module(".x", None)` different error
  classes, and the covered validation matrix preserves that split for
  non-string names/packages, missing packages, empty names, and beyond-top-level
  relative imports. Current native differential evidence covers
  `import_module("math", 1)`, relative non-string package `TypeError`, relative
  `package=None` `TypeError`, package-relative success, importlib bootstrap
  submodule identity, and the public resolver-validation transcript through
  this path.
- `importlib.import_module` has no alias side table in the Python shim. The old
  empty `_MODULE_ALIASES` map was a dead second source of truth and must not be
  restored. Frontend literal and direct-call folds for
  `importlib.import_module("literal")` must call the public
  `molt_importlib_import_module(name, None)` wrapper rather than a private
  Python alias or a duplicated resolved-name shortcut. The frontend proves
  callable identity and literal absolute name only; runtime import success,
  missing-module errors,
  version-gated absence, cache custody, fromlist behavior, and module
  provenance remain owned by the Rust transaction. Rebinding through
  `importlib` or any module alias records a module-attribute mutation; while
  that attribute is unstable, both the transaction fold and cross-module static
  direct-call lowering must be refused so runtime dispatch observes the user
  replacement. Ordinary source-language imports carry explicit
  `name`/`fromlist`/`level` payloads into the same Rust transaction path;
  bootstrap and importlib implementation modules keep the private
  cycle-breaking `MODULE_IMPORT` boundary. Public importlib APIs do not bypass
  the transaction.
- Source `from ... import ...` child preparation is runtime-owned. The Rust
  transaction imports graph-proven child modules only when the parent package
  lacks the requested export, binds successful child modules onto the parent,
  preserves existing package exports, converts an absent requested child into
  the final `IMPORT_FROM` `ImportError`, and propagates dependency import errors
  without broad suppression.
- Actual stdlib bootstrap entry bodies and generated chunks retain their raw
  module-load boundary, then call `molt_importlib_prepare_fromlist` once with the
  complete ordered fromlist before any `IMPORT_FROM` or alias binding. This
  joins the same package-presence and child-preparation owner as the public
  transaction, following CPython's
  [`_handle_fromlist`](https://github.com/python/cpython/blob/v3.12.15/Lib/importlib/_bootstrap.py).
  Existing exports, including falsey values, win; a later child failure leaves
  earlier import side effects and prevents all statement bindings. The stage
  avoids recursive public `__import__` entry but can enter ordered child
  finders. User source and functions inside bootstrap modules retain the public
  transaction. Native/WASM execution qualification is pending; Luau support for
  the surrounding import operations remains a separate open capability.
- The shared import transaction evaluates `fromlist` truth once. A falsey
  fromlist selects the ordinary top-level return without inspecting package
  attributes. For a truthy fromlist, only observable `__path__` presence admits
  package child preparation; falsey values still count as present, dynamic
  lookup is honored, and non-`AttributeError` failures propagate. Ordinary
  modules return without iterating or validating the fromlist or `__all__`.
  Their `MODULE_IMPORT_STAR` operation owns indexed `__all__` reads, error
  precedence, and partial destination writes. Package preparation retains its
  distinct iterator protocol before that same indexed binding operation.
- Source `from package import *` with a statically proven package `__all__`
  extends the build-time import scan with resolvable child modules named by that
  `__all__`, records those imports in persisted import-scan/module-analysis
  cache payloads, and prepares the child modules through the same Rust
  transaction `fromlist=["*"]` path before `MODULE_IMPORT_STAR` performs the
  binding copy. Dynamic `__all__` values and unresolved child names remain
  runtime-visible: unresolved names are not added to the graph and the final
  star binding raises the normal CPython-shaped missing-attribute error.
- Relative `builtins.__import__` package-context calculation is transaction
  owned for the covered CPython 3.12 cases. Omitted `globals` retains the internal
  MISSING default and raises KeyError for absent `__name__` at relative levels;
  explicit None is supplied and raises TypeError. The text signature still shows
  `globals=None`, matching CPython's presentation of its NULL C default.
  The C-API entry admits NULL through its native-object resolver; that boundary
  remains separate from managed-handle transaction binding. Non-dict `globals` raises
  `TypeError`; non-`None` `__package__` must be a string; `__package__ is None`
  consults `__spec__.parent`, preserves missing-parent `AttributeError`, and
  validates parent type; the fallback requires string `__name__`, treats a
  present non-`None` `__path__` as package context, and otherwise uses the
  dotted-name parent. Empty package context raises the normal relative-import
  no-known-parent `ImportError`.
- `FileLoader` and `SourceFileLoader.load_module` delegate module materializing,
  `sys.modules` preinsert, rollback/pop on failed new loads, existing-module
  reload no-rollback behavior, loader execution, and successful
  `sys.modules` substitution return selection to the shared Rust
  spec-execution transaction. Python loader code may still normalize arguments
  and build specs, but it must not own the module-cache transaction.
- `SourceFileLoader`/`ZipSourceLoader.exec_module(module)` and compiler-admitted
  extension/sourceless shim execution run the compiler initializer directly in
  the supplied module object. The execution transaction redirects the
  initializer's `MODULE_NEW` to that object, preserves loader-created metadata,
  and leaves partial namespace mutations on failure; no dict-copy executor or
  dict-only compatibility ABI remains.
- Target/device-specific lazy imports, such as GPU backend families, must be
  represented as explicit runtime/device policy edges before they are admitted
  to the compiled graph. Non-admitted imports raise deterministic errors.

---

## 4. Caching And Reload
- Modules are cached in `sys.modules`.
- Reload behavior is explicit; `importlib.reload` is gated.
- Cache invalidation requires explicit tooling support.

## 5. Validation Anchors
Import/bootstrap changes are expected to be covered by the existing in-tree regression lanes documented in
[0008_MINIMUM_MUST_PASS_MATRIX.md](../../testing/0008_MINIMUM_MUST_PASS_MATRIX.md):

- Native bootstrap/package-entry regressions: `tests/test_native_import_bootstrap_regressions.py`
- WASM import bootstrap smoke and package-relative imports: `tests/test_wasm_importlib_smoke.py`, `tests/test_wasm_importlib_package_bootstrap.py`
- Binary image closure authority: `tests/cli/test_cli_binary_image_closure.py`
  covers configured entry-file/entry-module image scopes, CLI selector
  override, ambiguous configured selectors, import-plan closure payload
  classification, fail-closed compile modules outside the admitted closure, and
  diagnostics closure/analysis payloads, DME-aware wrapper-cache identity,
  backend IR/artifact analysis projections, and wrapper-cache static-import
  closure fingerprinting.
- Module graph authority guards: `tests/cli/test_cli_module_graph_authority.py`
  keeps wrapper build cache dependency fingerprints routed through
  `_prepare_entry_module_graph` instead of direct discovery/static-import
  rediscovery; `tests/cli/test_cli_build_inputs_authority.py` keeps entry
  selector and binary image kind resolution in the build-input authority.
- Differential import semantics: `tests/differential/stdlib/importlib_basic.py`, `tests/differential/stdlib/importlib_from_bootstrap_submodules.py`, `tests/differential/stdlib/importlib_import_module_basic.py`, `tests/differential/stdlib/importlib_import_module_helper_constant.py`, `tests/differential/stdlib/importlib_import_module_helper_dotted.py`, `tests/differential/stdlib/importlib_import_module_helper_submodule.py`, `tests/differential/stdlib/importlib_import_module_relative_package_typeerror.py`, `tests/differential/stdlib/importlib_relative_import_from_package.py`, `tests/differential/stdlib/importlib_runtime_state_payload_intrinsic.py`, `tests/differential/stdlib/importlib_support_bootstrap.py`
- Focused active transaction/fromlist slice: `tests/differential/stdlib/importlib_import_module_basic.py`, `tests/differential/stdlib/importlib_import_module_helper_constant.py`, `tests/differential/stdlib/importlib_import_module_helper_submodule.py`, `tests/differential/stdlib/importlib_dunder_import_fromlist.py`; run this slice with `tests/molt_diff.py --stdlib-profile full` because the importlib discovery path intentionally pulls full-profile `zipfile`/`csv`/compression support. This is a focused regression slice for transaction/cache changes, not a replacement for the full IB2 matrix when declaring import semantics matrix-green.
- `IMPORT_FROM` uses ordinary attribute lookup, then the exact public submodule cache value (including a cached `None`). Missing exports construct `ImportError` with owned `name`, `path`, and `name_from` through the shared exception field transaction. Python 3.12 uses the physical module `__file__`; 3.13+ consults location-bearing `__spec__.origin` before the module-file fallback. Missing specifications, namespace origins, initialization state and metadata callback failures retain their version-specific behavior. The existing `from_import_missing_name.py` differential receiver supplies authored metadata and callback/cache controls, alongside the OEM/MBCS receivers in the required native/WASM proof-plan row. Source implementation and enrollment are not target qualification. The Python 3.13+ startup-dependent shadowing diagnostic remains a v1 exit requirement: it needs an immutable startup `safe_path`/`sys_path_0` producer, not mutable `sys.path` inference.

- Static package `__all__` star-child slice: `tests/cli/test_cli_import_collection.py::test_from_import_star_graph_admits_static_all_child_module`, `tests/test_native_import_star_all_regressions.py`, and `tests/differential/basic/import_star_package_all_child.py`. Keep this paired with `tests/differential/basic/import_star.py` when changing `MODULE_IMPORT_STAR`, import-scan caches, or the Rust transaction `fromlist=["*"]` path.
- Package-context slice: `tests/test_native_import_package_context_regressions.py` and `tests/differential/basic/import_dunder_package_context.py`; the differential receipt is `logs/import_dunder_package_context_diff.log` plus `logs/import_dunder_package_context_diff_results.jsonl`. Keep this paired with transaction/fromlist tests when changing `importlib_transaction_package_from_globals` or relative `__import__` resolution.
- Public importlib resolver-validation slice: `tests/test_native_importlib_public_api_regressions.py` and `tests/differential/stdlib/importlib_public_api_validation.py`; the differential receipt is `logs/importlib_public_api_validation_diff.log` plus `logs/importlib_public_api_validation_diff_results.jsonl`. Keep this paired with transaction tests when changing `molt_importlib_resolve_name`, `molt_importlib_import_module_resolve_name`, or `importlib.import_module` shim wiring.
- Load-module spec-execution slice: `tests/test_native_importlib_load_module_transaction.py`, `tests/differential/stdlib/importlib_load_module_transaction.py`, and the existing spec/module differential shard (`importlib_module_from_spec.py`, `importlib_spec_from_file.py`, `importlib_util_spec_module.py`, `importlib_util_exec_module.py`, `importlib_sourcefileloader_compiled_exec.py`). The differential receipts are `logs/importlib_load_module_transaction_diff.log`, `logs/importlib_load_module_transaction_diff_results.jsonl`, `logs/importlib_spec_execution_transaction_regression_diff.log`, and `logs/importlib_spec_execution_transaction_regression_diff_results.jsonl`.

---

## 6. Build-Time Manifest
Build emits an import manifest:
- list of resolved modules,
- module origin and admitted source/extension custody,
- import scan mode and reason/profile impact for each admitted support edge,
- hash or version for each module.

This manifest is part of reproducible builds.

---

## 7. Errors
Resolver absence is an explicit import outcome, separate from a failed finder,
loader, initializer, or publication callback. Only actual absence permits the
next admitted resolution mechanism. Exception names and messages never select
another importer. Execution failures retain the original exception object,
including its class, arguments, cause, context, notes, and traceback.

Import errors must include:
- target module name,
- resolution path attempted,
- whether the failure is policy or missing-module.

---

## 8. Open Questions
- Complete dynamic/broader CPython `fromlist` star/`__all__` expansion and
  namespace-package edge cases inside the Rust transaction while keeping
  compile-time graph discovery separate.
- Remaining namespace-package edge-case policy.
- Editable installs and dev-mode behaviors.


### Selected value provenance

The shared expression-result graph transports canonical Python value identity
independently of normal-result shape. An unknown-shaped importer remains an
importer when selected, unpacked, iterated, or returned by a supported builtin
method. Joins retain alternatives; source facts, binding storage, publication,
and result selection use the same identity vocabulary. Value exposure of the
globals mapping travels with result graphs, including joins or publication
that erase contents. Expression facts project identity and value exposure
from that graph. Exposure from every retained display child is accumulated
before uncertain unpacking erases the concrete contents.

Namespace observation during evaluation is a separate source event. Capturing
`globals` or an exact bound globals mutation method can observe the namespace
without publishing the globals mapping as its result. The observation event
remains on expression and statement facts, including merged execution paths;
it is never stamped onto returned-value exposure. Storing a mapping or aggregate
transports exposure; that alone does not prove a metadata mutation. Passing the
value to foreign code can grant access to the caller's globals and must invalidate
closed import-state custody. Invalidation keeps this possible exposure while
removing exact value, shape, ownership, and lifetime proofs, including through
binding taint, deferred captures, and subsequent iteration. The same result graph
retains possible canonical identities of previously observed elements alongside
the unknown alternative. This is candidate provenance, never an exact value or
a dispatch/elision proof. Joining an unknown alternative cannot erase those
candidates. A definite replacement uses the new value's own provenance.
Rooted globals retirement is transported through immutable containers and
publication; it does not protect mutable contents or create allocation ownership.

Eager list/set comprehension results carry their element facts. Dictionary
comprehensions carry iterated key facts and separately retain value exposure.
Generator creation does not execute the body: its result graph carries only
conservative yielded exposure, with unknown identity/shape and callback-capable
iteration/release. It never captures creation-time body values as future yields.

Every comprehension generator uses the shared completion-loop fixpoint: a
rejected filter takes the current generator's backedge, and nested iterables
execute inside the enclosing generator's body. Element, condition, call and
iteration facts join all visits before truth or dependency pruning. Empty
iterables skip targets, filters and payloads; generator bodies do not publish
creation-time writes, exception observations or closure-history states.
Strict and source import-state projections consume the same completed iteration
and target effects in execution order, including iterator release.
Locationless comprehension wrappers use their target-to-final-operand span in
the canonical source-key authority. Nested and sibling clauses remain distinct
after reparsing, so an outer clause cannot change an inner clause's emptiness,
element, callback or release facts.

Storing a namespace-exposing value selects the same strict state projection
whether or not unrelated control flow is present. Storage itself leaves import
metadata unchanged. Escape, member stores/deletes and in-place operations on an
exposing value invalidate metadata through their canonical call/target effects;
frontend relative-import lowering must retain a runtime transaction after such
an operation instead of freezing the original package anchor.

Value provenance does not prove ownership, callback freedom, or release safety.
Borrowed method results and selected aliases do not create fresh allocation
custody. Mutable contents expire at the existing object-write, callback, and
operand-retirement boundaries while captured object identity remains intact.
The exact globals-dictionary gate still separately requires activation and
receiver-shape evidence; namespace provenance alone cannot admit dict methods.

Truth and eager-dependency consumers use the provider's completed subscript
result, never reconstruct it from the pre-index owner fact. Partial providers
and syntax-only fallbacks expire mutable owner contents when index evaluation
is not proven inert. The binding cache schema, persisted local graph schema,
and product import-scan schema invalidate pre-provenance results together.


Deferred execution provenance is part of the canonical expression-result graph.
Direct invocation/resumption is separate from a value contained in an aggregate:
creating a generator, capturing a globals alias in a function, or storing either
does not execute its body. Source-owned execution candidates survive publication,
joins, widening, selection and iteration without claiming an exact callable or
value shape. Completed lexical-scope mutation effects are propagated once through
a finite dependency worklist and sealed onto the existing expression, statement
and iteration facts. Strict lowering, graph discovery and runtime-custody scans
consume those facts; no second mutator-name or function-body syntax registry is
permitted. Starred positional and keyword expansions use the shared argument
schedule and evaluated argument provenance, including stored aggregate aliases.

Canonical exact scalar kinds govern ordinary and augmented operator callbacks
independently of compile-time values. Both use the same successful-result kind
transfer, so repeated arithmetic and loop-carried scalar joins retain callback
freedom. Type transfer never evaluates arithmetic speculatively and never removes
runtime errors, overflow, allocation or evaluation. Addition folding is bounded
before allocation to 4096 sequence elements or 4096 integer bits; exceeding the
budget retains the exact kind and forgets only the constant. The shared result
record carries a finite set of exact scalar alternatives when one kind is not
known, such as integer powers with unknown exponent sign or mixed numeric joins.
Unary and binary transfers preserve this proof across subsequent operations;
an unknown or subclass alternative erases it. Callback-driven binding replacement
still invalidates representation facts together with values.

Both operator forms execute operand escape references only at a boundary that
can invoke their protocols, including reachable contained values. Subclass,
reflected and in-place dispatch retain their callback capability. The actual
post-callback target storage governs retirement; namespace mutation is transported
separately. Scalar arithmetic preserves a closed relative-import anchor, while a
module-defined operator that changes metadata requires the same runtime
transaction as a directly invoked deferred metadata writer.

The operation's own effects select its deferred operands. Inherited child
effects do not turn inert storage into execution and are never replayed as a
second state transfer after the child completes. Member reads, subscriptions,
lambda defaults and suspension wrappers obey the same operation-local boundary
as calls and operators. A completed named assignment therefore remains clean
until a subsequent operation can actually replace it. Hashing, comparison,
membership, indexing, representation, mapping expansion, member stores/deletes,
pattern matching and reference retirement retain possible contained values
when their protocol can reach them. Explicit invocation and generator resumption
remain distinct from ordinary object protocol dispatch. Store and release
dependencies travel with the existing statement observations and are sealed by
the same lexical worklist as expression and iteration dependencies.

Conditional expressions join both possible callable results. Surviving source
callable candidates do not prove complete provenance after callback rebinding;
an incomplete module callable depends on the module's escaped summary. A
current-module value exposes its namespace just as a globals mapping does.
`setattr` can dismiss a metadata write only when its evaluated owner excludes
the current module, or its literal attribute excludes metadata. Aggregate
namespace exposure applies to function-body item stores and deletes too.

Class namespace publication, inherited bases and metaclass arguments retain
source execution candidates. Preparation, construction and descriptor creation
consume them at their respective callback boundaries. Ordinary stored function
descriptors carry an empty attribute-lookup hook set; reading a bound method
does not execute its body. Custom attribute hooks and descriptor values remain
separate lookup candidates. Unknown class preparation cannot seal constructor
or member provenance. Skipping deferred-body analysis leaves those references
unresolved and conservative, including references reachable through a class.

Yield classification belongs to the compiler-analysis lexical authority. The
dependency summary records it during the existing lexical walk and frontend
consumers import the canonical helpers directly. Binding facts schema 54, local
source graph schema 18 and product import scan schema 23 invalidate prior
cached projections of these provenance and dispatch rules.

Every completed metadata callback or escape summary uses one import-state
projection. Semantic admission forgets the anchor; source discovery retains
explicit candidates beside an unknown alternative. This applies to expression,
statement, iteration, target-completion and argument-expansion summaries.
Explicit opaque metadata assignment still replaces its former candidate.
Unknown truth callbacks cannot establish an exact scalar loop input, and an
unknown loop predicate cannot establish the first iteration's package anchor.
Source candidates and backedge observations never authorize static imports.
