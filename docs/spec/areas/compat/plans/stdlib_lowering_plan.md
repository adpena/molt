# Stdlib Lowering Plan (Intrinsic-First, CPython 3.12+)

**Status:** Active
**Owner:** stdlib + runtime + tooling + frontend + tests
**Scope:** CPython 3.12/3.13/3.14 stdlib union, top-level + submodule coverage, intrinsic-first lowering for native and wasm.

## Mission
Drive Molt to a strict stdlib posture where:
1. every required stdlib module/submodule is canonically present,
2. every required behavior is implemented: intrinsic-backed where the runtime must own it, or compiled from Python source (`python-compiled`, spec 0016), with explicit fail-fast for unsupported dynamism,
3. CI enforces no silent fallback paths and no coverage regressions,
4. native + wasm behavior moves in lockstep for covered semantics.

## Closed-world stdlib (architecture)
Molt compiles the stdlib with the user program as one closed world. This section owns how; the
gates below enforce coverage.

The proposed [target policy and versioned source migration](stdlib_source/design.md)
lands Python 3.15 admission/default first, then vendors 3.15 source with asyncio
first. It awaits integrator Goldfish review; this is a design, not qualification.

Owner decisions, 2026-11-16: "We primarily care about supporting the latest minor
version" for the first release; supporting more minors "will involve a lot of
minutiae" and comes later. Python 3.15 is the default and only Tier 1 Python;
3.12-3.14 remain buildable and tested Tier 2, never release-blocking. "Some of
the asyncio and related tests have caused out of memory and crashed my computer
in the past, and we don't want that to happen." The linked design records the
binding two-job, 6/24/48 GiB limits, timeouts and isolated spinning-loop procedure.

1. **One source of semantics.** A module CPython ships in Python compiles from CPython's pinned
   `.py` source (version-keyed selection; vendor only 3.15 for v1), patched only where the verified subset
   forbids a construct; each patch is minimal, provenance-pinned and rebased per release. A module
   CPython ships in C (`_asyncio`, `_json`, `_struct`, `array`, `_sre`) is a Rust intrinsic with
   thin Python glue matching CPython's surface. A Molt-authored Python rewrite of a Python module
   is a defect: a second semantics that drifts (2026-11-09 audit: 142 rewrites and 261 stubs;
   `asyncio/__init__.py` is 2,153 Molt lines where CPython ships 47).
2. **Part of the program.** Whole-program analysis spans user code and the stdlib: dead code
   is removed at function, class and attribute granularity, stdlib functions are specialized to
   their call sites' types, and inlining crosses the boundary.
3. **Summaries, not reanalysis.** Each module gets a content-addressed interprocedural summary
   (effects, return types, written globals and attributes, init purity). Whole-program analysis
   reads summaries, so a no-change rebuild stays at the Cargo, Go and uv bar.
