# Reachability-Driven Runtime-Feature Elimination

Status: static-retention requirements and automatic tier selection implemented;
declared intrinsic availability (one `# cfg:` per executable, compile-time proven,
no feature-off stand-ins) implemented (HF-487); cross-target elimination and
performance claims require execution receipts.
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
3. Collect known runtime intrinsic symbols from retained named external/runtime
   calls and first-class function addresses, using the generated op-kind target
   roles. Compiled definitions take precedence over same-spelling runtime
   symbols; extern declarations do not. Runtime-only `builtin_func` references
   and conservative `const_str` resolver candidates remain requirements
   independently of compiled definitions.
4. Map those symbols to their declared features and compare against the target's
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
| Symbol availability (feature and target) | Each entry's `# cfg:` line in `runtime/molt-runtime/src/intrinsics/manifest.pyi`; a non-intrinsic runtime export declares `cfg` on its import entry in `runtime/molt-backend-wasm/src/wasm_abi_manifest.toml`. Projected by `tools/gen_intrinsics.py` into `src/molt/_runtime_feature_gates.py`, the resolver arms and their absence probes, and by `tools/gen_wasm_abi.py` into the WASM import tables (section 4) |
| Resolver bucketing (which generated resolver module owns a symbol) | `runtime/molt-runtime/src/intrinsics/categories.toml`; it states no availability |
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

A build computes the reached intrinsic symbols once, in `_prepare_backend_ir`;
the profile and target refusal and the required link features both read that
symbol set.

The Python requirement pass and backend resolver manifest operate at different
pipeline stages. Their intended agreement must be proved; naming both
"reachability" does not make them identical algorithms or prove binary contents.

## 3. Profiles and archive selection

`auto` selects the lowest runtime ladder tier containing the required
features, using the Cargo-derived target ceiling. An explicit
tier requests that concrete artifact tier; it is not silently downgraded.
Unavailable target features produce actionable refusal rather than fabricated
parity. Shared artifacts retain their toolchain, target, profile, and content
identity.

An imported `re` module may publish regex functions and retain `stdlib_regex`
even without a regex call. Such a program is not promised to fit `micro`.
A program whose actual module/body closure excludes regex should not acquire
that feature merely from a host compiler dependency.

The optional public unicodedata.name and lookup exports require
stdlib_unicode_names on native and WASM. Their reached intrinsic identities
select that feature through the same generated attribution and profile ceiling:
auto currently selects full; an explicit smaller tier refuses the requirement.
Property-only unicodedata operations require stdlib_text, not the names API.

General codec kernels accept runtime-selected encodings and error policies, so
Unicode named escapes and namereplace require the existing names table even
when the public names API is absent. The runtime therefore has a mandatory
unicode_names2 dependency; it never substitutes hexadecimal escapes or unknown
name errors because a build omitted the table. Normal section reachability owns
unused data removal. This adds table-crate compilation and may retain name data
through generic method roots. Artifact size and build cost must be measured for
minimal/fixed-UTF8 and name workloads; no zero-cost or optimal-retention claim is
made. Target-version name fidelity remains the separate Unicode-data obligation.

`module_required_intrinsic_names` remains useful for source-policy and wiring
audits. It must not become a second build link-requirement authority.

## 4. Declared availability (HF-487)

Every runtime executable has exactly one availability fact: the Rust `cfg` under
which it is defined. An intrinsic declares it in its manifest entry, spelled as
the generated attribute spells it, with at most one Cargo feature:

```
# cfg: feature = "stdlib_net", not(target_os = "windows")
def molt_socketpair(family: Any, type: Any, proto: Any) -> Any: ...
```

An alias (`# runtime-symbol:`) inherits its executable's cfg. Every consumer
projects this one fact: the generated Python gate table (exact symbols, no name
prefixes), the resolver arms, the WASM import tables, the WASM runtime export
set, the target-availability refusal and `auto` tier selection.

