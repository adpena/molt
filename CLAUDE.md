# Claude Code Instructions

## ABSOLUTE TOP PRIORITY (read this first, every turn)

**Engineer like Chris Lattner / Mojo / NASA. Never take a shortcut, workaround, or "simpler" implementation when the structurally correct fix is harder.** This rule overrides every comfort instinct.

The temptation chain you must reject:
- "I'll just promote Value-tier shadows at loop_start to fix this faster" → NO. That is a localized hack on top of an architecturally broken design. Do the structural redesign (typed IR, eliminate the shadow system) instead.
- "I'll add a small guard to handle this edge case" → NO. The edge case exists because the abstraction is wrong. Fix the abstraction.
- "I'll commit the partial fix and follow up later" → NO. There is no later. Either land the complete fix or do not start.
- "The full fix is too risky, let me ship something safer" → NO. The "safer" thing accumulates compound interest of bugs. Take the time. Do it right.
- "I'll skip the perf step and come back to it" → NO. Perf-correctness gaps create distrust. Land the fast version with the correct version.

When you identify a structurally correct fix and feel pulled toward an "immediate win" or "incremental approach", **STOP**. That pull IS the signal you are about to ship a workaround. Land the structural fix even if it is multi-day work.

If you cannot complete the structural fix in this session, **do not commit the localized hack as a placeholder**. Leave a clean baton-pass note describing the structural fix needed; the next session picks it up. Half-measures committed to main are worse than nothing committed.

### Narrow Aperture, Full Structural Rip

Default to one narrow, named aperture into the real structure: one invariant,
command family, file cluster, authority surface, or failing execution path. The
aperture keeps discovery bounded; it is not the deliverable, not the commit
size, and not a smallest-next-chip plan. The deliverable is a complete
structural rip through the authority exposed by that aperture, followed through
every consumer needed to delete or unify the old lane.

- "Tiny slice and rip it open" is binding operator policy. Tiny slice means the
  smallest concrete opening that exposes the duplicate authority; it never means
  shrinking the engineering unit. Rip it open means delete or unify the duplicate
  authority behind that aperture before moving on.
- Do not start with a broad soup of goals, and do not stop at a tiny chip. Begin
  with the narrow aperture, then widen only along the structure it reveals:
  callers, tests, docs, generated facts, backend/frontend/tooling consumers, and
  proof lanes that govern the same invariant.
- The entry point may be narrow; the work may not be a chip. Do not scope
  broadly into endless planning before real structure is exposed, but once it is
  exposed, migrate the sibling authorities and consumers that define the
  invariant.
- Full structural rip means implementing the actual structure behind the entry
  point: the missing IR fact, one generated authority, the ownership boundary, or
  the shared primitive, plus the bug class it exposes inside that boundary, with
  zero workarounds.
- A forbidden chip is sized for process comfort: a checkpoint, a commit, a
  status line, or one local test. It leaves sibling authorities untouched. Reject
  it. Width follows the structure, not convenience.
- This kills both failure modes: endless breadth/planning with nothing changed,
  and surface patching that leaves the real structure intact. When uncertain,
  narrow the entry point and deepen the structural rip; never narrow the
  ambition.
- Crash recovery constrains process fanout, not work integrity. In unstable
  sessions, keep one active aperture and one bounded proof lane; never convert
  the structural rip into a queue of isolated tiny chips.
- Boldness is required once the aperture exposes structure. Expand to the whole
  coherent authority class, even when that is larger than the comfortable
  checkpoint, and delete or unify the old lane instead of preserving a hybrid
  path.
- No local minima, no smallest-next-chip progress, and no excessive
  test/conformance/proof apparatus as a substitute for changing the
  architecture.

### Comprehensive Analysis Spine: AST To Binary Closure

Every compiler/runtime change must bridge micro facts to macro architecture.
Parser, AST, lowering, IR, TIR, allocation, ownership, escape/effect/drop,
backend, runtime, binary-image, and DX facts are one structural chain. Do not
fix or upgrade one layer while leaving adjacent facts stale, duplicated, or
contradictory.

- Build new analysis around canonical fact paths, not scattered local scans.
  Parser and AST facts feed IR/TIR facts; TIR facts drive allocation, ownership,
  backend codegen, runtime custody, and binary closure; binary/allocation
  findings feed diagnostics, proof selection, and dependency risk.
- Dependency updates are compiler architecture work. Research current upstream
  changelogs and release notes from primary sources, update manifest and lock
  authorities together, and migrate API or semantic changes through every
  affected AST/IR/TIR/backend/runtime consumer.
