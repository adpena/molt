# Production Debug DX Hardening Design

**Status:** Approved
**Date:** 2026-04-08
**Scope:** Canonical debugging, repro, verification, tracing, reduction,
differential, and performance DX across the Molt compiler, runtime, and
backends
**Primary goal:** Make Molt debugging and correctness workflows feel
production-hardened and compiler-owned, with one canonical authority, no legacy
clutter, no backward-compatibility burden, and deterministic evidence for every
serious failure or performance claim

---

## 1. Goal

Redesign Molt's debugging and compiler-engineering DX around one coherent,
first-class system owned by the compiler/runtime product itself.

The target shape is closer to LLVM, Go, and other production toolchains than to
a repo full of partially overlapping scripts:

- one canonical `molt` debug surface;
- first-class structural verifiers at phase boundaries;
- deterministic tiny repro harnesses;
- filterable IR dumps at every meaningful layer;
- narrow, high-signal trace switches with structured outputs;
- one-command reducers and pass bisection;
- explicit differential and performance matrices;
- aggressive deletion of legacy duplicate entrypoints.

The system must make it easier to debug deep correctness bugs, prove
cross-backend parity, catch stale exception state and arg-binding corruption
immediately, and move reduced failures into permanent regression suites instead
of leaving them in terminal history or chat.

## 2. Non-Goals

- Preserving duplicated legacy DX surfaces for backward compatibility.
- Keeping expert-only scripts as permanent parallel authorities beside `molt`.
- Adding best-effort debug behavior that silently omits layers, traces, or
  invariants without reporting that omission.
- Building a second artifact tree outside the canonical `logs/` and `tmp/`
  roots.
- Accepting "debug print" culture as a substitute for structured trace/dump
  contracts.
- Introducing reducers or verifiers that only understand one backend while
  pretending to be cross-backend tooling.
- Treating reduced failures as disposable scratch outputs instead of canonical
  regression inputs.

## 3. Problem Statement

Molt already has useful pieces of a serious DX stack, but they do not yet form
one clean production-hardened system.

The current shape has six structural weaknesses:

1. **Too many partial authorities.**
   `src/molt/cli.py`, `tools/ir_dump.py`, `tools/ir_probe_supervisor.py`,
   `tools/check_molt_ir_ops.py`, `tools/profile_analyze.py`, backend-specific
   env flags, and scattered runtime trace switches each own part of the answer.

2. **IR and trace surfaces are not yet unified.**
   Some layers expose ad hoc dumps (`TIR_DUMP`, backend log files, standalone
   scripts), while other layers require direct code edits or bespoke env
   switches to inspect.

3. **Structural invariants are under-productized.**
   Important correctness rules such as block-param validity, exception-state
   cleanliness, successful-return pending-exception absence, and arg-binding
   container correctness are not exposed as one canonical verifier system.

4. **Repro and reduction loops are too artisanal.**
   Engineers are already writing strong tiny repros such as
   `tensor_pack_loop_repro.py`, `struct_pack_many_repro.py`, and safetensors
   mini drivers, but the repo does not yet provide one authoritative reduction
   and promotion path from bug report to minimized regression.

5. **Legacy tooling creates clutter and code smell.**
   Helpful scripts exist, but too many of them own behavior directly instead of
   delegating into a shared internal core. That raises the long-term risk of
   drift, duplicate semantics, and inconsistent artifact contracts.

6. **Performance and correctness evidence remain too fragmented.**
   Per-pass timing, cache counters, alloc/refcount counters, backend parity
   deltas, and reduced failure evidence are not yet emitted through one
   deterministic schema and one obvious command family.

The result is engineering friction:

- root-cause loops are slower than they should be;
- correctness contracts are harder to enforce uniformly;
- equivalent debug concepts are exposed through multiple surfaces;
- some failures still live in scratch files or terminal scrollback too long;
- the codebase carries more tooling clutter than a production-hardened compiler
  should tolerate.

## 4. Design Principles

1. **One authority.**
   `molt` is the canonical user-facing and engineer-facing debug authority.
   Scripts and helper entrypoints do not own parallel semantics.

2. **Compiler-owned observability.**
   Verifiers, dumps, reduction hooks, pass timing, and trace contracts are
   product features of the compiler/runtime stack, not sidecar scripts.

3. **Deterministic evidence before claims.**
   Every correctness or performance claim must be supported by reproducible
   manifests, logs, dumps, counters, and regression artifacts.