4. **Build-time initialization.** A pure module body runs at compile time; its objects (constant
   tables, enum members, compiled regexes, codec maps) become immortal read-only data, the image
   heap (GraalVM Native Image's build-time class initialization). Runtime module init disappears.
   The whole-program literal pool is the first step.
5. **Target facts are constants.** `sys.platform`, `os.name`, `sys.version_info` and capability
   flags are fixed per target plan; their branches fold, and code for other targets never compiles.
6. **Static binding.** An intrinsic is bound by typed id, never by name. A simple operation (len
   of an exact list, isinstance on an exact type) is a late-TIR expansion every backend inlines; a
   runtime call is kept for real work.
7. **Two tiers, one source.** Development builds link prebuilt, content-addressed generic stdlib
   objects; release builds specialize the whole program. Both read the same summaries.
8. **Imports resolve at compile time.** A module object exists at run time only where the program
   reflects on it; module namespaces are static cells.

### Discovery and summaries (HF-938; owner approval 2026-11-20)

**Measured problem** (2026-11-20, macOS arm64, fresh cache, `import os, sys, json, re,
collections`): cold module-graph discovery takes 23.1 s and warm takes 3.3 s for 41 modules.
- 100 whole-module binding fixpoints cost 15.9 s (69%).
- 26 modules run their fixpoint 2-8 times, about 9 s (40%) in all. Each rerun follows a premise
  revision (`opaque_activation_sources`, `foreign_namespace_writes`) as discovery finds more
  writers. Then a replay runs again under the settled premises (HF-926).
- The early cutoff (`ScanPremiseReads.unchanged_under`) reruns the fixpoint to verify, so it saves
  only import collection.
- A scan record's key digests the whole frontend. Any compiler edit voids every stdlib record.
  The scan producer's static import closure is 477 files: the eager `molt.frontend` package
  import, and 219 CLI modules.

**Cause.** Discovery runs the precise proof to answer a reachability question, then repeats it
per premise revision.

**Design.** Each stage lands on its own, with its measurement.

1. *Stdlib summaries are a toolchain artifact.* The stdlib is fixed per toolchain and target. The
   release build computes each stdlib module's scan summary once and ships it, content-addressed
   by source digest, target plan and analysis schema. A user's cold build then analyzes only user
   modules. This is item 3 above applied to discovery, and GraalVM Native Image's build-time
   initialization applied to analysis.
   Source request records address the captured bytes, parser, target, package role and premise.
   Relative source requests stay symbolic in the record. Graph completion resolves them against
   the current package and filesystem, so byte-identical source can reuse requests after relocation.
   Release staging now builds the namespace syntax portion for each supported Python target from
   the sealed stdlib sources. The installed compiler manifest binds the artifact's bytes. Each
   row is selected by source digest, target, and package; its canonical AST digest and producer
   identity must match before it can supply the syntax-only namespace summary. The proof still
   refines those writes against binding facts for the importing program.
   The same archive carries source requests for stdlib initializers under a closed premise
   that allows package child bindings. It uses the same key and codec as the writable scan
   cache, including the exact parser, target plan and namespace premise. The importing program
   completes admitted requests against its own topology. A revised premise or a demanded body
   can still require binding analysis.
2. *Reachability is separate from proof.* Discovery's edges come from a syntactic import scan
   (milliseconds per module, parallel, cached by source digest). Only dynamic import sites (the
   import callables, `__import__`, `importlib`) consult the binding facts. The precise fixpoint
   runs once per module, under its settled premise, where lowering needs it. This is the shape of
   the Go, Zig and rustc module graphs.
   Entry-driven demand can identify the callable and argument facts an analysis needs before visiting
   its transitive library bodies. The transfer still needs Molt's existing custody for writes and
   unresolved calls. [JARVIS, §III](https://arxiv.org/abs/2305.05949)
   A module scan now captures syntax-only namespace writes before binding analysis. The binding
   core reads the package-independent own names from this capture, and discovery refines the
   package-relative foreign writes against the binding results. Both use the same admitted AST.
3. *Parametric summaries.* A module's scan summary is a function of its premise inputs: the
   foreign writes and the exposed functions it reads, as logged by `ScanPremiseReads`. The global
   fixpoint iterates over these small summaries and never reruns a module fixpoint. This is
   assume-guarantee composition, as in IFDS/IDE (Reps, Horwitz, Sagiv 1995) and Datalog solvers.
   The binding core records whether any member of each queried activation group is opaque.
   A different activation set can reuse that core when every consumed answer holds and the
   source and all other flow inputs match. For example, an enclosing function and its closure
   can supply the same proof of a foreign activation namespace. Its import projections use
   that core's facts, the module context and the
   discovery mode, so equivalent flow inputs also share strict and discovery projections. A
   missing activation set keeps its own cache key. Within one discovery operation, a revision
   can resume from a deferred-job boundary if the source and other flow inputs match and every
   opaque-activation answer read by that prefix still agrees. The checkpoint shares immutable
   state, copies the scope and history owners before resuming, and retains a bounded amount of
   recent work. A changed foreign-write set or a changed answer before any saved boundary
   starts the analysis from the source. The whole-graph transfer-summary solver remains open.
   Transfer outputs include namespace writes and source demands because callers can observe either
   before a body returns. On an input revision, a stable output can preserve dependent results.
   From-scratch graph and scan equality is the acceptance check for demand-driven execution.
   [Demanded Abstract Interpretation, §§4–6](https://arxiv.org/abs/2104.01270),
   [Interactive Abstract Interpretation, §3](https://arxiv.org/abs/2209.10445)
4. *Iteration order.* Premise propagation visits writers before the modules they write, in a
   weak topological order of the write graph (Bourdoncle 1993). Revisions then reach a module
   once per strongly connected component, not once per discovered writer.
   Use the write graph for scheduling once its edges are known. A convergence boundary belongs
   at each cyclic component exit, including nested components. A graph revision can add a writer
   edge and require scheduling that component again. [Deterministic Parallel Fixpoint Computation, §§4–7](https://arxiv.org/abs/1909.05951)
   Revision waves now group known active writers and receivers by component. A component
   waits while an ancestor component has pending revisions. A new admission or demand can
   change the order before the next wave. Unrelated components keep the same batch, and the
   convergence check still covers writes through unknown receivers. For revisions within a
   cycle, remove its already evaluated head and settle its nested components before revisiting
   the head. Dependent components wait for the outer exit. A consumer known to escape a callable
   is also an opaque-activation premise producer for the defining module. The revision order
   includes that dependency. An active writer with an unknown receiver can affect every module,
   so a pending revision for that writer participates in the order for all admitted receivers.
   The summary solver remains open.
5. *Incremental records.* The scan producer becomes a leaf of `compiler_analysis` with no CLI
   imports, and the `molt.frontend` package import becomes lazy (PEP 562). Each record kind is
   keyed by its producer's own import closure, so a frontend edit keeps scan records valid.
   The import-scan fingerprint now covers the scan producer's 32 Python source dependencies,
   the source decoder and the record codec. The import walk includes function-local imports
   and rejects dependencies on CLI or frontend code. A real store test keeps a scan after
   changing frontend bytes and invalidates it after changing a binding producer. See
   [Frontend semantic fingerprint](../../../../design/frontend_semantic_fingerprint.md).
6. *Laziness in development.* A development build does not wait for the whole closure: it
   analyzes on demand, and the JIT lane compiles a function on its first call (no interpreter
   fallback). A release build keeps the full closed-world solve.

#### Development source-to-TIR materialization

Proposed for integrator review.

The frontend owns an operation that materializes a body by its function identity in a frozen
compilation plan. The plan seals source bytes, target, settled namespace facts and declaration
identities. Declarations carry signatures and the metadata needed to create function objects,
closures and tasks. The entry and module initializers seed demand. Calls, value escapes, import
custody and observable metadata add demands through the existing source-value graph. An unresolved
selection demands every eligible body in its current conservative set.

One owner produces final TIR and its existing function digest for each demanded definition.
Concurrent requests share its result or its diagnostic. The compiler publishes the result only
after it has validated the declaration, frame layout, referenced definitions and runtime symbols
against the sealed plan. Whole-program diagnostics demand every body through the same operation.
Release compilation does the full solve through this operation as well.

The first integration settles source demands during compilation and passes the sealed TIR closure
to the existing JIT image and first-call code owner. Every runtime-visible function and launch
descriptor must have custody before publication. The runner keeps stable entries, launch
metadata and first-call native compilation. Deferring source-to-TIR work beyond launch would
require a runner-owned materializer for sealed inputs that can execute independently of the
frontend host process. The current directly executable image contract requires that independence.

Discovery owns the demand plan and the minimum frontend entry points. The JIT lane owns acceptance
at `jit_pipeline.py` and the image and runner contracts. Qualification compares demanded TIR and
diagnostics with the eager build, checks that an unreachable body stays unanalyzed, and exercises
an escaped callable, an import-selected callable, a closure, a resumable frame and concurrent
first calls. Measure launch and first-call latency separately.

**Invariants.**
- A summary is a pure function of (source digest, target plan, analysis schema, premise inputs).
- The global solve reaches the same fixpoint as today's replay: differential results and the
  emitted graph are unchanged.
- A refused or unknown dynamic import widens to today's conservative edge set, never narrows.

**Proof per stage.**
- Cold and warm discovery time for the program above and for the five slow tests (HF-938).
- Fixpoint count: target one per module.
- Graph identity against the current discovery over the basic and stdlib differential sets.

**Alternatives rejected.**
- A hand-listed narrow key: unsound, because it misses imports added later.
- Tuning the binding trie alone: it shrinks a 69% share but keeps the reruns.

## Non-Negotiable Gates
1. a required intrinsic that nothing reads hard fails (no anchors, spec 0016).
2. top-level union coverage hard fail.
3. submodule union coverage hard fail.
4. module/package collision hard fail.
5. selected/bootstrap roots must have implemented source owners; inventory does not require every optional import to succeed.
6. required runtime symbols and module custody must be admitted; ordinary Python exception behavior is qualified through API differential tests.
7. `intrinsic-partial` and `stub` ratchets hard fail (`tools/stdlib_intrinsics_ratchet.json`).
8. memory regressions in differential runs are blockers (`MOLT_DIFF_MEASURE_RSS=1`, process cap 10GB).

## Program Board
| Phase | Scope | Status | Entry Gate | Exit Gate |
| --- | --- | --- | --- | --- |
| 0 | Enforcement spine | Completed | Intrinsics audit in CI | `tools/check_stdlib_intrinsics.py` + `tools/check_core_lane_lowering.py` green |
| 1 | Core-lane closure lowering | Completed | Phase 0 complete | `tests/differential/basic/CORE_TESTS.txt` green + closure implemented |
| 2 | Concurrency substrate | In progress | Phase 1 complete | `socket` + `threading` + `asyncio` clusters green in native and wasm |
| 3 | Core-adjacent stdlib | Planned | Phase 2 complete | target families promoted to `intrinsic-backed` |
| 4 | Capability-gated long tail | Planned | Phase 3 complete | shipped compiled surface has no `stub` modules |

Specialized long-tail plan:
- Tkinter family (`_tkinter`, `tkinter`, `ttk`, dialogs) is tracked in
  `docs/spec/areas/compat/plans/tkinter_lowering_plan.md`.

## Acceptance Template (Required For Each Module Conversion)
1. Choose the semantic owner: CPython's own Python source for that target version (compiled,
   patched only where the verified subset excludes a construct, each patch naming its rule), or a
   Rust runtime primitive for a C module or a measured hot path. Never a Molt rewrite of CPython's
   Python (AGENTS.md; HF-820 measures the current rewrites and stubs).
2. For a runtime primitive, update its manifest and generated bindings together.
3. Keep one implementation owner; argument/error/capability handling follows its real API.
4. Targeted differential tests prove supported semantics, including failure propagation.
5. Run the complete audit once with `--critical-allowlist`; combine `--allowlist-modules` for additional selected roots.
6. Qualify native and WASM behavior and the relevant performance/size/compile-cost cells.

Source classification is an inventory, not evidence of runtime parity. Optional
accelerators may be absent when the admitted compiled algorithm has the correct
Python behavior. No host interpreter is consulted. The
[source authority](../../../../internals/source_scan_authority.md#what-source-inventory-establishes)
owns this distinction; coverage and performance require actual target receipts.

## Blocker-First Execution Order
1. Import system blockers and resolver correctness.
2. Concurrency substrate in strict order:
   - `socket` + `select` + `selectors`
   - `threading`
   - `asyncio`
3. Core-adjacent performance/semantic modules:
   - `builtins`, `math`, `re`, `struct`, `time`, `inspect`, `functools`, `itertools`, `operator`, `contextlib`
4. Capability-gated long tail:
   - import/filesystem cluster
   - data/codecs cluster
   - networking/process cluster
5. Final hardening:
   - ratchet to `intrinsic-partial == 0` and `stub == 0`

## Differential Execution Policy
Per cluster:
1. targeted native differential lane first,
2. targeted wasm parity lane second,
3. full sweep only after targeted lanes are green.

Required constraints:
- `MOLT_DIFF_MEASURE_RSS=1`
- default adaptive process/tree/global RSS guard plus adaptive child rlimit

## Weekly Scoreboard (Required)
1. intrinsic-backed count
2. intrinsic-partial count
3. python-compiled count
4. stub count
5. missing required top-level entries
6. missing required submodule entries
7. native differential pass %
8. wasm parity pass %
9. memory regressions (count + top offenders)

Any PR that worsens the scoreboard must include explicit exception sign-off and rollback plan.

## Update Workflow (Python Release Advance or Major Sweep)
1. Pin the new CPython tag and commit in `config/cpython_regrtest_sources.toml`,
   then `python3 tools/gen_stdlib_module_union.py --refresh-sources`
2. `python3 tools/gen_stdlib_module_union.py --write`
3. `python3 tools/gen_stdlib_stubs.py --write`
4. `python3 tools/check_stdlib_intrinsics.py --update-doc`
5. `python3 tools/check_stdlib_intrinsics.py --critical-allowlist`
6. sync rollup docs:
   - `docs/spec/STATUS.md`
   - `ROADMAP.md`
   - `docs/OPERATIONS.md`