- Never pin around a breakage, downgrade for comfort, fake a toolchain surface,
  or accept a silent fallback. If latest dependencies expose a wrong local
  abstraction, move the abstraction.
- No-regression is structural. Prove parser/AST changes at the syntax layer,
  IR/TIR changes at the semantic layer, allocation/drop/escape changes at the
  memory-fact layer, backend API changes at codegen, and packaging changes at
  binary closure.
- Prefer one shared analysis primitive over repeated probes. If CLI setup,
  diagnostics, validation, binary closure, docs, and tests need the same fact,
  route them through the same implementation or generated authority.
- Canonical TIR data, op-kind, and representation facts live under
  `runtime/molt-ir/`: `runtime/molt-ir/src/tir/`,
  `runtime/molt-ir/src/tir/op_kinds.toml`,
  `runtime/molt-ir/src/tir/op_kinds_generated.rs`, and
  `runtime/molt-ir/src/repr.rs`. TIR analyses, pass orchestration, SimpleIR<->TIR
  transport, module/drop orchestration, target/profile descriptors, and
  value-keyed representation facts live under `runtime/molt-passes/`. Residual
  backend-neutral lowering and SimpleIR-name representation planning live under
  `runtime/molt-tir/`. Do not patch an old backend lane or add a local
  classifier when this generated authority can own it.
- WASM codegen, import planning, ABI registry emission, binary patching, and
  encoder-local tests live under `runtime/molt-backend-wasm/src/`. The legacy
  `molt_backend::wasm` path is only a feature-gated facade reexport; do not add
  new WASM encoder modules, ABI manifests, or import tables under
  `runtime/molt-backend/src/`.

### Anomaly Crux Protocol: Question The Question

When weird behavior appears, do not answer only the first-order question. Zoom
out before patching, widening proof, or accepting a profile/workflow refusal as
the task. Ask what fact, authority, import edge, IR node, runtime feature gate,
binary artifact, or ownership boundary made the weird behavior possible.

- Treat anomalies as architecture evidence. A failed proof lane, surprising
  dependency pull, unexpected feature gate, silent fallback, timeout, generated
  drift, or profile refusal is a prompt to find the missing or duplicated
  authority, not merely an instruction to rerun with a larger profile.
- Never stop at the first-order question. If the visible question is "why did
  this command fail?", "why does this import need regex?", "why did the process
  vanish?", or "why did this profile refuse to build?", also ask which upstream
  model, ownership boundary, generated fact, dependency edge, or host-control
  contract made that question appear in the first place.
- Always identify the crux before acting: the smallest fact that, if corrected,
  collapses the confusing behavior across consumers. Then rip through that
  authority class instead of stacking local fixes.
- Ask "why this question?" as well as "what is the answer?" Example: if a
  codec-only proof pulls `stdlib_regex`, the question is not just how to enable
  regex; it is which import edge, stdlib bootstrap dependency, feature gate, or
  tree-shaking boundary caused a codec path to require regex, and whether that
  edge is semantically necessary.
- Prefer structural deforestation over profile inflation. Remove avoidable
  intermediate objects, import edges, generated duplicates, facade fallbacks,
  and heavyweight runtime features from hot or minimal paths when semantics do
  not require them.
- Use subagents for crux-finding on independent axes: one can map import/feature
  graph causes, another can check upstream/OSS semantics, and another can audit
  data layout or binary-size impact. Do not use subagents to create more status
  chatter or redundant proof loops.
- When an anomaly involves compatibility, verify against primary sources
  (current repo authority first, then upstream CPython/OSS docs or source where
  relevant) before narrowing or broadening support claims.
- Current Molt regex crux: a codec-only proof pulling `stdlib_regex` means the
  module graph has overreached. The known failure mode is
  `codecs -> os -> TYPE_CHECKING typing -> warnings -> re`, amplified by broad
  `typing` scans. Fix the static import facts, constant-guard pruning, and
  tree-shaking authority before widening profiles or treating regex as a codec
  dependency.

### Concrete examples of partial implementations to reject

These are real shortcuts caught and reversed in past sessions. Do not repeat them:

