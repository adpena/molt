# Source scan authority

Lexical binding fixpoints are shared by source identity and a typed flow policy:
target Python version, target platform, canonical-import provenance, and deferred
body analysis. Module name, spec name, package role, and execution kind belong to
the subsequent import-context projection, not the fixpoint. A static scan asks
for core facts directly; import discovery and lowering project their own context
over the same immutable fact tuples and lookup maps.

One single-flight FIFO cache implementation governs both levels. Up to 128 core
entries each own up to eight projections; core eviction releases its projection
cache. Neither retains ASTs or analyzer state. Projection retries retain completed
core facts, and concurrent failure waiters receive independent exception objects.
Import-flow projection retains all assignment-effect and metadata-mutation
requirements; context sharing never relaxes version, platform, or provenance
gates. Persisted compiler/tooling closure identity includes the complete binding policy,
analysis schema, and parser version.

Completed deferred histories retain their raw-storage extent. Allocating exposed
slots in a later activation cannot change an earlier history that never stored
those slots, so it does not replay that history's transitions or result suffixes.
Growth of the retained history or exposure within its conservative storage bound
still refreshes the summary. Binding queries start from the canonical projection
at their actual history position, then join future events; an earlier exposure
remains live even when it recorded no write. There is no separate cache of these
domain-dependent binding projections. The frontier tests compare every suffix
against public state projections, including absent slots, late exposure and
nonancestor/decreasing-epoch histories. This is compiler analysis only.

`compiler_analysis/python_source_keys.py` owns source-text digests, typed AST
digests, source-span keys, and exact-tree digest admission. AST identity streams
prefix-free v3 framing into SHA-256, with separate sorted member digests only for
unordered constants. Class module/qualified name, field and attribute schemas,
absent versus present slots, exact scalar types/bits, and source spans all remain
identity-bearing. Explicit traversal frames reject active-path cycles while
allowing shared subtrees by value; deeply nested ordered and unordered values
do not consume the Python call stack. Buffering is bounded, and ordered container
width does not require retaining all child hashes.

One admission retains the exact tree and its captured digest only for a read-only
scan generation. It rejects a substituted tree; it is not a mutation detector or
a process-wide AST cache. A new operation after mutation must create a new
admission. Binding analysis, import scans, frontend lowering, and profiling use
this same identity authority; internal digest versions are not a stable public
serialization contract.

The binding profiler measures cold analysis and same-source context batches
separately. Fact telemetry describes the computation that created shared facts;
it is not work performed again on a cache hit. The process-lifetime fixpoint
counter measures actual starts (including failed attempts). Historical source
roots without that counter report unavailable, not zero.

A closure result carries its graph, explicit imports, and immutable source scan
authority. Each module identity binds a resolved source path, package execution
identity, and effective scan mode. Entry/static/spawn roots request full scans;
profile/package initialization seeds use the shared mode selector, including its
named static helpers. Diagnostic reasons and compile membership do not grant scan
depth.

Graph merges reject conflicting source paths or package identity and preserve the
strongest completed mode. A source already present in the graph must still be
rescanned when a caller promotes it. Every operation reconstructs the graph using
live candidate resolution; no persisted whole-graph receipt can hide a newly
created module or a higher-priority shadow. An imports-only tuple cannot stand
in for a complete source scan.

The graph's existing queue also converges namespace premises. A module publishes
its syntax-only writer summary once, retaining source execution owners. Immediate
writes join at initialization; deferred writes join when that existing body
context is reached. Full inventories retain all owners. The shared namespace index
wakes affected module scans and reached deferred contexts when that premise
grows. A context already scanned under the same premise is not repeated.
Package-child bindings use the same resolver authority as tooling closures.
Uninspected native/artifact/catalog owners retain unknown writes. The final
per-source authority carries the actual premise into runtime-import policy;
compiler discovery cannot hand that consumer a narrower or invented proof.
This is compiler/development analysis and adds no work to the emitted binary.

