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

The persisted scan contains only source-pure requests: unexpanded imports,
star-package names, dynamic-relative facts, and unevaluated loader/runpy paths.
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

Precomputed records carry both imports and source-execution edges, with source
content, target, mode, and capability identity. Selected native helper slices are
scanned from their emitted generated source. Unsliced initialization scans remain
initialization scans. A previously admitted real source is not silently replaced
by a narrower helper slice.

Runtime policy is a projection of the current source authority, not an entry-name
classifier. Its operation-local memo contains booleans keyed by source content,
module/package identity, scan mode, and target; it retains no AST or source.
Preparation and native materialization use the same runtime closure finalizer.
New roots, promoted modes, or native imports participate in closure convergence
before the final policy, finite source-backed catalog, and generated importer are
published.

Runtime custody remains a separate permission boundary: only verified owner
sources may use it, every owner scan must be full-depth, and native-only artifacts
never acquire source custody. A changed catalog creates new custody and rescans
owners under it. Owners never read or publish strict records; non-owner sources
may reuse pure requests and complete them live. Strict records never carry
runtime custody.

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