- **Compressing architect/research output**: When a sub-agent returns a 1500-line architecture plan, write the *full* text to disk. Condensing to "key points" loses the line numbers, sub-phase test specifications, and risk treatments that an implementing agent needs. The architect's full text is the artifact.
- **Asymmetric coverage of a structural fix**: If you migrate the in-loop call sites of a helper, also migrate the out-of-loop sites. If you fix the int lane's shadow system, mirror the same change to bool/float/str. Asymmetry is a partial implementation that re-creates the original bug at the unmigrated site.
- **Splitting an atomic refactor across commits to "make progress visible"**: If Phases 1a/1b/1c/1d are one structural arc per the design, ship them as one atomic change or commit them with explicit "this leaves the codebase in a hybrid state until 1d lands" notes. Three commits that shipped 1a/1b/1c without 1d leave two parallel sources of truth in the tree — exactly the compound-interest-of-bugs trap this policy exists to prevent.
- **Stopping at the first measurable win**: A 10% perf bump from Phase 1b is not "good enough" if Phase 1d would yield 50%. The 10% does not justify halting the structural change.
- **"Debug-gated assertion" as a substitute for migration**: An assertion that catches divergence between the static and dynamic sources of truth is a verification tool, not a substitute for unifying them. Verify the invariant *while* completing the migration, not as a way to defer it.
- **Per-test special cases**: If a test fails after a structural change, the change is wrong (or the test reveals a missing invariant). Do not add a guard that special-cases the failing test.
- **Renaming `_unused`** to silence a compiler warning instead of using or removing the variable: pick one. Both options are clean; the rename is a shortcut.

### Structural change as the unit of work

The unit of work is the *complete structural change*, not the smallest committable diff. When the design says "Phase 1 = 1a + 1b + 1c + 1d", Phase 1 is not done until 1d lands. Intermediate commits are acceptable only when each is itself a complete structural piece (not a partial fix toward the next piece) and a baton-pass note documents the remaining unfinished arc.

Before choosing work size, identify the whole structural work class: every
neighboring duplicate authority, every call site, every backend/frontend/tooling
consumer, every generated table, every proof lane, and every doc route that is
part of the same invariant. Do not take the smallest visible board item, one
match arm, one failing test, one file-local patch, or one easy metric decrement
when the evidence shows a larger shared abstraction is being exposed. Burning
down tiny counts while leaving the surrounding duplicate authority intact is
avoidance, not progress.

A smaller landing is valid only when it is a complete end-state subsystem cut:
it exhausts that invariant's duplicate authorities inside the touched subsystem,
has no adjacent same-kind dispatch/fact/source-of-truth left behind, and gives
future work a stronger foundation instead of another seam. If the first proposed
unit leaves a sibling classifier, parallel backend lane, mirrored frontend path,
or same bug class still open next to it, expand the unit until the whole class is
unified. Use baton-pass notes only for genuine external blockers or proof lanes
that cannot run in the current environment; never use them to excuse a tiny-chip
sequence.

Convenient tiny-chip progress is the silent death of this project. It creates
the feeling of velocity while preserving exactly the scattered authorities that
make correctness, performance, and compatibility non-compounding. Any agent that
keeps choosing tiny audit-row burn-down, "safe" local edits, or narrow proof
loops after the operator asks for deeper structural work is refusing the task.
When the operator says a plan is too small, stop immediately, discard the
comfort-sized plan, re-open the design radius, and attack the whole coherent
work class.

Bold structural convergence outranks local neatness. Avoid local minima,
overfitted proof apparatus, and excessive conformance/testing loops that serve
as a substitute for changing the architecture. Verification is mandatory only
insofar as it proves the structural invariant being moved; once it proves that
invariant, return to unifying the system instead of orbiting the tests.

Operator correction is binding. If the user says the work is being sliced too
small, says "tiny slice", says "rip it open", or uses equivalent language, do
not defend the current plan, do not rename the slice, and do not continue the
same local tactic. Name the aperture, expand through the underlying structural
class it exposes, and proceed.

This rule applies equally to:
- **Correctness**: bug class fixes, not bug instance fixes (e.g., fix the phi-representation invariant, not just the one site that exposed it)
- **Optimization**: structural codegen changes, not localized peephole tweaks
- **Performance**: redesign the hot path, do not add bypass cases
- **Architecture**: rework the abstraction, do not stack patches on a wrong foundation

Performance contract: molt MUST be faster than CPython on every benchmark, across every target (native, WASM, LLVM, Luau) and every profile (release-fast, dev-fast, debug-with-asserts). Do not declare a perf task complete until measurements confirm it on all targets.

## Top Priority: Tinygrad + DFlash Fidelity

This is a turn-blocking policy.