Native current-globals dictionary selection uses the binding authority's current
module slots. A literal-key read does not inherit access to the whole dictionary;
actual stored self-references retain that access. Native mutation methods share
the existing namespace-store transfer. Unknown writes invalidate actual module
bindings, including undeclared global/builtin reads, independently of the future
callback domain. The binding state and history joins retain unmodeled namespace
contents so a later read cannot lose aliases or custom-key callbacks. Unknown
owners, escaping namespaces and unproved activation mappings remain conservative.
Exact dictionary ownership does not remove the evaluated key's hashing and
comparison demands; syntax stores, deletes and augmented assignments retain
those demands at their shared publication boundary.
Source writers publish their actual value origins; storage uncertainty alone
does not expose every function in a verified module. Starred call arguments and
class bases use their evaluated operand facts through that same publication
boundary.

`compiler_analysis/python_import_scan.py` is the sole source-import producer.
It projects `PythonBindingIndex` origins and the shared lexical execution regions
into typed import sites, requests, source keys, value
bindings, execution demands, export facts, and unevaluated loader/runpy paths.
The CLI scanner owns topology completion and runtime import custody; policy and
core-lane admission consume these facts instead of resolving imports again.

`compiler_analysis/python_source_demands.py` owns the existing value-candidate
and activation worklist used by compiler discovery.
Aliases and imported members resolve through the same namespaces. Availability-only
`expose` requests retain source closure without claiming execution. Calling a
generator/coroutine factory creates a suspended value; resumption executes its
body. Call results retain evaluated callee relations and direct versus contained
return/yield origins. Subscription, unpacking and iteration targets project
elements rather than treating the container as its selected value.
One typed function-result record keeps final return origins separate from yielded
origins. Element records declare subscription, synchronous iteration or
asynchronous iteration explicitly; consumers do not infer the protocol from a
method spelling or execute every possible protocol.
Class values, instances and suspended results remain distinct: constructing an
instance does not execute every method, and retaining a callable does not call it.
Descriptor dispatch selects attribute hooks and descriptor-valued members without
activating unrelated iterator methods. Its canonical class demand receives later
namespace-origin deltas through the same worklist. Unknown or mixed callback
effects retain generic protocol activation rather than claiming descriptor-only
dispatch.
Inherited protocol members use the same namespace dependencies as ordinary lookup.
Evaluated class-base expressions have their own typed source relation. Namespace
exposure describes escape reachability; a nested or member class is not an
inherited owner merely because it appears in that exposure.
Iteration does not call functions stored in a container.

