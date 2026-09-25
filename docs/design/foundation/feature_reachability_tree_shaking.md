# Reachability-Driven Runtime-Feature Elimination

Status: static-retention requirements and automatic tier selection implemented;
cross-target elimination and performance claims require execution receipts.
Owner: compiler import, function-retention, and runtime-feature authorities.

## 1. Contract

Reachability here means **static link reachability**, not observed execution.
Registry relocations retain module initializers. Publishing a function with
`func_new` retains its body, including intrinsic references in lazy branches.
An uncalled function can therefore impose a real link requirement.

Optimization must remove the actual dependency or prove the owning body dead.
Dropping registry roots, ignoring function-address references, masking a feature,
or weakening conservative reentrant binding analysis is not an optimization.
Moving an import inside a retained function alone does not establish removal.

For fixed prepared SimpleIR and explicit roots:

1. Retain the first entry, protected runtime entries, and registry relocation roots.
2. Follow defined-function references to a fixed point.
3. Collect known runtime intrinsic symbols from retained `builtin_func` and
   `const_str` operations.
4. Map those symbols to link-affecting features and compare against the target's
   available profile features.

This is a conservative requirement analysis. A matching string can retain an
intrinsic even when runtime data flow never uses it. Conversely, a computed name
or runtime-generated dependency needs its own sound admission evidence; scanning
literal names does not prove arbitrary dynamic completeness.

## 2. Canonical authorities

| Fact | Authority and consumers |
| --- | --- |
| Defined-function reference-bearing SimpleIR kinds | `runtime/molt-ir/src/tir/op_kinds.toml`, generated for Python and Rust by `tools/gen_op_kinds.py` |
| Python function closure and cache reference validation | `src/molt/cli/function_references.py` |
| Backend dead-function elimination | `runtime/molt-tir/src/passes/dead_functions.rs` |
| Protected runtime entrypoints | `runtime/molt-tir/src/passes/runtime_roots.rs`, with Python agreement tests |
| Intrinsic requirements and target diagnostics | `src/molt/cli/required_features.py`, consumed by `backend_ir.py` |
| Intrinsic identity and public loader registration | `runtime/molt-runtime/src/intrinsics/manifest.pyi` and generated registries |
| Symbol-to-feature attribution | `runtime/molt-runtime/src/intrinsics/categories.toml`, Cargo/cfg facts, and generated `src/molt/_runtime_feature_gates.py` |
| Profile names and default | `src/molt/cli/config_resolution.py` |
| Profile feature closure and tier selection | Cargo feature graph through `src/molt/cli/runtime_features.py` |
| Backend resolver candidate manifest | `runtime/molt-tir/src/passes/app_callable_manifest.rs`, validated against runtime symbols |

Defined-function retention is distinct from the narrower first-class-function
carrier used to propagate runtime requirement masks. Do not merge these facts
just because both involve `s_value`.

Task creation retains its exact table-addressable target. `alloc_task` and
`call_async` do not synthesize a `_poll` companion name. A poll body is retained
when its exact name is referenced or it is an explicit root. Retired creation
pseudo-operations must not survive as a separate Python retention policy.

The Python requirement pass and backend resolver manifest operate at different
pipeline stages. Their intended agreement must be proved; naming both
"reachability" does not make them identical algorithms or prove binary contents.

## 3. Profiles and archive selection

`auto` selects the lowest runtime ladder tier containing the required
link-affecting features, using the Cargo-derived target ceiling. An explicit
tier requests that concrete artifact tier; it is not silently downgraded.
Unavailable target features produce actionable refusal rather than fabricated
parity. Shared artifacts retain their toolchain, target, profile, and content
identity.

An imported `re` module may publish regex functions and retain `stdlib_regex`
even without a regex call. Such a program is not promised to fit `micro`.
A program whose actual module/body closure excludes regex should not acquire
that feature merely from a host compiler dependency.

`module_required_intrinsic_names` remains useful for source-policy and wiring
audits. It must not become a second build link-requirement authority.

## 4. Runtime-library boundary

CPython facades and `moltlib` runtime sources must not import the host compiler
package. The shared AST checker in `tools/check_stdlib_intrinsics.py` walks
absolute imports, including lazy and type-checking branches, for both roots.
The normalized `molt.stdlib.*` spelling is permitted; `_intrinsics` owns the
runtime loader. This lexical gate is not a proof of computed dynamic imports.