- Exact tinygrad semantics and API shape are the public ML contract. No drift is acceptable.
- Exact DFlash algorithmic fidelity is non-negotiable when implementing DFlash support. Do not ship generic speculative decoding under a DFlash label.
- `molt.gpu` and `molt.gpu.dflash` are implementation layers, not excuses to diverge from tinygrad or the DFlash paper/project.
- If the official DFlash design requires target-conditioned draft behavior, verifier/drafter separation, hidden-feature conditioning, KV injection, or a trained drafter, preserve those requirements. If a model lacks a real trained DFlash drafter, say so explicitly and do not fake support.
- If you detect existing drift from tinygrad or DFlash source-of-truth behavior, clean that drift up before adding more code.

## ABSOLUTE NON-NEGOTIABLE: Zero Workarounds Policy

This is an early alpha project. We are the sole users and developers. There is ZERO tolerance for:

- **Workarounds** of any kind. If the correct fix requires refactoring, do the refactoring.
- **Hacky fixes**. No regex where structural parsing is needed. No bare except. No magic constants.
- **Partial fixes**. If a fix addresses 80% of cases, it's not done. Fix 100%.
- **TODO/FIXME as excuse to ship broken code**. If you write a TODO, implement it in the same turn.
- **"Simpler fix" that avoids the real problem**. The "simpler" path is always the workaround. Do the correct fix.
- **Technical debt**. We are building foundations. Every line of code must be defensible for the long term.
- **Code smell**. If something feels wrong, it is wrong. Fix it properly.
- **Silent failures or divergences from CPython >= 3.12**. Full deterministic parity except: no exec/eval/compile, no runtime monkeypatching, no unrestricted reflection.
- **Bypassing safety checks** (--no-verify, catch_unwind to swallow panics, etc.)
- **Sharp edges** left for "later". There is no later. Fix it now.

When you identify the correct fix and feel tempted to do something "simpler" instead — STOP. That temptation IS the signal that you're about to create a workaround. Do the correct fix.

## Engineering Standards

- **Correctness first, performance second, elegance third**. But all three are required.
- **NASA-grade quality**. Every change must be defensible as if deployed to production at scale.
- **Full parity** with CPython >= 3.12 for all supported features, including edge and corner cases.
- **All backends** (native/Cranelift, WASM, LLVM) must have parity. No backend-specific workarounds.
- **Extreme optimization and performance**. Choose the most performant algorithm and data structure. No lazy shortcuts.

## Performance Constitution — speed is a correctness property (release-blocking)

Correctness parity is the FLOOR. Performance dominance is the PRODUCT CONTRACT. A molt
feature is not complete because it passes CPython differential tests; it is complete only
when it preserves or improves the performance contract across the relevant targets, profiles,
and backends. This is a release-blocking contract, not an aspiration.

**The bar:** molt must be faster than CPython on EVERY benchmark in the verified subset, on
EVERY supported target, backend, and profile; and it must steadily approach, match, or exceed
PyPy and Codon on the benchmark classes where their execution models apply. Codon is the
AOT/native north star for the statically compilable subset (10–100×+ over CPython, C/C++-class,
non-drop-in semantics). PyPy is the dynamic-runtime reference (~3× over CPython 3.11 via JIT)
for pure-Python dynamic workloads.

**Non-negotiable gates — every correctness landing must answer "what did this do to speed?"**
- A commit that fixes parity but introduces a permanent benchmark regression is INCOMPLETE.
  Silent slowdown is a FAILED landing. If a structural fix necessarily slows a path
  temporarily, the commit must state exactly which perf debt was introduced, why it is
  unavoidable, which invariant now enables recovery, and which follow-up arc retires it.
- CPython is the absolute floor: faster on every verified-subset benchmark. Any benchmark
  below 1.00× vs CPython is RED and is a contract violation, not "later optimization work."
- PyPy is the dynamic reference: match/beat on JIT-favorable pure-Python workloads, or NAME the
  missing compiler fact (IC tiering, class-version guard, borrow inference, generator fusion,
  shape propagation, trace-like loop specialization).
- Codon is the AOT reference: approach/exceed on numeric/loop/data-structure/NumPy-like/typed
  kernels where semantics match; mark non-equivalent semantic models as "non-equivalent," never
  as a win/loss.
- A backend "degradation" must be a DOCUMENTED target limitation (an explicit portable-IR fact),
  never a hidden benchmark exception. A profile-specific slowdown is still a bug: dev may
  optimize compile latency, but release-fast/release-output are held to shipped-perf standards.