Iterator acquisition and advancement are separate typed demands for synchronous
and asynchronous protocols. Acquisition invokes `__iter__` or `__aiter__`;
advancement follows the returned carrier, with asynchronous next results awaited.
A generator factory is not its returned generator object. Source discovery must
not execute an uncalled factory or advance an iterator just because a generator
expression captures it. These distinctions follow
[CPython comprehension generation](https://github.com/python/cpython/blob/3bb231a6a5dc02b95658877318bf61501a7209e9/Python/compile.c#L5334-L5443)
and [PEP 525](https://peps.python.org/pep-0525/#support-for-asynchronous-iteration-protocol).

Await and delegation expressions consume completion values; forwarded yields
remain separate. A delegated element has a typed projected source identity so it
cannot alias the same expression's completion. Await drives the iterator returned
by `__await__` directly, as in
[CPython's awaitable admission](https://github.com/python/cpython/blob/3bb231a6a5dc02b95658877318bf61501a7209e9/Objects/genobject.c#L951-L1004).
Async context methods are called before awaiting their result; an ordinary
function returning an awaitable is valid under
[PEP 492](https://peps.python.org/pep-0492/#asynchronous-context-managers-and-async-with).
Arbitrary custom iterator exception payloads are not exact completion facts;
their actual opaque escapes retain conservative source availability.

Generator-expression kind is a syntactic property of its own code scope, as in
[CPython's symbol table](https://github.com/python/cpython/blob/3bb231a6a5dc02b95658877318bf61501a7209e9/Python/symtable.c#L2417-L2485).
An await in a dead branch of that scope still marks it asynchronous; an await
only in the outermost iterable belongs to the enclosing scope. Reuse cached
lexical facts rather than deriving the code kind from optimized effects.

Execution regions retain evaluation order and the sync/async context entry,
exit and unpack phases needed for import discovery. They do not store exception
handler provenance. The binding authority's `PythonCompletionFlow` independently
owns real completion/reachability semantics; removing a source lint does not
remove that analysis. Context protocol behavior follows
[PEP 343](https://peps.python.org/pep-0343/), with suspension and close governed by
[PEP 342](https://peps.python.org/pep-0342/).

Candidate/demand activation joins process newly added candidates or demands;
a dependency wake does not replay the full historical Cartesian product. A
candidate/demand relation may itself be a product, and namespace resolution has
its own work; this is not a global linear-complexity claim.
An unadmitted module retains its waiting values until admission. Subsequent
deferred-body receipts do not wake all previously admitted module values:
changed facts wake their registered users. Source-value summaries are published
by the module scan rather than repeated with each deferred body.

The persisted scan contains only source-pure requests: unexpanded imports,
star-package names, dynamic-relative facts, source values/demands,
unevaluated loader/runpy paths, and an owner's catalog deferral. Intrinsic classification metadata references the
same source-request cache record; it does not persist another import-site copy.
The decoder validates every nested collection's declared list/row shape before
accepting it. An empty object or string cannot erase sites, namespace members,
base origins, attribute origins, or call/return relations. Invalid source records
regenerate from captured source; no completed custody verdict is cached.
Package `__all__` facts distinguish absent, known, unknown and incomplete source.
A module attribute hook prevents a proof of absent `__all__`; it may supply the
attribute at import time. A read or parse failure is not an empty export set. Statement and literal dynamic
star imports use the same completion, and existing declared package children
remain dependencies. Unknown/incomplete exports remain explicit obligations.

This follows [PEP 562](https://peps.python.org/pep-0562/): a module attribute
hook may supply a missing attribute, while direct global lookup bypasses the
hook. The worklist uses monotone joins, as described by
[MLIR data-flow analysis](https://mlir.llvm.org/docs/Tutorials/DataFlowAnalysis/);
unknown information cannot be treated as an empty or proven-absent value.
Source summaries establish discovery obligations, not target
execution qualification. Unknown calls that can reach mutable module globals
still conservatively expose namespace members; tighter effects and cold/warm
consumer costs remain part of the owning convergence work.
One completion step resolves current package exports, children, loader targets,
directory entry points, ordered roots, and stdlib policy. Path operations remain
ordered requests, so resolving a symlink before joining a parent segment is
replayed correctly. Resolver memos include roots and allowlist context.

One byte snapshot owns decoding (including encoding cookies and BOM), source
hash, and parse input. Publication uses that captured hash, never a later file
generation, and rejects changed content. Supplied source or AST remains
operation-local rather than reading or publishing strict disk records. Function
analysis caches retain defaults and kinds only; every consumer admits imports
through the same source-request loader.

The frontend carries that generation in its existing source catalog. A path
lease stores the captured raw-byte SHA and extent; an inline lease stores the
explicit text's UTF-8 SHA and extent, with a distinct source kind. Analysis gives
the import loader the same captured snapshot and checks the graph's SHA before
publishing its lease. Serial lowering parses lease text into its own mutable
AST; retained diagnostic trees never bypass this read. Worker envelopes decode
one closed lease shape and return the consumed SHA. Integration and persistence
check that receipt against the submitted lease before accepting IR.

Lowering context v4 includes the captured SHA, extent and source kind. Local
cache slots identify module/package/target; shared slots additionally identify
the whole context. Locating either slot performs no source read or stat. Schema
6 retires older lowering records whose publication could hash a later file than
the lowered source. A qualified hit checks live bytes against the lease; a miss
does not hash just to choose a slot. Writers stamp only the original lease and
reject changed or missing inputs before publication. Optional cache IO cannot
suppress a lost source generation.

One operation-local import topology starts at entry/root selection and remains
with the graph resolver and frontend catalog. Positive and negative lookups,
exact-case names, selected namespace portions and package-child inventories
retain their original observations. CLI and tooling share the segmented resolver:
a regular parent package or module shadows later namespace portions, following
[CPython's PathFinder](https://github.com/python/cpython/blob/v3.15.0/Lib/importlib/_bootstrap_external.py).
Relative selectors are anchored without collapsing `..` before symlink traversal.
Directory tokens avoid unnecessary repeated enumeration; changed, racy and
Windows tokens recheck the observed semantic queries. Unrelated output names do
not invalidate a single-name lookup. A failed generation cannot be refreshed in
place; it requires a new operation.

The full catalog fences source and topology after frontend work and at the
existing publication boundaries for emitted IR, native objects/links, WASM,
transpiled output, MLIR and factgraphs. Requested artifacts remain candidates
until that artifact's admission; a refusal preserves the pending artifact's
prior output and emits no success. Earlier separately admitted outputs can
remain published. Diagnostic metadata sidecars retain their independent contract;
this is not a transaction or rollback across every output of a build.
These are observed-input fences, not proof of an unobserved change-and-restore.
The content/input distinction follows [LLVM content addressing](https://llvm.org/docs/ContentAddressableStorage.html)
and [rustc incremental queries](https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation-in-detail.html).
All of this work belongs to the compiler and development tools. Emitted guest
binaries contain no lease, topology scan or publication validation.

Wrapper run/deploy commands resolve a fresh entry snapshot and target parse for
each operation. A resolved entry is not a process-lifetime source cache: changing
the file does not refresh its retained AST, and reusing that admission after
mutation must fail the strict precomputed-scan content check.
Even a matching precomputed premise retains a captured byte generation through
the operation's final content fence. Explicit supplied source or AST takes
precedence over a precomputed disk projection. Native helper root growth retains
the original source bytes and spans; a byte-generation change requires a new
operation. An earlier selected-view receipt cannot erase a widened premise or
narrow an admitted raw source.

Wrapper cache publication retains the complete pre-build input payload and key.
Its entry fences resolve the original selector with the child's explicit
environment and working directory through the shared entry/root resolver.
File selectors are anchored to the caller's directory before child execution;
symlinks remain live selectors. A new package shadow or a retargeted selector
cannot keep the old resolved entry as publication authority. The pre-build fence
also rejects selector drift before a cache-hit output can be consumed.
After the build child returns, the canonical closure/input builder is used again
only as a final equality fence. A changed entry, dependency, resolution topology,
target policy, or tooling identity cannot relabel the child's output with a later
generation. Classified source-content drift, a missing final input, or a different
final key makes the wrapper fail explicitly before returning a usable output
contract. The child's output may remain on disk, but run/deploy cannot consume it
through that failed operation. No new cache receipt is published, and an existing
receipt is left byte-for-byte unchanged; its input and binary hashes still govern
any later read. A subsequent operation resolves a new entry and must rebuild.
Unrelated policy/programming exceptions are not recast as source drift.
Receipt schema v3 excludes receipts made before this fence.
A binary-hash or receipt-publication I/O failure also returns an explicit wrapper
error with the output/receipt paths and underlying reason. The produced file stays
on disk, but no usable contract or success signal is returned. The wrapper's
path-only return contract does not carry optional-cache warnings to JSON callers.

The child success contract supplies output paths, not an attestation of compiled
source inputs. The wrapper fence prevents publication under a later observed
generation; it does not certify an unobserved change-and-restore during the child.
It applies to operations for which wrapper cache inputs can be captured; explicit
no-cache/rebuild operations retain their existing child-build contract.

Tooling dependency discovery returns one immutable receipt: ordered canonical
paths, per-file hashes, a root-relative content digest, and captured byte count.
Python and dynamic-import manifest identities use the exact bytes parsed by
discovery. Frontend fingerprints, native benchmark provenance, and WASM linker
identities consume this receipt instead of reopening those sources. Non-Python
frontend assets remain fingerprint inputs. Captured semantic inputs use their
byte identity directly without a redundant Git query; uncaptured backend/runtime
trees retain the clean-pathspec shortcut. Receipts retain neither source bytes
nor ASTs and may be reused only within the explicit immutable tooling operation.

Path admission belongs to discovery: missing candidates stop at the filesystem
probe; existing candidates retain their kind and resolve symlinks before search
root containment is checked. Downstream projections do not repeat that
admission. Persisted request-cache keys are only lookup hints, never path
authority, and unchanged request payloads are not republished. Each new
operation still resolves missing members, namespaces, and package shadows live.
Git clean-scope results and WASM tool digests cannot outlive that operation:
process-wide caching would otherwise conceal edits from later builds.

Precomputed records retain source requests with source content, target, mode,
and capability identity. Every reuse recompletes package exports and filesystem
edges in the current operation; a valid entry digest is not authority for stale
package topology. Native-artifact-owned exclusions keep their explicit source
execution roots without reinterpreting their implementation as Python source.
Selected native helpers are semantic views of the original captured AST, using
the same pruning authority as lowering. Root selection belongs to the source-scan
authority and participates in request/syntax/runtime-policy cache identity; raw
initialization remains raw. Growing views preserve original source keys and reuse
one parse rather than generating replacement Python files.

One explicit `_ModuleGraphDiscovery` continues through entry, package/core, native
support, runtime owner and generated namespace/importer admission. Each source
retains its own roots and admission policy. Writer deltas revisit affected prior
projections; logical receiver incidences survive until a later generated package
is admitted. Closure publication applies only changed source rows. Frozen graphs
cannot substitute for the live operation's writer and value provenance.

Runtime policy is a projection of the current source authority, not an entry-name
classifier. Every import statement a graph module executes uses the runtime
import protocol (it can reach `builtins.__import__`), so the core-closure
bootstrap sources that every program contains, such as `_io`, bind runtime
intrinsics and import nothing else; one import there gives `print(1)` the importlib
machinery. `tests/cli/test_runtime_import_scan_custody.py` (the print-only
micro program) and `tests/test_io_private_module_surface.py` check it. Its operation-local memo contains booleans keyed by source content,
module/package identity, scan mode, and target; it retains no AST or source.
Preparation and native materialization use the same runtime closure finalizer.
New roots, promoted modes, or native imports participate in closure convergence
before the final policy, finite source-backed catalog, and generated importer are
published.

Runtime custody remains a separate permission boundary: only verified owner
sources may use it, every owner scan must be full-depth, and native-only artifacts
never acquire source custody. An owner's scan is source-pure like any other: at a
dynamic import the runtime may resolve to any catalog row, it records a typed
`RuntimeCatalogDeferral` (for its imports, star modules, or one binding's bases)
and the digest of the AST it scanned, never the catalog itself.
`_RuntimeImportScanCustody.complete_owner_scan` admits only the custody's own
owner AST and appends the live catalog, so one owner scan serves every custody
generation of a build, and it persists under an owner-mode key beside the strict
record. Strict records carry no deferral and never carry runtime custody; an
owner never reads or publishes a strict record.

Generated module sources use one content-addressed writer keyed only by canonical
module name and emitted text; the text never embeds its generated filename.
Importer and namespace identities remain explicit in the graph; synthetic/native
roots carry complete precomputed records that force their original Python names.
Native helper-root growth may transfer an admitted generated source only from an
exact prior slice receipt whose retained roots are a subset of the new selection.
Dependencies resolve through enclosing admitted source authority. The shared
import collector keys and forwards
the target Python version used to certify each complete scan.

Package roles come from exact admitted or precomputed source authority when
present; generated filenames never redefine that role.
Native-artifact package support parsing and collection use the artifact admission
request's selected Python target.

## What source inventory establishes

Source inventory describes candidates, explicit uncertainty and ownership. It
cannot infer that every candidate import must succeed, or decide whether
catching an exception violates an API contract. Retained IR, generated
intrinsic target/profile availability and the linker establish required runtime
symbols. The admitted import plan and finite source-backed runtime catalog
establish module custody. Neither exception syntax nor a capability grant
establishes either fact.

Compiled Python algorithms are supported implementation owners. CPython's pinned
[heapq](https://github.com/python/cpython/blob/3bb231a6a5dc02b95658877318bf61501a7209e9/Lib/heapq.py#L581-L597),
[bisect](https://github.com/python/cpython/blob/3bb231a6a5dc02b95658877318bf61501a7209e9/Lib/bisect.py#L102-L108)
and [JSON decoder](https://github.com/python/cpython/blob/3bb231a6a5dc02b95658877318bf61501a7209e9/Lib/json/decoder.py#L5-L8)
select optional accelerators with Python exception handling. Compiling the
surviving algorithm is compatible with Molt's
[execution boundary](../spec/areas/compat/contracts/cpython_bridge_policy.md).
An intentionally absent closed-world module may raise the proper Python
exception; admitted compiled code may catch it. Unknown import custody remains
explicit and requires the existing admission boundary. Source status, candidate
closure and a full-coverage manifest do not qualify target behavior.

The audit therefore has no guarded-import or JSON pass-handler veto, and source
requests carry no handler/effect rows or call-owner fields used only by those
vetoes. Cache schema changes reject replaced records; current-schema records
also reject retired fields. Intrinsic-read classification, missing/unread
requirements, union coverage, collisions, ratchets and full-coverage manifest
checks retain their existing owners. Do not build a contextual heap interpreter
or a provider-obligation registry solely to reproduce the deleted lint.

## Activation semantics and qualification

Function objects and code objects have separate identities. The runtime's
`CodeCallableIdentity` in
[`object/layout.rs`](../../runtime/molt-runtime/src/object/layout.rs) describes
executable entry and physical call ABI. Construction in
[`function_abi.rs`](../../runtime/molt-runtime/src/builtins/functions/function_abi.rs)
validates code, globals and closure before publication; frames retain globals
and separately captured builtins. Code replacement in
[`attributes/mutation.rs`](../../runtime/molt-runtime/src/builtins/attributes/mutation.rs)
preserves the function's activation namespace and compatible closure.

CPython's pinned
[`PyFunction_NewWithQualName`, `func_new_impl`, `func_set_code`](https://github.com/python/cpython/blob/3bb231a6a5dc02b95658877318bf61501a7209e9/Objects/funcobject.c)
and
[`LOAD_GLOBAL`](https://github.com/python/cpython/blob/3bb231a6a5dc02b95658877318bf61501a7209e9/Python/bytecodes.c#L1220-L1268)
are the reference. Globals remain live mappings. A function captures the
builtins mapping object: replacing `globals['__builtins__']` does not retarget
it, while mutation of the captured mapping remains observable. Dictionary
subclasses and closure-cell order require their actual mapping/positional
semantics; module publication unions are not point-sensitive heap facts.

Eight reconstructed-function cases remain native/WASM qualification obligations:
explicit globals, dictionary-subclass lookup, helper-returned callable,
same code under two globals, nested def, nested lambda, compatible code
replacement retaining globals and a nonempty closure, and captured builtins.
Their existing CPython observations, propagating counterparts and code-read-only
controls are retained in `tests/test_check_stdlib_intrinsics.py`. Source inventory
success does not establish their runtime behavior. Existing runtime differentials
`tests/differential/basic/globals_callable.py` and
`tests/differential/basic/closure_cell_protocol.py` own the overlapping execution
families; qualify exact source/backend/profile cells before closing the finding.

JSON exception behavior belongs to public API differential tests, including
malformed data and user hooks that raise. Exact exception identity, callback
order, one invocation and absence of retry/silent success are observable
obligations; a `pass` statement or `_MOLT_*` spelling is not their oracle.

All source scanning, cache validation and these policy tools run on the
compiler/development side. They add no exception-provenance table or analysis
work to emitted programs. Measure compiler latency and retained memory when
changing this analysis, and qualify guest performance separately through
[`tools/PERF_AUTHORITY.md`](../../tools/PERF_AUTHORITY.md).
