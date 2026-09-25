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
gates. Persisted graph identity still includes the complete binding policy,
analysis schema, and parser version.

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
rescanned when a caller promotes it. Strict persisted graph receipts restore and
validate the same modes; their keys include root role and complete precomputed
input identity. An imports-only tuple cannot stand in for a complete source scan.

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
owners under it. Strict persisted source records never carry runtime custody.

Generated module sources use one content-addressed writer keyed only by canonical
module name and emitted text; the text never embeds its generated filename.
Importer and namespace identities remain explicit in the graph; synthetic/native
roots carry complete precomputed records that force their original Python names.
Native helper-root growth may transfer an admitted generated source only from an
exact prior slice receipt whose retained roots are a subset of the new selection.
Dependencies resolve through enclosing admitted source authority, and persisted
graph inputs include that authority. The shared import collector keys and forwards
the target Python version used to certify each complete scan.

Persisted graph package roles are checked against exact admitted or precomputed
source authority when present; generated filenames never redefine that role.
Native-artifact package support parsing and collection use the artifact admission
request's selected Python target.