**Methodology — pyperformance/pyperf discipline, not vibes.** Every perf claim reports:
`benchmark → target → backend → profile → CPython ratio → PyPy ratio (when applicable) →
Codon ratio (when applicable) → binary size → peak RSS → compile time → command/log artifact`.
Repeated worker runs, calibration, instability detection, statistics, JSON output. No
"looks faster," no cherry-picked one-off, no warm-cache-only wins (report cold AND warm). No
benchmark is healed until measured against the full matrix it affects.

**Required machine-readable scoreboards** (kept green, CI-gated): (1) CPython — every benchmark
× backend/profile, any <1.00× is red; (2) PyPy — pure-Python dynamic, names the missing molt
mechanism where PyPy wins; (3) Codon — static/AOT subset on matched semantics; (4) Backend —
native/LLVM/WASM/Luau each its own table, a native win never excuses a WASM regression;
(5) Profile — dev/release-fast/release-output are separate products, none hides runtime regressions.

**Perf triage priority** (after P0 silent-wrong-answer + memory unsafety): (1) any benchmark
slower than CPython; (2) any previously-green benchmark that regressed; (3) any backend/profile
divergence losing a known optimization; (4) any PyPy/Codon gap where molt lacks the needed
representation fact; (5) binary size / cold start / RSS / compile-time regressions.

**Posture — do not "optimize passes," fix the REPRESENTATION.** When a benchmark is slow the
first question is never "which peephole recovers it" but "which FACT is missing from IR?": RC
overhead → ownership/borrow/reuse; dynamic dispatch → class identity/version/target/shape;
boxing → Repr precision; slow loops → induction/range/overflow/lane stability; slow generators
→ resumable-frame ownership + fusion eligibility; WASM losing a native opt → the fact belongs in
portable IR, not native codegen; release-output wins but dev unusable → profile-tier separation.

**Landing report format:** not just "tests green" but "tests green; perf matrix green; no
CPython-red benchmarks; PyPy/Codon deltas known; regressions zero or explicitly tracked with
owner and structural fix."

**Automated god-file ratchet:** `tools/structural_audit.py --check` is the
default detector for kitchen-sink files, undecomposed god-files, duplicate
authorities, and structural-debt growth, and `tools/ci_gate.py --tier 1` runs it
by default. Raw large-file size is board triage only; CI gates concern-mixing
files and lone undecomposed monoliths. Do not create or grow monoliths as an
implementation shortcut. Split authority into coherent modules/packages, and
never repin `tools/structural_audit_baseline.json` without explicit maintainer
acceptance plus a metric-correction or retirement plan.

## Council Operating Doctrine (2026-06-08, binding)

**Ratified fork resolutions** (full record: memory project_council_decisions_20260608):
- **Finalizer ordering goes on a minimal OWNERSHIP LATTICE, never as another DropInsertion
  special-case.** Build the smallest complete ownership aperture `alias-root → ownership state → Python lifetime
  boundary → ordered release obligation` (new `ownership_lattice_min.rs`/`ownership_boundaries.rs`),
  then ship ordering on it. Narrow is allowed; a disguised ad-hoc finalizer patch is not. This
  is the rung-1→rung-2 bridge — do NOT boil the ocean, do NOT re-patch DropInsertion.
- **`Free` is demoted.** For Python heap objects it is a backend/runtime LOWERING of a
  proven-unique DecRef only under `¬MayFinalize ∧ ¬HasWeakrefs ∧ ¬MayResurrect ∧
  ¬InnerRefOrdering ∧ ProvenUnique`; otherwise the only legal op is finalizer-aware DecRef.
  Runtime-internal finalizer-free frees get a SEPARATE opcode (`FreeInternal`/`FreeRaw`) — never
  share with "free Python object."
- **`MOLT_ASSERT_NO_LEAK` = actual destruction** (not zero-transition).
- **`FinalizerSensitive` = one ClassInfo/MRO/version-derived cached fact**, consumed by escape +
  refcount-elim + stack-alloc + Free-eligibility + ownership-lowering. No pass-local finalizer
  reasoning. Any optimization changing lifetime/placement/release-order/direct-free-eligibility
  consults the same fact.

**P0 ranking:** a resurrection/finalizer/weakref MEMORY-CORRUPTION bug (e.g. the resurrection-
at-scale SIGSEGV) OUTRANKS the native RC flip and all performance/feature work — it invalidates
trust in the memory model. Root-cause structurally; never cap the repro or mark it expected.

