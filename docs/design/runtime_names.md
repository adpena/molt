# Typed runtime names (HF-653)

The runtime-known name vocabulary is declared in
`runtime/molt-ir/src/tir/runtime_names.toml`. `tools/gen_runtime_names.py`
generates the closed `NameId` enum, its spellings, and isolate-owned name slots.
The generator is registered in `tools/generator_manifest.toml`; regenerate with
`tools/generators.py write --only tools/gen_runtime_names.py`.

Consumers select `NameId` values. The closed enum's generated discriminant indexes
one canonical atomic slot, without a second spelling-based field dispatcher. The spelling
constructs its string object; a generated decoder admits bytes only at a genuinely
dynamic input boundary. A slot owns
one reference; lookups borrow it, and runtime teardown clears
all generated slots through the existing atomic-cache release authority. Failure
to allocate a name remains raw zero and must propagate as allocation failure,
never an attribute miss. There is no byte-keyed static-slot dispatcher.

The census excludes its own generated name declarations, comments, string literals, inline
test modules, and test-only source files. Every registered identity needs a
production consumer. It rejects the retired byte-keyed static interfaces.
The Rust compiler, rather than a text comparison, rejects a removed identity.

Migration is by complete consumer family. Static and interned-name consumers
share `RuntimeNames`; `InternedNames` and both byte-keyed interning helpers are
retired. Function binding reads its existing typed metadata fields directly,
preserving published-code signature precedence. Native method tables,
native callable declarations and class-method publication also use `NameId`.
Generated builtin and intrinsic declarations carry checked registry identities.
Public callable identity remains declared once in `runtime/molt-runtime/src/intrinsics/public_objects.toml`;
`tools/gen_intrinsics.py` projects its name, qualified name and module through `NameId`.
Builtin and intrinsic declarations retain only `PublicObjectId`, so neither
repeats a public spelling or carries a parallel name ID. The name generator
follows intrinsic generation so its consumer census sees the current projection.
Public metadata materialization acquires the typed slots directly.
C-extension callable display names remain unbounded external values, represented
explicitly by `NativeCallableName::External`; known declarations use `Known(NameId)`.
Their lookup boundary decodes a dynamic name once; generated method dispatch
compares the typed identity. Fixed attribute and special-method callers,
including pickle, use `NameId` through their helper parameters and finite member
tables. The old special-method byte wrapper is deleted.
User-defined names remain dynamic: a closed runtime vocabulary cannot replace
Python's user-defined attribute space.

`dynamic_attr_name_bits` is the byte-input boundary for user-defined attributes,
external callable display names, qualified values, and signature text. Fixed
runtime names use `NameId::owned_bits`; the census rejects identifier literals
passed back through the dynamic helper and the migrated protocol interfaces.
Extracted runtime crates retain their actual byte-input ABI symbols and fields.
The satellite `crate::bridge` byte-input API is also preserved: reduced profiles
compile shared satellite sources against that real input boundary.

Known-name objects are allocated lazily in their generated slots. Runtime
startup does not materialize the unused vocabulary. At the dynamic boundary,
the first observation decodes a name; canonical objects then enter a sorted
reverse projection and later observations compare pointers. A reverse entry
borrows only the exact object held by its generated slot. Noncanonical strings
remain dynamic inputs and never enter this borrowed projection. Classification
never replaces the original name passed to a Python attribute hook or dictionary
comparison. Teardown withdraws the reverse projection before releasing its name
owners.

The existing class mutation authority is `class_bump_layout_version`, which
also advances `global_type_version`; releasing a type advances the same epoch.
An MRO cache must use that authority to invalidate facts inherited from a base,
and must preserve the uncached callback and dictionary-error semantics. Cached
references cannot outlive their owners or match a recycled address.