4. **No backward compatibility for clutter.**
   When a legacy script, env-only workflow, or duplicate interface is replaced
   by the canonical surface, it is deleted unless an internal non-user-facing
   integration point strictly still requires a delegate during the same
   convergence tranche. There is no compatibility lane for preserving obsolete
   public interfaces.

5. **Clean codebase over tooling sentimentality.**
   Legacy DX code that creates clutter, duplicate semantics, or stale concepts
   must be deleted even if it was once useful.

6. **Narrow traces, loud invariants.**
   Trace switches should be precise and cheap to enable for one failing area.
   Invariant failures should be explicit, attributable, and trap-capable in
   debug mode.

7. **Text for humans, JSON for machines.**
   Every major debug command emits readable default text and stable JSON on
   request, with deterministic field names and exit behavior.

8. **No silent feature absence.**
   If a backend or layer cannot provide a requested dump, verifier, or counter,
   the command reports that explicitly as an unsupported capability. It does not
   quietly degrade.

9. **Regression promotion is part of the workflow.**
   A reduced failure is not complete until it is promoted into a permanent
   regression suite or a committed verifier fixture.

## 5. Canonical Debug Architecture

The debugging system becomes one shared internal subsystem under `src/molt`,
tentatively organized as:

- `src/molt/debug/contracts/`
- `src/molt/debug/commands/`
- `src/molt/debug/probes/`
- `src/molt/debug/verifiers/`
- `src/molt/debug/reducers/`
- `src/molt/debug/diff/`
- `src/molt/debug/perf/`

These modules own the canonical contracts for:

- run manifests;
- artifact naming;
- output schemas;
- capability reporting;
- per-layer dump adapters;
- verifier result taxonomies;
- trace event schemas;
- reduction oracles and minimization outputs;
- performance counter aggregation.

`src/molt/cli.py` exposes the only canonical command surface. Runtime and
backend code expose instrumentation hooks and capability descriptors to the
shared debug core rather than implementing user-facing debug workflows
themselves.

### 5.1 Layer responsibilities

**`contracts/`**

- run metadata schema;
- artifact directory structure;
- common JSON result schema;
- backend/layer capability descriptors;
- failure classification enums.

**`commands/`**

- CLI argument parsing and flag normalization;
- translation of user-facing flags into shared internal requests;
- consistent text and JSON rendering;
- exit-code policy.

**`probes/`**

- AST/TIR/CFG/lowered/backend IR capture;
- trace event production;
- pass timing and per-function compile telemetry;
- low-level runtime instrumentation shims.

**`verifiers/`**

- structural IR verifiers;
- exception-state verifiers;
- arg-binding and call metadata verifiers;
- backend contract verifiers;
- sanitized debug assertions.

**`reducers/`**

- source testcase reduction;
- pass bisection;
- backend/configuration bisection;
- oracle definition and failure signature retention.

**`diff/`**

- CPython vs Molt execution comparison;
- optimized vs unoptimized comparison;
- IC on vs IC off comparison;
- backend/target comparison across native, wasm, and LLVM.

**`perf/`**

- per-pass timers;
- counters for cache, alloc, refcount, block/phi, and related hot metrics;
- sample/profiler driver integration;
- human-readable and machine-readable summaries.

### 5.2 Authority rule

Any existing or new DX tool must satisfy one of these conditions:

1. it is a `molt` subcommand;
2. it is an internal library used by `molt`;
3. it is a thin delegate that calls the same shared core and exists only when a
   non-CLI automation integration point strictly requires it and the delegate
   is not a documented public authority.

Anything else is legacy clutter and must be removed.

## 6. Canonical Command Surface

The production command family is:

- `molt debug repro`
- `molt debug ir`
- `molt debug verify`
- `molt debug trace`
- `molt debug reduce`
- `molt debug bisect`
- `molt debug diff`
- `molt debug perf`

Shared selectors and flags, where applicable:

- `--function <name>`
- `--module <name>`
- `--pass <name>`
- `--layer <name>`
- `--backend native|wasm|llvm`
- `--profile dev|release`
- `--format text|json`
- `--out <path>`
- `--manifest <path>`
- `--fail-fast`
- `--strict-capabilities`

### 6.1 `molt debug repro`

Owns tiny deterministic repro execution and evidence capture.

Required behavior:

- run a one-file failing or diagnostic-focused program deterministically;
- record exact command, profile, backend, environment knobs, and source hash;
- optionally compare against CPython or another Molt lane;
- emit the canonical manifest even on failure.

This command becomes the institutional home for the "tiny one-file contract
repro" workflow already happening informally.

### 6.2 `molt debug ir`