**Three-lane model** (non-overlapping files, continuous): A = P0 semantic safety (corruption,
finalizer ordering, ownership-lattice arc, flip blockers, leak/finalizer/weakref/unwind tests);
B = performance frontier (CPython-reds, regressions, PyPy/Codon harness, raw/boxed/dispatch/loop/
generator bottlenecks); C = infra/scoreboards/decomposition that makes A&B faster. A blocks B only
when memory unsafety makes perf numbers untrustworthy; B blocks new features when any benchmark
< CPython; C is never decorative.

**Every batch reports the PERF/SPEED STATUS block** (CPython-red benchmarks + suspected missing
fact; regressions; PyPy/Codon deltas where semantically comparable; fastest next unlock = one
fact / one file-lane / one gate). If it cannot be filled, the next task is to CREATE THE
MEASUREMENT PATH, not optimize blind. Perf work's deliverable is a NEW IR FACT that makes a
class of slow programs unexpressible — not "faster code." Five-year target = retire one CLASS
of slowness per month (the compression ladder), not one benchmark.

**Structural landing & evidence standard (binding).** A reported unit of work must CHANGE
PROJECT STATE — landed code/tests/tooling/docs, a verified refusal that deletes a bad plan, or
(only at a real fork) a decision packet with a recommended default — never "status + a question."
Build first; ask only when the next step encodes a semantic invariant, needs a public/API/subset
decision, faces two mutually-exclusive structural designs, would risk a workaround, or is
contradicted by memory-safety/correctness evidence; otherwise default, implement, measure, test,
report. Research→artifact→next-move; falsification must leave a durable artifact (test/doc/baton/
deleted-plan) or it didn't count.
- **Perf claims are quiescent, repeated, attributed, classified.** No optimizing from a noisy
  red; no warm-time claim from allocation counters alone (alloc-count is a memory-dimension
  signal — warm reds need CYCLE attribution); no "one run flipped it"; no stale-local-main
  authority. Classify every result GREEN / RED_STABLE / RED_NOISY / TIE / DIMENSIONAL_WIN. A
  DIMENSIONAL_WIN (alloc/RSS/binary/cold/backend improved, warm gate did not flip) is reported
  honestly as dimensional, NEVER as a speed heal.
- **Gates:** run the relevant gates for the touched surface, full gates before integration, and
  explicitly list any omitted gate with its reason. NEVER imply an unrun gate is green.
- Cold-start is an artifact-footprint/page-in/codesign problem, NOT a runtime-init problem
  (runtime init measured 0.127ms); it is WARN under the v0 budget, not an execution-engine red.

## Bootstrap Authority (Non-Negotiable)

- Runtime-known module bootstrap must go through the runtime import boundary (`MODULE_IMPORT`). Do not split bootstrap ownership between frontend special cases and runtime import code.
- Bootstrap-critical builtin type objects such as `classmethod`, `staticmethod`, and `property` must come from explicit runtime bootstrap intrinsics/primitives. Do not probe-construct Python objects in stdlib bootstrap code to discover their types.
- When modifying `builtins.py`, `sys.py`, `importlib`, `_intrinsics.py`, or frontend import lowering, add or update native end-to-end bootstrap regressions in the same change.
- If a bootstrap fix depends on control-flow behavior in a fast-moving frontend/backend file, factor that dependency into a first-class runtime/bootstrap contract instead of leaving another chicken-and-egg edge in place.

## Git Discipline

- **NEVER revert or discard unstaged changes**. They are from trusted partners. Pause and wait.
- **NEVER trample partner work**. If you encounter unfinished changes, work around them or wait.
- **Always `git add` immediately** after writing any file. Linter hooks can silently revert unstaged changes.
- **Atomic operations**: write file + git add in the same step using `&&` chaining.

## Crash Recovery Structural Stability Mode

When Codex, Claude, Desktop, WSL bridging, MCP/tool discovery, subagents,
process custody, or a guarded command has crashed, stalled, disappeared, or
been manually killed in the current session, stabilize the control plane without
shrinking the engineering ambition into tiny chips. Reduce concurrency, isolate
Molt-owned process scope, record evidence paths, and keep the unit of work a
complete structural primitive that deletes or unifies a real authority.

Recovery mode constrains process fanout, not engineering scope: one active
structural arc, one bounded proof lane, no retry storms, and no parallel proof
fanout. Subagents may map or migrate disjoint consumers inside that arc, but
they must not multiply status chatter or broaden cleanup scope.