The cache uses 256 isolate-owned slots keyed by class identity,
class layout version, global type version and name-object identity. Each entry
owns its exact-string key to prevent address reuse; its result is borrowed from
the declaring namespace. Owning the result would delay descriptor finalization
after deletion, which would change Python behavior. The existing GIL token
protects these borrowed observations. Class-reference detachment and namespace
clearing already advance the mutation authority before releasing displaced
values. Cache teardown belongs to the same runtime-state drain as name slots.

A completed raw MRO lookup can publish a hit or a miss when its original
versions remain unchanged, including a successful dictionary-equality callback.
[CPython 3.12.15's `_PyType_Lookup`](https://github.com/python/cpython/blob/v3.12.15/Objects/typeobject.c#L4416-L4464)
also caches successful callback-dependent misses until type invalidation;
mutating a comparison key alone does not invalidate that observation.
The same entry records a typed purity fact. Optional pure clients reuse only
callback-free observations certified by the namespace observer. After the real
resolver has queried native declarations, this observer may certify native
absence; unresolved observers still decline unpublished native namespaces.
Pure clients pass existing name objects, with `NameId` values at fixed sites.
Lazy callable publication advances the existing class version.
A suppressed dictionary-probe
error must not publish a cached miss. Independently rooted synthetic function
descriptors are also uncached because their roots can retire independently of
class mutation. Descriptor binding and user hooks remain outside this raw cache: a cache
hit must still invoke them. Cache observations cannot retain a transaction's
selected descriptor across a callback; the existing descriptor snapshot owns
that separate lifetime obligation. The old persistent descriptor cache is
deleted; only the active operation snapshot retains classes and values.

Ordinary C attribute assignment on managed types publishes through the existing
`TypeMutation` boundary. Raw generic writes that physically alias a heap class
namespace advance the same runtime class version before retiring values, without
publishing native slots or notifying C type watchers. Static classes keep their
separate physical instance dictionary. Native-only types retain their existing C cache.
Direct dictionary mutation of `tp_dict` is not a supported invalidation oracle:
the [CPython 3.12 type-object contract](https://github.com/python/cpython/blob/v3.12.15/Doc/c-api/typeobj.rst)
documents dictionary C-API writes to that field as unsafe.
After a manual namespace edit, the caller must invoke
[`PyType_Modified`](https://docs.python.org/3.12/c-api/type.html#c.PyType_Modified).
`PyDict_SetItem`/`PyDict_DelItem` do not notify type owners, and repeated readiness
does not replace that notification. For a managed type, `PyType_Modified` advances
`class_bump_layout_version` before checking native version tags or invoking
watchers. Its global epoch invalidates inherited runtime hits and misses even
when no C type tag was ever admitted. Native-only types keep their native
invalidation traversal. Readiness and enumeration never republish a deleted
declaration from a completed native namespace.

Prior art: [family A](findings_prior_art.md#a-typed-op-and-runtime-call-abi).
CPython 3.12's [`MCACHE_HASH_METHOD` and `_PyType_Lookup`](https://github.com/python/cpython/blob/v3.12.0/Objects/typeobject.c)
key the type cache by a type version and name pointer. Molt reuses its own class
publication and invalidation authorities instead of adding an independent epoch.

## Assumptions and measured composition

The research verdict against the original 50% band is `DeferredPendingResearch`.
The band is not a claim that names alone must account for the remaining work.
The retained release comparison uses the original `974f410a7e` executable and
the `28d7c68594` code tree, macOS arm64, CPython frontend 3.12.15, seed 0 and five
balanced pairs per successful primitive. Later retirement of the inlining probe
restores that exact code tree; documentation changes do not change its producers.

| Assumption | Classification and evidence |
| --- | --- |
| 256 direct-mapped slots | Cargo-culted as a performance choice: bounded storage is justified, but neither this capacity nor its collision rule has a measured optimum. |
| Class, layout, global epoch and original name identity form the key | Hard-earned for mutation/lifetime/name identity in the executed regressions. The separate layout component is conservative; its necessity in addition to the global epoch remains cargo-culted until ablated. |
| Every class mutation advances the global epoch | Hard-earned for correctness through class/base writes, raw generic namespace aliases and type retirement. Invalidating unrelated classes is conservative; its churn cost is unmeasured. |
| Completed native misses are cacheable | Hard-earned: the initial configuration repeatedly rediscovered native absence and regressed dispatch to about 21.5K instructions/op. Admitting completed misses reduced it to about 11.1K. Pure unresolved observers still cannot certify unpublished declarations. |
| Bypass unfinished MROs, dictionary failures and separately rooted synthetic descriptors | Hard-earned by construction/exception/retirement controls. Exact-string admission preserves name lifetime and original query identity. The 100-byte cutoff is an untuned bounded-memory policy. |
| Embed the table in the runtime state | Unwound: the selected implementation owns one boxed bounded table. The placement probe reduced the measured recursion cost relative to the inline table. One Rust allocation per runtime is the cold cost; field-offset causality is not independently established. |
| Inline the canonical cached-handle helper | Measured configuration retired: the hint did not improve the repeated primitive matrix. Publication rules and the helper remain unchanged. |

The corrected native-miss admission is not evidence that the residual 11.1K is
all global binding or runtime entry. Three retained profiles total 818 dispatch
body samples and 450 isinstance body samples. Exclusive counts for name
resolution, namespace probes, cache probes, call binding, runtime entry/state
and reference handling are respectively 14/5/64/96/121/47 and 3/29/11/0/52/24.
Descriptor selection, type relations, globals and other guest/runtime work occupy
the remainder. These are CPU-time samples, not instruction counts: a disjoint
instruction partition is still unknown, and no constant-CPI conversion is used.
Separate existing runtime counters observe 1,000,183 call dispatches and
1,000,075 attribute lookups for dispatch, versus 183 and 1,000,075 for isinstance.
Startup/setup contributes to these process totals.

A source-bound local-binding probe supplies measured counts-times-cost effects:
one million omitted dispatch-global reads save approximately 809M instructions;
prebinding the method then saves approximately 3.840G more, including changed
call-shape effects. These deltas are not independent category budgets. Capturing
`type` as a local instead increases the prebound variant by about 7.311G;
capturing `isinstance` and `str` increases that loop by about 4.119G. Alias capture
can lose existing specialization, so cheap global binding is not uniformly
additive. A guarded method target could remove a path that currently benefits
from typed names; dynamic and invalidated paths would still need the name/cache
authority. Cheaper runtime entry can benefit remaining calls, but the quantitative
composition with either guard is not yet measured.

Reactivation requires explicit receipt conditions: `CapacitySensitivityProof`
over 64/256/1024 slots and realistic mutation/collision churn;
`InstructionAttributionProof` with measured path counts and costs rather than
time-to-instruction inference; `GuardedMethodCompositionProof` retaining receiver,
callable, class-version, descriptor and callback-order guards on native and WASM;
and `RepeatedNoRegressionProof` over every valid primitive and minimal startup,
with matching source/tool/environment/binary identities and reported spread.
The historical baseline `reduce_ex` workload failed and has no qualified
instruction cost. HF-646 subsequently added default object reduction; that
consumer migration does not establish a new performance comparison.

The accepted landing verdict is a named, measured trade: dispatch instructions
fall 18%, new-and-update 35%, and minimal startup 1.3%; runtime initialization
falls from 3,524 to 3,282 microseconds and the binary shrinks 16 KB. Recursion
increases 0.106% with non-overlapping ranges and memo-get increases 0.14%.
The integration owner accepted these costs on 2026-11-01; the research verdict
against the 50% band remains `DeferredPendingResearch` with the conditions above.

HF-646 copy and reduction consumers use this same authority, including copyreg
`_reduce_ex`, `__newobj__` and `__newobj_ex__`. Reducer method tables and
attribute/special-method helpers take `NameId`; no internal byte-keyed wrapper
remains. Module import paths and encoding names retain their separate semantics.