**Proof.** A declaration is only useful if it equals the definition's cfg. The
generated resolvers prove both directions at compile time. A resolver arm names
`crate::<symbol>` under the declared cfg, so a definition gated more narrowly
fails to compile. Outside the declared cfg, a generated probe names the symbol
under two glob imports, `crate::*` and a `declared_absent` sentinel module. If a
definition still exists (a hand stub, or a definition gated more widely than
declared), the name is ambiguous and rustc rejects the build (E0659). Symbols of
leaf crates need no probe: the leaf crate is an optional dependency, so dropping
the feature drops the crate. `tests/test_runtime_feature_gates.py` covers the
shape the probe cannot name: any runtime export compiled only when a feature, or
the networking provider it selects, is off.

**No stand-ins.** A runtime built without a feature defines none of its symbols.
A program that reaches one selects the feature under `auto`, and an explicit
profile without it refuses at build time, naming the reached symbols, the
feature and the smallest tier that provides it. Nothing raises "requires the
X feature" or "networking not available" at run time. The 82 exported stand-ins
this replaced (`micro_stubs.rs`, the compile/codeop stubs, `net_stubs.rs`, and
the no-net ssl variants) and the six resolver-only features (empty Cargo groups
that hid defined symbols from the resolver) are gone: `stdlib_select` and
`stdlib_dbm` now gate their modules; logging, signal, concurrent and the import
system's resources/metadata intrinsics are core and always defined.

**Core builtins.** A Python builtin is reached through a global name lookup,
which reachability does not map to a feature, so `tools/gen_wasm_abi.py`
rejects a gated builtin callable. `compile()` is core: the parser is linked into
every runtime for traceback ranges, and `zipimport` retains it, so gating it
moved those programs to the full tier. Only the `ast` module needs `stdlib_ast`.

**Networking providers.** `stdlib_net` requests networking; `build.rs` names the
provider: `molt_has_net_io` (the native socket ABI on unix), `molt_wasm_net`
(the WASM host-call ABI) and `molt_net` (either). Windows has no provider until
the WinSock contracts land, so networking symbols declare
`not(target_os = "windows")` and a Windows build that reaches them refuses. The
I/O-driver futures (sockets, streams, fd watchers, accept loops) and the I/O
poller exist only with a provider; WASM host-progress retries for processes and
streams live outside the poller and need none.

**WASM poll table.** A WASM app fills fixed poll-table slots with the runtime's
poll functions, and a runtime without `stdlib_net` lacks 17 of them (the I/O
wait, the WebSocket wait and the 15 asyncio socket, stream, watcher and accept
polls). The codegen layout therefore carries one more fact from the bound
runtime: `MOLT_WASM_RUNTIME_POLL_SLOTS`, a bit mask of the slots whose poll
functions that runtime defines. `wasm_codegen_layout.py` derives it from the
pair's generated export obligations
(`RuntimeWasmGeneration.shared_runtime_import_names`), the same declared cfg the
runtime's admission checks. The backend imports only the defined poll functions
and fills every other slot with the sentinel; the slot numbering and the table
layout stay fixed. No future can name an absent slot, because only the absent
feature creates one.

Precedent: Rust `cfg` removes an item from the crate, so a reference fails to
compile and a missing symbol fails to link
([cfg](https://doc.rust-lang.org/reference/conditional-compilation.html),
[features](https://doc.rust-lang.org/cargo/reference/features.html)); Go build
constraints exclude whole files, with the same compile-time failure
([build constraints](https://pkg.go.dev/cmd/go#hdr-Build_constraints)); CPython
built without an optional extension (`_ssl`, `_sqlite3`) has no such module, and
importing it raises `ImportError` rather than linking a stub that fails later
([Modules/Setup](https://github.com/python/cpython/blob/main/Modules/Setup));
Bazel's `target_compatible_with` refuses an incompatible target at analysis time
([platforms](https://bazel.build/extending/platforms#skipping-incompatible-targets)).

## 5. Runtime-library boundary

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

## 6. Verification and measurement

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

## 7. Remaining optimization and soundness work

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