Recovery discipline is process containment, not permission to chip away at the
project. A valid recovery landing removes a real source of drift, avoids
duplicate authority, and leaves no dangling legacy lane. Before risky commands,
leave a death capsule: command, cwd, guard pid, child pid when known, status,
timestamp, and evidence path. Prefer `tmp/memory_guard/active/`,
`tmp/memory_guard/incidents/`, pytest outer-guard summaries, and
`logs/agents/codex_stall/*.json`.

If a process disappears, inspect git status, active guard markers, incidents,
pytest outer guards, codex-stall records, and host-control-plane classification
before guessing. Manual killing of a Molt-owned child/helper must stay scoped to
that child; never broaden cleanup to Codex, Claude, app-server, renderer,
node-repl, ancestors, or unrelated host control-plane processes.

Only proved Molt-owned processes may ever be cleanup targets. A repo path,
process name, stale PID, or missing sampler identity is not enough authority to
signal a process. If live identity cannot prove a non-host Molt-owned target,
do not kill it; preserve evidence and fix custody first. Codex itself is never
a cleanup target.
Only Molt processes should ever be cleaned for Molt work. Do not clean, kill,
restart, rewrite, or repair Codex/Claude/app control-plane state as a side
effect of recovering a Molt command unless the operator explicitly asks for
Codex or Claude app repair.
Molt-owned means live command, sidecar, session, backend-daemon, guard, or
runtime-child identity for this repo's Molt build/test/bench/runtime work.
Codex, Claude, app-server, renderer, node-repl, MCP/plugin helpers, shell hosts,
Git pollers, and ancestor/control-plane processes are never Molt-owned just
because they reference the repo path or spawned a Molt child.
Codex app and Codex CLI are host control-plane processes on Windows, macOS, and
Linux. This includes `codex`, `codex.exe`, Electron/renderer helpers, app-server
helpers, shell hosts whose ancestry is Codex/Claude, and Codex Git polling
children. Never use `pkill`, `killall`, raw `kill -TERM/-KILL`, process-group
kills, `taskkill`, `Stop-Process`, name-based sweeps, stale PID files,
repo-path matches, or parent-chain cleanup against them while doing Molt work.
If Codex itself is wedged, preserve logs/state evidence and either continue
through a healthy lane or ask for explicit Codex app/CLI repair; do not make
Codex repair collateral damage of Molt cleanup.
The executable classifier authority for this boundary is
`tools/memory_guard_core/process_model.py`. Any cleanup, sentinel, guard, or
process-sampling change that touches Codex/Claude/app-server/node-repl identity
must update that classifier and the host-control matrix tests before changing
kill behavior. Do not add ad hoc name/path matching elsewhere.

## Tools — discover before you build (canonical registry)

**Before building ANY analysis / verification / perf / audit / gate tool, it almost
certainly EXISTS.** Read `tools/REGISTRY.md` first — the canonical, agent-facing catalog
of molt's highest-signal tools. Agents have re-authored the same tool twice (two
independent `perf_authority.py`; the float and int repr cuts built from opposite ends);
that waste is forbidden. Run `uv run --python 3.12 python3 tools/ci_gate.py` to run every
wired gate by default. A tool counts as real only when it is **wired** (run by ci_gate /
a workflow / the build pipeline), **fast** (under its tier wall-clock budget), and
**registered** in `tools/REGISTRY.md`; the `check_tool_registry` + `check_ci_walltime_budget`
gates fail CI otherwise — so a potent tool can never go undiscovered, unrun, slow-enough-to-mute,
or rebuilt.

## Build & Test

- Build with `cargo build --profile release-fast -p molt-backend --features native-backend`
- Test with `python3 -m molt build --target native --output /tmp/test_out test_file.py --rebuild`
- Backend daemon uses release-fast profile. Drain stale live-proved Molt build/test/bench
  workers through `molt clean --apply --kill-processes` or
  `python3 tools/process_sentinel.py --once --stale-orphan-sec 3600 --stale-pytest-sec 900`
  before testing new builds.
- Max 2 build-triggering agents at once. 5 concurrent builds OOM the machine.
- Max 3 backend daemons enforced by the CLI. Stale sockets are auto-cleaned.
- After a session with multiple agents, run `molt clean --apply --kill-processes`
  only when stale Molt-owned workers need draining, so process cleanup and
  artifact deletion stay inside the canonical guard and allowlist. It is never
  Codex, Claude, app-server, renderer, node-repl, shell, or Git cleanup.
