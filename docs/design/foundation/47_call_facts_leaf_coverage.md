# CallFacts: demanded call-site facts

`runtime/molt-passes/src/tir/call_facts.rs` owns the per-function CallFacts table.
It records result-bearing sites from the shared `FunctionCallSites` projection,
including Python callbacks and proven runtime primitives. Each record is keyed
by the call's first result `ValueId`; `BTreeMap<u32, CallFacts>` gives deterministic
iteration. The table is read-only analysis, not part of executable op layout.

## Precise and local producers

`CallFactsTable::build_for_function(func, module, call_graph, summaries, tti)`
builds one selected function's table from explicit current module context. The
compiler FactGraph exporter invokes it after module transformations, using a
fresh final call graph and summaries. It excludes extern declarations from the
local module, so an external or unresolved call remains opaque. Recursive SCCs
remain visible to leaf and inline-eligibility classification.

Ordinary native, WASM, LLVM, and Luau module compilation does not construct these
diagnostic tables. The initial graph and summaries consumed by inlining and
fusion remain in the transformation owner. Native leaf metadata is separately
computed after native lifetime/splitting passes, from the actual emitted bodies.

`CallFactsTable::build_local(func)` supplies the sound intraprocedural floor.
`CallFactsAnalysis` caches that floor under `AnalysisId::CallFacts` and invalidates
it on CFG or op changes. The local API cannot see callee bodies or membership;
it makes no callee-side proof. Precise facts are requested directly, without
seeding every function's analysis manager or retaining an all-function table.

## Fields and semantic authority

| Field | Authority and conservative result |
| --- | --- |
| `target` | Shared call-target provenance plus module membership; `StaticDirect { callee }` only for proven local direct calls, otherwise `Opaque` |
| `typed_return` | Result semantic type and default representation; unknown types remain `None` |
| `leaf` | Callee call graph; `Proven` for no calls, `False` for a calling callee, `Unknown` for an unresolved callee |
| `no_throw` | Generated operation effect contract; handler absence or builtin spelling cannot prove no-throw |
| `inlinable` | The inliner's existing eligibility classifier, including typed refusal reasons |
| `no_alloc`, `no_escape_args` | Remain conservative until a sound producer and consumer are implemented |

`FactValue` distinguishes `Proven`, `False`, `Unknown`, `Guarded`, and `Profiled`.
Unknown is pessimistic. The local floor must never out-claim precise facts.
Fact construction reuses the existing classifier; the inliner does not read this
diagnostic table. No backend optimization may infer a proof from its presence.

## FactGraph export

`molt factgraph <entry> <function> --output <path>` emits schema-version-3 JSON
through the backend's normal optimized module path. Only the selected function's
precise CallFacts table is requested. Exported call facts identify their producer
as `CallFactsTable::build_for_function`. Missing function selection fails with the
sorted list of available local functions before final diagnostic analyses run.
`FactGraph::build_local` remains an explicit local-view API, not a fallback for
compiler emission.

`tools/fact_graph_dump.py` validates and renders this artifact. Facts retain their
stable ordering, event identifiers, confidence, and source sites; the producer
string identifies provenance rather than changing fact semantics. CallFacts unit
tests cover the conservative floor and precise classifiers. Export tests include
recursive SCC calls, extern calls, unresolved calls, and source-file custody.

## Future consumers

Escape-derived allocation/noescape proofs, guarded devirtualization, RC elision,
and backend call specialization require their own sound producers, consumers,
invalidation rules, and target evidence. Build such facts only where a consumer
needs them. Coverage tools measure representation; coverage alone does not prove
runtime speed, correct specialization, or backend/profile release acceptance.