Owns phase-accurate IR dumping.

Required behavior:

- dump one or more requested layers;
- filter by function/module;
- filter by pass or dump every pass boundary;
- produce stable text and JSON;
- emit exact capability errors when a layer is unavailable on a backend.

### 6.3 `molt debug verify`

Owns structural verifier execution.

Required behavior:

- run requested verifier classes against requested layers/backends;
- fail with attributable diagnostics that name function, pass, and artifact;
- support hard-fail debug assertions for runtime/state invariants;
- integrate cleanly with CI and local targeted debugging.

Verification activation must not split into separate implementations.

Contract:

- `molt debug verify` is the canonical targeted verifier runner;
- `molt build`, `molt run`, `molt compare`, and `molt diff` may expose verifier
  enablement flags or profiles, but they must call the same verifier core and
  result model used by `molt debug verify`;
- always-on verifier subsets, if any, must be documented as named bundles in
  the shared verifier registry rather than implemented as ad hoc duplicated
  checks in individual command paths.

### 6.4 `molt debug trace`

Owns scoped trace enablement and rendering.

Required behavior:

- enable only the requested trace families;
- support function/module/pass filters;
- render reason codes, not just free-form messages;
- write trace output through the canonical manifest and artifact roots.

### 6.5 `molt debug reduce`

Owns testcase reduction.

Required behavior:

- take a failing source or manifest as input;
- minimize source while preserving the oracle;
- retain the failure signature and minimized artifact set;
- output a promotion-ready reduced repro.

### 6.6 `molt debug bisect`

Owns pass/configuration bisection.

Required behavior:

- locate the first bad pass or configuration delta;
- support pass-window minimization;
- support backend/config/profile/IC toggles as bisect dimensions;
- record exact bisect decisions and retained failure signature.

### 6.7 `molt debug diff`

Owns differential execution matrices for focused debugging.

Required behavior:

- compare CPython vs Molt;
- compare optimized vs unoptimized;
- compare IC enabled vs disabled;
- compare native vs wasm vs LLVM;
- record exact mismatch category and retained outputs.

### 6.8 `molt debug perf`

Owns targeted performance debugging and counter summaries.

Required behavior:

- per-pass timing;
- compile/run wall time breakdowns;
- cache hit/miss counters;
- alloc/refcount counters;
- block/phi and similar structural counts;
- optional integration with sampling/profiling tools.

## 7. Artifact And Manifest Contract

Debug outputs must use canonical artifact roots only.

Retained evidence lives under:

- `logs/debug/`

Ephemeral local debug outputs live under:

- `tmp/debug/`

Suggested subtrees:

- `logs/debug/repro/`
- `logs/debug/ir/`
- `logs/debug/verify/`
- `logs/debug/trace/`
- `logs/debug/reduce/`
- `logs/debug/bisect/`
- `logs/debug/diff/`
- `logs/debug/perf/`
- `tmp/debug/...` mirrors when persistence is not requested

Every debug run writes a manifest containing at minimum:

- run id;
- timestamp;
- command and subcommand;
- backend and profile;
- selected filters;
- enabled traces/assertions;
- source path and source hash;
- capability report;
- output artifact paths;
- final status and failure classification.

No serious debugging flow should rely on unnamed temporary files, ad hoc output
directories, or terminal scrollback as the only retained evidence.

## 8. IR Dump Contract

IR dumping becomes first-class across all meaningful layers:

- AST
- typed IR
- CFG
- lowered IR
- backend IR
- final machine-oriented or backend-native IR:
  - CLIF for Cranelift lanes
  - LLVM IR for LLVM lanes
  - wasm text or structured wasm-layer representation where applicable

### 8.1 Filtering

IR dumping must support:

- only this function;
- only this module;
- only this pass;
- full pass-boundary dump;
- stable ordering of functions and passes.

### 8.2 Output

Each dump should include:

- layer name;
- function name;
- pass name when relevant;
- stable fingerprint;
- op/block counts where meaningful;
- verifier status for that snapshot when requested.

### 8.3 Existing surface consolidation

The following current surfaces are replaced or internalized by this contract:

- `tools/ir_dump.py`
- backend-specific `TIR_DUMP` behavior
- ad hoc backend IR log files under `logs/`

They may survive only as internal delegates for in-repo automation that has not
yet been switched in the same convergence tranche. They do not remain
documented or supported user-facing authorities.

## 9. Structural Verifiers And Sanitized Debug Modes

Verifiers are mandatory first-class product features, not optional polish.

Required verifier classes:

- SSA invariants;
- block-param invariants;
- CFG edge and exception-edge invariants;
- exception-state invariants;
- arg-binding invariants;
- call metadata invariants;
- "no pending exception on successful return" invariants;
- backend/lowering contract invariants that protect cross-layer coherence.

### 9.1 Sanitized debug modes

The runtime and backends must expose trap-capable debug assertions for the
highest-value stale-state failures.

Required initial assertions:

- `MOLT_ASSERT_NO_PENDING_ON_SUCCESS=1`
- trap on invalid block param creation
- trap on wrong arg container type
- trap on stale exception state entering success-only edges

These switches are low-level assertion knobs owned by the shared debug core and
surfaced through the canonical debug commands. They do not define a second
public interface beside `molt debug`.

These must integrate with `molt debug verify` and `molt debug trace` rather
than living as isolated ad hoc runtime checks.

### 9.2 Failure behavior

A verifier or assertion failure must:

- identify the function/module/pass;
- report the invariant class;
- include or reference the relevant artifact snapshot;
- use deterministic failure text and JSON;
- support `SIGTRAP`-friendly local debugging when explicitly enabled.

## 10. Trace Contract

Trace support must be narrow, structured, attributable, and cheap enough to use
for real debugging instead of only desperate debugging.

### 10.1 Immediate required traces

The first required high-value trace set is:

- `MOLT_TRACE_CALL_BIND_IC=1`
- `MOLT_TRACE_CALLARGS=1`
- `MOLT_TRACE_FUNCTION_BIND_META=1`
- exception flow traces
- per-function compile traces
- per-pass timing traces

These env vars are low-level runtime/backend switches surfaced by the canonical
`molt debug trace` flow. They do not define a separate public authority beside
the CLI.

### 10.2 Required semantics

`MOLT_TRACE_CALL_BIND_IC=1`

- log when a direct-function IC entry is installed;
- log when an IC entry is bypassed;
- log the exact reason code for bypass or refusal to install;
- include site id, callable identity, arity, and bind-requirement metadata.

`MOLT_TRACE_CALLARGS=1`

- dump builder contents before `molt_call_bind`;
- include positional and keyword counts, names, and value type summaries;
- support filtering so one hot failing function does not flood the entire run.

`MOLT_TRACE_FUNCTION_BIND_META=1`

- log `__molt_arg_names__`, `__molt_vararg__`, `__molt_varkw__`,
  `__defaults__`, `__kwdefaults__`, and related bind metadata at bind time;
- show the exact metadata shape seen by the binder, not a reconstructed summary.

### 10.3 Trace output model

Trace outputs must support:

- terse human text by default;
- structured JSON events on request;
- reason codes and categorical fields;
- correlation to the run manifest and selected function/pass/module filters.

Scattered direct `std::env::var(...)` checks in runtime/backend code should be
converged behind a shared debug settings and trace event contract instead of
remaining an unbounded set of unrelated one-off toggles.

## 11. Repro, Reduction, And Bisection Contract

Reduction tooling must be first-class, not a remembered shell recipe.

### 11.1 Tiny deterministic repros

The canonical unit of deep bug isolation is a tiny deterministic one-file
program that isolates one contract.

The debug system must make it easy to:

- run the repro deterministically;
- attach traces/verifiers/dumps to it;
- compare it across backends and configurations;
- promote it into a permanent regression.

### 11.2 Canonical reduction entrypoint

`molt debug reduce` is the human-facing authority for reduction.

`MOLT_IR_REDUCE` may exist only as a low-level implementation knob used by the
canonical debug core and tests. It is not a second public authority beside the
CLI.

Reduction lanes:

- source testcase reduction;
- pass reduction;
- first bad pass identification;
- backend/configuration reduction.

### 11.2.1 Oracle model

Reducers and bisection flows must use one canonical oracle abstraction, because
the reduction engine is only as reliable as its retained failure predicate.

Required canonical oracle categories:

- process exit classification;
- verifier failure classification;
- structured diff mismatch classification;
- trace event or invariant signature match;
- manifest predicate over retained artifacts and result fields.

The reducer CLI may expose user-friendly shorthands, but manifests must record
the oracle in one normalized machine-readable format so different reduction
lanes do not invent incompatible predicate systems.

### 11.3 Required outputs

A successful reduction run must retain:

- original failing manifest;
- minimized source;
- failure oracle definition;
- reduced pass/config window if applicable;
- stable failure signature;
- promotion target recommendation.

### 11.4 Regression promotion

The end state of reduction is not "artifact exists under `tmp/`."