- Cleanup commands must fail closed on ambiguous ownership: no blanket
  `taskkill`, no name-based Codex cleanup, no signaling a PID that cannot be
  reidentified as a live non-host Molt-owned worker.

## Safe Execution (Non-Negotiable: never OOM or hang the host)

**NEVER run a compiled molt binary (or any command that might infinite-loop /
allocate unboundedly) directly.** Raw binaries carry no memory guard, and the
harness memory guard only wraps `molt run`/`molt test`/`molt build` — not bare
`./binary` execution. A single runaway loop can take the host to tens of GB of
RSS (observed: 97GB before OOM-kill) and wedge the machine.

Always route direct binary execution — smoke tests, bisecting, profiling,
differential one-offs, repro reduction — through the shared memory guard or a
harness path that delegates to it. `tools/safe_run.py` is not a new authority
for process custody; use it only after it is known to route through the shared
guard stack for the current command family.

```bash
# Hard wall-time + RSS caps with protected host/control-plane filtering.
python3 tools/memory_guard.py --max-rss-gb 2 --max-total-rss-gb 3 --timeout 15 -- ./my_binary [args]
```

Rules:
- Bisecting a suspected hang/OOM: use `tools/memory_guard.py` with a small RSS
  budget and short timeout so a runaway dies promptly under custody evidence.
- Prefer `molt run`/`molt test` (guarded) over raw binaries whenever possible;
  use the shared guard for the cases where you must invoke the artifact
  directly.
- `gtimeout`/`perl -e 'alarm'` bound wall-time but NOT memory — they do not stop
  an OOM. Use the shared memory guard for anything that could allocate
  unboundedly.
- An infinite-loop / hang / OOM bug is the most severe class: fix it
  structurally and add a differential regression. Do not work around it by only
  capping the runner.

## Concurrent Development (MOLT_SESSION_ID)

`MOLT_SESSION_ID` **must be set BEFORE any build command**. Every agent must export it at the start of every shell command:

```bash
export MOLT_SESSION_ID="agent-1"  # MUST come before any molt or cargo command
```

**Molt developer build artifacts go on an external drive, never `C:`.** This is
a development self-protection rule for agents and local Molt maintainers, not a
user-facing compile contract. Real users may build in place, use Cargo defaults,
or pass their own target/output flags.

Use the DX resolver (`molt dx env`, `molt dx run`, `tools/dev.py`, or
`tools/run_context_env.py --prefer-external-artifacts`) before build/test/bench
work. On Windows checkouts on `C:`, the resolver fails closed unless it can
place artifacts on a healthy non-`C:` root such as `E:\Molt`; macOS/Linux use
the configured external candidates. The resolver owns `MOLT_EXT_ROOT`,
`CARGO_TARGET_DIR`, `MOLT_DIFF_CARGO_TARGET_DIR`, `MOLT_CACHE`, diff/tmp roots,
`UV_CACHE_DIR`, `UV_PROJECT_ENVIRONMENT`, `PIP_CACHE_DIR`,
`PYTHONPYCACHEPREFIX`, `TMPDIR`, `TMP`, and `TEMP`. Default Cargo output is
session-scoped as `$MOLT_EXT_ROOT/target/sessions/$MOLT_SESSION_ID`; explicit
`CARGO_TARGET_DIR` remains an operator-owned override.

Raw `cargo` commands do NOT honor `MOLT_SESSION_ID` by themselves. For any
direct cargo invocation, export the DX env first. If you bypass the DX env, keep
session isolation inside the external target root:

```bash
eval "$(python3 tools/run_context_env.py --prefer-external-artifacts --dx --format posix)"
export CARGO_TARGET_DIR="${MOLT_EXT_ROOT:?}/target/sessions/$MOLT_SESSION_ID"
```

This gives each session:
- **Its own cargo target directory** (`target/sessions/<id>/`) — no cargo lock contention, no artifact clobbering
- **Its own daemon socket** — no kill/restart conflicts between sessions
- **Its own build state and lock-check caches** — fully isolated build lifecycle
- **No `cargo clean`** — incremental builds only, no binary deletion

The first build in a new session takes approximately 5 minutes (full compile). Subsequent builds are incremental.

Without `MOLT_SESSION_ID`, all sessions share the selected `CARGO_TARGET_DIR`
(solo dev mode).

Agents **MUST** use `export MOLT_SESSION_ID="unique-name"` at the start of every command to ensure isolation.