Molt-specific file streaming belongs to `moltlib.io`, not CPython `io`.
It consumes public `io.open/read/close` and their existing capability authority,
without networking dependencies. The full lifecycle contract is in
[Streaming and WebSockets](../../spec/areas/web/0600_STREAMING_AND_WEBSOCKETS.md).

The same rule applies to networking and concurrency: runtime-library imports
resolve their loader directly, not through `molt.intrinsics`. Remaining legacy
host-package exports are tracked by `config/legacy_inventory.toml`; retiring
them requires migrating frontend identities and proving compiled consumers.

## 5. Verification and measurement

Use the smallest proof that falsifies the changed invariant, then exercise the
actual target before making an execution or artifact claim.

- Generated-fact tests reject malformed/duplicate rows and prove the Python and
  Rust consumers use the same table. Reference tests cover explicit roots,
  function publication, async targets, and absence of invented poll siblings.
- Source-closure tests scan real `io` and `_io` under supported target-version
  policies. Intrinsic audits cover manifest registration without counting
  `moltlib` as CPython stdlib coverage.
- Capture actual module admission and final prepared IR for a minimal program,
  an explicit heavy-feature control, and representative ecosystem workloads.
  Source indentation or mocked IR alone does not establish a smaller closure.
- Run native and linked WASM against the CPython oracle, retaining exact output,
  exit status, source identity, target/version/profile/capabilities, and cleanup
  receipts. Permission-granted tests do not establish permission-denied behavior.
- For elimination claims, inspect resolver manifests and final artifact symbols.
  A smaller IR or archive does not prove an intrinsic is absent from the binary.

Measure cold and warm frontend stages separately: import discovery, binding
analysis, AST digesting, lowering, serialization, backend work, and linking.
Record retained modules/functions/symbols, cache hits/misses, wall time, peak
process-tree memory, build footprint, binary size, and startup. Keep profiler
overhead separate from ordinary build latency; do not compare warm cache hits
against a freshly invalidated lowering cache as a speedup result.

Use the existing [benchmark authority](../../BENCHMARKING.md), proof plan,
and target matrix. Keep per-run profiles and receipts outside tracked source.
Artifact-size, memory, buildability, and steady-state throughput are distinct
dimensions; improvements in one do not imply improvements in the others.

## 6. Remaining optimization and soundness work

These are research/implementation directions, not completed guarantees:

- Validate the full path from requirement analysis through selected archive,
  per-app resolver, link retention, and runtime dynamic loading. Classify and
  minimize disagreements into replayable cases.
- Prove computed intrinsic names and hidden runtime dependencies through the
  existing admission authorities. Add generated dependency metadata only when
  an actual unrepresented edge requires it; never create a second handwritten
  fallback list or silently discard an unknown edge.
- Remove demonstrated gratuitous dependency classes. Prior candidate families
  include metadata/email/archive imports, warning/unittest regex helpers,
  gettext parsing, typing extensions, glob helpers, logging identifier checks,
  and large-integer decimal conversion. Reinspect current consumers before
  changing any of them; real parsers and tokenizers retain their dependencies.
- Prove within-feature elimination with positive/negative intrinsic controls.
  Investigate section/COMDAT retention, runtime call edges, resolver references,
  archive granularity, LTO, and linker behavior before choosing a structural
  split. Absence of a direct call alone is not sufficient.
- Consolidate stale profile-name projections in harnesses and tooling with
  `config_resolution.py`; changing to a larger profile must not hide an
  unexpected small-program dependency.
- Close applicable Python-version, OS, architecture, backend, and runtime-profile
  cells. Identical source-level requirements do not imply identical artifacts or
  execution semantics across targets.

## References

- [GraalVM reachability metadata](https://www.graalvm.org/latest/reference-manual/native-image/metadata/)
- [Codon whole-program compilation](https://www.exaloop.io/blog/mapping-python-to-llvm)
- [Nuitka implicit-import metadata](https://github.com/Nuitka/Nuitka/blob/develop/nuitka/plugins/standard/ImplicitImports.py)
- [Linker garbage collection](https://maskray.me/blog/2021-02-28-linker-garbage-collection)
- [Rust binary-size techniques](https://github.com/johnthagen/min-sized-rust)
- [JavaScript tree shaking and side effects](https://webpack.js.org/guides/tree-shaking/)