The end state is one of:

- committed differential regression;
- committed runtime/backend unit regression;
- committed verifier fixture;
- committed perf regression case.

## 12. Differential Execution Contract

Differential debugging must be exposed as a first-class product surface.

Required matrix dimensions:

- CPython vs Molt
- optimized vs unoptimized
- IC enabled vs IC disabled
- native vs wasm vs LLVM

The command model must report:

- exact comparison dimensions;
- mismatch class;
- stdout/stderr and structured payload differences;
- capability-based explicit skips or unsupported results.

There must be no silent fallback to "the other lane looked close enough."

## 13. Performance Debug Contract

Performance tooling must be part of the same debug system and share the same
manifest/artifact model.

Required capabilities:

- per-pass timers;
- per-function compile timing;
- cache hit/miss counters;
- block/phi counts;
- alloc/refcount counters;
- optional sample/profiler integration;
- machine-readable summaries consumable by bench and regression tooling.

The immediate standard is not "full perf lab automation." The immediate
standard is that a compiler engineer can prove where compile time or runtime
time moved without inventing a new ad hoc measurement loop each time.

## 14. Backend And Target Contract

The debug interface must be uniform across supported backends and targets, but
uniform interface does not mean fake parity.

Contract:

- the same `molt debug ...` command family exists across native, wasm, and
  LLVM;
- unsupported layers or traces are reported explicitly as unsupported
  capabilities;
- capability reports are part of the manifest;
- no backend may silently omit a requested diagnostic surface.

This keeps the command surface clean while preserving honesty about backend
coverage.

## 15. Legacy Deletion And Cleanup Policy

Legacy cleanup is a hard requirement of this design.

### 15.1 Files and surfaces to consolidate

At minimum, the implementation plan must explicitly converge or delete the
standalone-authority behavior in:

- `tools/ir_dump.py`
- `tools/ir_probe_supervisor.py`
- `tools/profile_analyze.py`
- direct user-facing `tools/check_molt_ir_ops.py` workflows, with its
  inventory/probe validation semantics migrated into the shared verifier core
  behind `molt debug verify` rather than discarded
- backend ad hoc `TIR_DUMP` behavior
- scattered undocumented env-only debug workflows that duplicate canonical
  `molt debug` behavior

### 15.2 No backward compatibility policy

When the canonical debug surface replaces a legacy user-facing entrypoint:

- docs are updated in the same change;
- tests are updated in the same change;
- the legacy entrypoint is deleted unless an internal automation dependency
  still requires a non-user-facing delegate during the same convergence
  tranche;
- any such delegate is undocumented, treated as transitional debt, and removed
  by the end of the planned convergence work. It does not count as the finished
  state.

The desired end state is a cleaner codebase, not a larger compatibility layer.

### 15.3 Clean codebase requirement

The implementation must reduce code smell and tooling clutter:

- fewer authorities;
- fewer duplicated argument parsers and output renderers;
- fewer special-case artifact layouts;
- fewer ad hoc env checks without shared ownership;
- fewer debug concepts that exist only in one script.

## 16. Acceptance Criteria

This design is ready for implementation planning only if the resulting plan can
deliver all of the following:

1. one canonical `molt debug` command family for repro, IR, verify, trace,
   reduce, bisect, diff, and perf;
2. deterministic debug manifests and canonical artifact roots under `logs/` and
   `tmp/`;
3. first-class IR dumps across the requested layers with function/pass filters;
4. structural verifiers and sanitized debug assertions, including
   `MOLT_ASSERT_NO_PENDING_ON_SUCCESS=1`;
5. structured trace support for `call_bind_ic`, `callargs`, and function bind
   metadata;
6. a one-command reducer/bisector workflow for failing compiled repros;
7. differential execution support for CPython vs Molt, optimization deltas, IC
   deltas, and backend deltas;
8. integrated performance counters and pass timing;
9. explicit convergence or deletion of legacy duplicate DX entrypoints;
10. committed regression promotion rules so reduced bugs do not live only in
    scratch files or human memory.

## 17. Planning Units

The implementation plan derived from this spec should break the work into a
small number of coherent units, not a flat pile of unrelated tasks.

Recommended planning units:

1. shared debug contracts, manifests, and command scaffolding;
2. IR dump and verifier core;
3. trace registry unification plus the required call-bind/exception assertions;
4. repro/reduce/bisect engine;
5. differential and perf integration;
6. legacy deletion, doc rewrites, and final convergence.

This keeps the work focused on one coherent subsystem while still allowing
delivery in production-safe increments.
