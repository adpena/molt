# Live Orchestration Board

Use `tools/agent_coordination.py context` for current Git, worktree, ownership,
and proof state, and `docs/ops/MULTI_AGENT_COORDINATION.md` for registration and
handoff. The active integrator owns reconciliation and landing. Register that
ownership; a role named in old prose is not a current lock.

This board holds the binding coordination protocol, the canonical host paths,
and the R0-R9 roadmap recipe. Current V1.0 burndown state, priorities, blockers
and next closure outcomes come only from `docs/agent/V1_HANDOFF_FINDINGS.md`.
`docs/agent/CLAIMS.md` and coordination records own their claim/worker custody;
their evidence feeds the ledger rather than a second V1 status board. Current
operator instructions determine concurrency.

On 2026-10-06 the dated status snapshots, pause handoffs, swarm burndowns, and
per-agent lane lists were deleted from this board. Git history keeps them.
Status notes that remain in the roadmap are dated; check them against live
source before reuse.

V1 burndown routing reviewed: 2026-10-28.

## Canonical paths (binding)

The primary development host is macOS (from 2026-10-06). The Windows x64
development and proof host is `bat00`, reached over Tailscale through its
configured SSH route. Dispatch independent development work and Windows
qualification there with exact source, toolchain and target custody while
other host lanes proceed.

`tools/cross_hosts.toml` is the shared host inventory for smoke execution and
the trunk proof runner. Its proof-command allow-set is host capability, not a
second proof policy. The selected commit's proof plan owns prerequisites and
execution arguments. See [farm proof operations](../OPERATIONS.md#farm-proofs-at-an-exact-commit)
and [the Stage 1 design](../design/trunk_loop.md) for exact-source detached
execution, archive seeding, receipt collection and recovery. The integrator owns
landing, findings-ledger proposals and retirement; Stage 1 does not install a
continuous service.

Both hosts use the same checkout-family layout. `src/molt/custody_layout.py` is
the one rule: a checkout at `<root>/molt-src` and every worktree at
`<root>/worktrees/<name>` resolve to the custody root `<root>`, which owns
build artifacts, toolchains (`<root>/target-root`), guard state, and scratch.
Automatically derived roots for nested worktrees are promoted beyond every
enclosing Git source checkout. Safe external wrappers keep their existing
addresses; explicit execution overrides retain their declared semantics.
`molt.dx` is the only reader of `MOLT_EXT_ROOT`: `artifact_root` gives the
artifact root, and `scratch_root`, `scratch_dir` and `control_state_dir` give
scratch, which never lands in a checkout. The structural audit rejects a new
`<root>/tmp` derivation or direct `MOLT_EXT_ROOT` read.

| purpose | macOS (primary) | Windows (development and proof host) |
|---|---|---|
| custody root | `~/Molt` (internal NVMe) | `C:\Molt` (internal NVMe) |
| checkout, git work, landings | `~/Molt/molt-src` (`~/Projects/molt` links to it) | `C:\Molt\molt-src` |
| lane worktrees | `~/Molt/worktrees/<lane>` | `C:\Molt\worktrees\<lane>` |
| build artifacts | `/Volumes/VertigoDataTier/Molt` (APFS SSD), set in `~/.zshenv` by `MOLT_EXTERNAL_ARTIFACT_ROOTS` and `MOLT_PREFER_EXTERNAL_ARTIFACTS=1`; the custody root while the SSD is absent or has under 20 GiB free | the custody root |
| Python commands | `uv run --python 3.12 ...` from the checkout | same |

Route builds through RunContext (`tools/run_context_env.py --prefer-external-artifacts
--dx`, `tools/dev.py`, or the proof queue). It resolves the artifact root, a
stable `CARGO_TARGET_DIR`, cache and temp roots, and `MOLT_TARGET_ROOT`. No
volume is ever selected by name, label, or free space: put build output on
another drive only by naming it in `MOLT_EXTERNAL_ARTIFACT_ROOTS`. A plain clone
outside the family layout is its own custody root: artifacts stay in the clone
(the Cargo norm) and scratch goes to a per-checkout folder under the host temp
root, never into the source tree.

Provision the pinned WASI SDK once per custody root with
`uv run --python 3.12 python tools/provision_wasi_sdk.py`; WebAssembly builds
then take every C tool from it, locally and in CI.

Forbidden on every host: a checkout, worktree, venv, or artifact root under
OneDrive (`src/molt/dx.py` rejects it). On Windows, `D:\Molt` and `E:\Molt`
(exFAT) must not hold canonical source inputs, package seals, worktrees,
toolchains, custody records, or landings. Agent worktrees are short-lived; land
their signal and delete them.

## Exit criteria (E1-E4)

The program is done only when all four hold. Release acceptance authorities:
`config/phase_exit_requirements.toml`, `tools/phase_exit_manifest.py`,
`tools/release_exit_gate.py`, and the findings ledger through
`tools/finding_status.py`. A release of version 1.0 or later cannot pass while
a ledger finding is open.

- **E1 · WITNESS GREEN.** `collab/pact/pact_witness_kernel/field_solve.py` (numpy + scipy.ndimage) → Molt **WASM** → `candidate_outputs.npz` → `check_parity.py` **PASS**. Zero fakes, zero host-CPython/Pyodide fallback, executable ABI dispatch only, all ecosystem behavior through real custody primitives.
- **E2 · PERF > CPython** on the claimed benchmarks: R3b/R4a numeric raw-lane + `spectral_norm` + the 54–67 portfolio, proven on `tools/perf_scoreboard`.
- **E3 · PARITY.** CPython ≥3.12 within the verified subset — R6 conformance shards + differential green.
- **E4 · STRUCTURAL FLOOR.** god-file/god-crate ratchet green; fail_closed poison classes at/under baseline and trending to zero (read the live registry, drive DOWN); effect-attestation live so no capability silently degrades; warm shared builds.

## Protocol (binding for every agent)

1. **OWN THE DELIVERABLE.** Finish authorized implementation, integration and
   retirement through the named owner. Bounded read-only reviews return evidence;
   workers have no commit, proof-run or landing quota. Do not manufacture work to
   end a turn, or stop an implementation while useful in-scope work remains.
2. **VERIFY THE CLAIM.** Exercise the affected contract and consumer family with
   the smallest sufficient independent check. State unverified cells. Broaden or
   repeat for changed inputs, new failures or named unresolved obligations;
   required integration gates and the release matrix remain mandatory. A focused
   development check does not imply release acceptance or demand an unrelated suite.
3. **BUILD HYGIENE.** Reconcile live upstream through
   `tools/molt_dev.py integrate --expected-upstream FULLSHA`, preserving existing
   commits with normal merges. Ownership claims forbid rebase/reset and direct
   pushes outside that driver. Do NOT set `MOLT_SESSION_ID` for ordinary `molt build`/`cargo build` (that opts back
   into a cold per-session target dir); leave it unset to reuse the persistent
   target under the host's artifact root (see Canonical paths). Set it ONLY for
   perf/bench/test-shard isolation. Do NOT hand-clean artifacts; use the owning
   consumer's custody policy.
4. **PROFILE BEFORE OPTIMIZING.** State the hot path + Big-O and attest a
   before/after delta for any perf/build change (tools/dx_build_timer.py,
   tools/build_graph_audit.py). No optimizing by feel.
5. **DRIFT DISCIPLINE.** Land each complete change and retire its worktree when
   the owner confirms its work is preserved. Use
   `python tools/drift_harvest.py --report` for explicit maintenance; reconcile
   actual unlanded changes before any requested prune. Preserve active and
   unrelated WIP. Worktree age, counts,
   and other checkouts' state do not block an independently complete landing.
   Install managed hooks with **`python tools/install_git_hooks.py`**. The installer
   retains the commit-attribution hook and retires the obsolete Molt pre-push
   hook, restoring preserved foreign hooks; `--check` verifies this state. It
   uses the common Git hooks directory, without enabling `core.hooksPath` or the
   unrelated pre-commit type-check. `tools/molt_dev.py integrate` runs the selected canonical gates in an
   exclusive clean checkout and publishes the captured commit. Canonical CI
   checks the same generated projections. Explicit proof-plan tiers remain available. Molt installs no
   global hygiene or live-working-tree generator gate at push time.
6. **REVIEW FINDINGS ARE LANES.** The 2026-07-08 full-stack adversarial review
   recorded **26 CONFIRMED** findings (bug classes / metabugs / optimizations), each
   independently refuted-then-survived, in
   [REVIEW_FINDINGS_20260708.md](REVIEW_FINDINGS_20260708.md) with per-finding
   lane assignments + fix directions. Claim a finding via CLAIMS.md and own it
   end-to-end (fix + owning proof + land + retire). Do NOT freelance
   outside your claimed lane.
7. **CRASH RESILIENCE.** In-process subagents die with the harness and lose
   their in-process state. Commit incrementally by exact pathspec, keep durable
   state in the coordination records, claims, and proof queue, and give every
   long proof a detached owner.

## Operator-delegated orchestration authority (binding)

The human operator delegates lane assignment, landing sequence, conflict
resolution and stand-down decisions to the orchestrator. Explicit human direction
and current coordination custody govern; stale board prose cannot grant ownership.
Read relevant live assignments when starting work or when custody changes, without
rereading the entire board before each commit.

1. Edit only assigned paths recorded in the board, claim or coordination record.
   A worktree name or an announced intention is not an ownership grant.
2. A reclaim or stand-down stops competing work immediately. Preserve WIP and
   evidence for its integration owner; do not discard it or land over that owner.
3. Before each commit, assert checkout/branch/HEAD, ownership and the exact staged
   path set. Review the scoped diff; never broadly stage unrelated work.
4. Preserve parallel work. Never force-push, reset shared refs, overwrite another
   lane's WIP or drop its commits. A publication conflict returns to the integrator.
5. Raise disputed assignments with evidence through existing coordination custody;
   workers do not override or silently expand them. The orchestrator resolves the
   assignment and may explicitly expand a complete migration's scope.

## Attest every optimization (binding)

"Landed" and "effective" are separate claims. Performance/capability claims need
representative measurements and evidence that the selected path actually ran
(for example cache hits, raw-lane use or worker utilization). Use existing counters
and receipts; add instrumentation only when needed to decide the change. A
configured path or silently skipped benchmark proves no effect. Rank work by user
impact, release dependencies and measured critical-path cost, including emitted-code
performance and development throughput.

Example: sccache stayed off for months while "configured". A new silent
degrade-to-slow path cannot land: `tools/degrade_to_slow_gate.py` enforces this
class.

The coordination toolchain is complete: tree_drift_check + molt_dev integrate +
claim_lane + dirty_tree_landing_audit cover
detection / safe-landing / solo-claims / dirty-replay-coverage. Spend cycles on
the goal (E1-E4) and the leverage frontiers above, not more meta-tooling.

## Destructive git is banned on the shared checkout (git_guard)

A `git reset --hard` in a cleanup one-liner once destroyed another lane's
uncommitted work beyond recovery (2026-07-03). The rule is now a mechanism.

**MECHANISM (not just a rule): `tools/git_guard.py` is now landed and MANDATORY
for the shared checkout.** Destructive working-tree git — `reset --hard`,
`checkout -- <path>` / `checkout -f`, `clean -fd`, `stash drop/clear/pop`,
`branch -D`, `gc --prune=now` — is BANNED on the shared main checkout. Use it
only inside an ISOLATED worktree or plumbing-index mode (`GIT_INDEX_FILE`).
- Need a clean tree for a build/cherry-pick trial → `git worktree add`, never the
  shared checkout.
- Route any unavoidable destructive op through `python tools/git_guard.py run --
  <git args>` (refuses on the shared checkout, snapshots first).
- An always-on recovery net (`git_guard.py watch`) snapshots WT+index to
  `refs/wip-guard/*`; recover via `git_guard.py list` + `git stash apply <sha>`
  in a worktree. This is defense-in-depth, NOT a license to run destructive git.

## God-file and god-crate decomposition (operator P0)

**OPERATOR P0 DIRECTIVE 2026-07-03 — GOD-FILE/GOD-CRATE DECOMPOSITION (TOP
PRIORITY, co-equal with the witness — under operator-delegated orchestration authority
above).** The live ratchet is `tools/structural_audit.py`. Operator's words,
binding and quoted: *"break up all god files and crates as a P0 priority
because that is a dev velocity murderer that is intolerable and unacceptable
and offensive."* This is the #1 standing
structural obligation until the god-crate is gone. Schedule complete decomposition
outcomes against the live release critical path; unrelated arcs need no extra move.

DISCIPLINE (binding): (a) `tools/structural_audit.py` FIRST — ratchet only moves
DOWN; (b) STRICT move-only diff, pure renames, widen `pub` PRECISELY (never
blanket `pub(crate)→pub`), gate on a byte-identical corpus build + 0-warning +
lib tests + symbol identity (21f specs); (c) ISOLATED worktree, commit small
per-move by EXACT pathspec, ping the integrator to cherry-pick; (d) new crates
born UNGATED — register the per-crate clippy gate in `tools/proof_plan.toml` and
regenerate (`tools/gen_proof_plan.py --write`) in the SAME move; (e) generated files
(`wasm_abi_generated/**`, `intrinsics/generated.rs`, `op_kinds_generated.rs`, `import_metadata.rs`) OWNED
BY THEIR GENERATORS — never hand-split; fix the generator/authority. This is
R5b: the permanent fix for the ~2160s god-crate wasm rebuild.

GATES (every cut, non-negotiable): build-verified (leaf `cargo check` + `clippy
-D warnings` + queued god-crate check); `tools/canonicalization_contract.py
--check` green; `tools/structural_audit.py --check` must IMPROVE — NEVER
`--update-baseline` to hide debt; STRICT move-only diffs; exact-pathspec commits;
ISOLATED worktrees off current origin/main (or session base, then the
integrator reconciles); the integrator cherry-picks. `git worktree list` BEFORE
claiming — lanes hold uncommitted WIP, and lanes have collided this way.

## Canonical crate naming (operator directive: standardize like Lattner)

One convention, mirroring the CPython layer axis, replacing the inconsistent mix
of `molt-runtime-*` / `molt-lang-*` / `molt-*`:
- **Core / primitives**: `molt-object` (the `MoltObject` NaN-box value repr;
  currently pkg `molt-lang-obj-model`, dir `molt-obj-model` — drop the `-lang-`
  prefix, make package == dir). The object protocol (ops), when extracted, joins
  it or becomes `molt-object-protocol`.
- **Runtime API surface**: the crate currently MISNAMED `molt-runtime-core` is
  NOT core — it is the thin re-export/API-surface subcrates depend on. Rename to
  `molt-runtime-api` (honest name; a wrapper that masquerades as core is a
  canonicalization defect).
- **Stdlib**: `molt-stdlib-<mod>` for every stdlib module crate. Rename the ~19
  `molt-runtime-{crypto,tk,math,path,collections,regex,itertools,serial,difflib,
  logging,http,stringprep,xml,ipaddress,zoneinfo,net,asyncio,compression,text}`
  → `molt-stdlib-{…}`. This makes the stdlib layer legible at a glance.
- **Third-party / extensions**: `molt-cpython-abi` (drop `molt-lang-` package
  prefix; package == dir).
- **Backends / IR / passes**: already consistent (`molt-backend-*`, `molt-ir`,
  `molt-tir`, `molt-passes`) — leave.
SEQUENCING: a crate rename is build-breaking and touches every Cargo.toml + `use`
path, so it is an ATOMIC sweep per crate (or a tight batch) in an ISOLATED
worktree, gated on a full `cargo build` + `check_rustfmt --changed` + the
per-crate clippy gate, then cherry-picked. Do renames when the touched crate has
no other in-flight lane (check CLAIMS.md). Do NOT interleave a rename
with a semantic change in the same commit — rename-only diffs must stay reviewable.

Status 2026-10-06: `molt-stdlib-{difflib,graphlib,text}` are renamed. The
other `molt-runtime-*` stdlib crates, `molt-runtime-core`, `molt-obj-model`, and
the `molt-lang-` package prefixes remain.

## Target features (binding)

Any target feature, capability flag, or browser-host import MUST be declared in
`src/molt/target_feature_manifest.toml` and regenerated through its generator.
NEVER hand-add a feature literal, a second capability list, or a backend-local
reclassification.

## Apparatus track

Plan: `docs/design/foundation/72_interpretable_self_improving_apparatus.md`. The
control plane (proof queue, gates, memory, board) must be interpretable by
construction and improve itself from evidence; it serves compiler outcomes and
never displaces them. `proof_queue.py`, `structural_audit.py`,
`canonicalization_contract.py`, and `apparatus_ledger.py` are hot shared tools:
claim the lane before you edit them.

## Drift-resolution protocol (binding — the shared-checkout is the bottleneck)

The shared checkout accumulates multiple hands' uncommitted WIP, which blocks
`merge origin/main` and causes stale-base builds. Discipline to keep velocity:

- **Commit verified work in SMALL, DISJOINT commits, promptly.** The moment a
  proof row confirms your lane's change compiles/passes, commit ONLY your
  files by exact pathspec. Do not accumulate a large dirty tree — it drifts
  and blocks everyone.
- **Run an ownership audit before committing** (e.g. grep for a lane-marker
  like `capi_trace` reference count) to prove a file is yours, not another
  lane's WIP. Never bundle another lane's uncommitted files.
- **Never stash/overwrite another hand's WIP to force your branch forward.**
  If you can't push (non-fast-forward) because the shared tree is dirty, DEFER
  the push and tell the orchestrator. Preserving parallel work overrides
  tidiness.
- **Orchestrator lands via cherry-pick-in-isolated-worktree.** To land a
  disjoint commit onto origin/main without disturbing the shared dirty tree:
  `git worktree add --detach <path> origin/main; git -C <path>
  cherry-pick <sha>; git -C <path> push origin HEAD:main`. Verify the base
  delta doesn't touch the commit's crate (`git log <oldbase>..origin/main --
  <crate>`) so the author's compile-check transfers — no rebuild. This is how
  0f955df37 landed cleanly while the shared tree stayed dirty.
- **Prefer per-lane worktrees for NEW build-heavy lanes** so the shared
  checkout stays clean: `tools/molt_dev.py lane start <slug>` creates,
  records and provisions one, and `lane retire <slug>` removes it after it
  lands (docs/ops/INTEGRATION.md).

## 1000-Year End-State Roadmap (R0–R9) — the recipe

This is the full path from HERE to the end state. Every item names its
ingredients (files/authorities), its process (commands), and its acceptance
evidence. No item may land as a partial: each is a complete subsystem cut.
Dependency edges are explicit; anything not blocked may proceed in parallel
subject to lane ownership. The overriding outcome bar, in order:
(1) correctness incl. memory safety, (2) faster than CPython everywhere
claimed — approaching/beating Codon and PyPy on numeric kernels,
(3) CPython >=3.12 parity with version+platform gating, (4) deterministic
small fast-start artifacts, (5) world-class agent-first DX.

Owner tags were removed on 2026-10-06; ownership comes from CLAIMS.md and
the coordination records. Status notes inside items are dated history.

### R0. Pact witness kernel GREEN end-to-end

Current frontier state: only `V1_HANDOFF_FINDINGS.md` row HF-04.
`E1_REMAINING_FRONTIER_MAP.md` is dated historical reconnaissance, to reconcile
against that row and current source before using its observations.

The R0 done criterion: `field_solve.py` from
`collab/pact/` compiles through the live WASM/browser path, produces
`candidate_outputs.npz`, and `check_parity.py` passes — no host-CPython
fallback, no fake symbols, upstream source only through package custody.

- R0.1 `_multiarray_umath` static init failure + propagation wedge.
  Ingredients: `runtime/molt-runtime/src/builtins/module_table.rs` (module
  states {Uninit, Initializing, Ready, Tombstone, Replaced};
  `molt_module_ensure` is the ONLY transition owner), static extension init
  path, `static_extension_init_failure.json` dossier emitted by the
  acceptance lane. Process: `bash tools/witness_cycle.sh` (build|run|cycle)
  with `MOLT_TRACE_IMPORT_STAGE=1`; read the dossier BEFORE any manual
  rummaging. Two defects to close as one arc: (a) the init failure itself
  (whatever C-API/ABI symbol or capsule the module needs — close it as a
  reusable primitive, never a stub); (b) init failure must propagate as a
  Python ImportError and unwind — a wedge/hang on the error path is a
  module-state custody bug (Initializing never resolved). Acceptance:
  `alias_probe.py` prints its numpy/scipy census and `WITNESS-CHAIN-OK`.
- R0.2 numpy.linalg closure (eigh chain). The built artifact
  `_umath_linalg.molt.wasm` is parked at `tmp/pact_staging_parked/`;
  restage into the numpy seal `numpy/linalg/`, rebuild, prove
  `numpy.linalg.eigh` executes. Depends: R0.1.
- R0.3 scipy.ndimage executable dispatch: `distance_transform_edt`,
  `gaussian_filter`, `label` native callable_exports must be executable ABI
  dispatch (not import-visible-only). Ingredients: callable-table slots
  (`module_abi/callable_table/layout.rs` slot-addressed builder), app
  callable resolver, `_nd_image.molt.wasm` manifest callable_exports.
  Acceptance: alias_probe's EDT/gaussian/label chain returns correct
  values. Depends: R0.1.
  RESOLVED 2026-07-03 on origin/main by 93f2eb5d9: the from-import form
  `from nativepkg.ndimage import distance_transform_edt;
  distance_transform_edt(x)` now lowers to `invoke_ffi` when the import binding
  is live. Conditional/evicted imports still route through `module_get_global`
  and `call_bind`, preserving CPython LOAD_GLOBAL semantics. Evidence:
  `tests/test_frontend_ir_alias_ops.py` passed 33/33 and pins both paths.
- R0.4 Acceptance lane: `uv run --active --project . --python 3.12 python
  tools/proof_queue.py pact-witness-acceptance --target wasm --detach --timeout 7200`.
  Evidence: run ID, `candidate_outputs.npz` produced by Molt WASM,
  `check_parity.py` PASS. Depends: R0.1–R0.3.
- R0.5 Witness performance: time the kernel vs CPython (same inputs);
  faster-than-CPython is part of DONE, and the number goes on the R8
  scoreboard. Depends: R0.4.

### R1. Native call-lane unification

End state: ONE call-target authority. Trampoline vs fixed-arity direct
dispatch is a single registry decision; the borrowed-vs-consumed argument
ownership contract is written in exactly one place and both lowering and
runtime read it. No callsite may resolve a function's direct target where
the trampoline target is required (the P0), and no borrowed name string may
be dec_ref'd by the callee. Remaining known layers after the current WIP
lands: dec_ref-of-borrowed-name; "SystemError: module id out of range".
Gates: `tests/test_native_import_bootstrap_regressions.py`, a synthetic
compile_func indirect-call test (E2E is release/WIP-brittle; the synthetic
test is the durable gate), differential `python tests/molt_diff.py ... --jobs 1`.

### R2. Import bedrock completion + FREEZE

Per `docs/design/foundation/import_bedrock_frozen_module_layer.md`.
PR1 (generated ModuleRegistry + runtime ModuleTable) is LIVE.
- PR2: sys.modules becomes a dict VIEW over the one module store; DELETE the
  Rust mirror sync (task #14). Blocked on: R1 landing (modules.rs quiet).
- PR3: wasm import/export/callable tables become REGISTRY PROJECTIONS
  (generated from the same authority; `module_abi/**` is reserved for this).
- FREEZE: wire the design's 11 invariant gates into CI; add the freeze
  contract to AGENTS.md ("the import/bootstrap layer changes
  only by amending doc 69 first"); then this layer is bedrock — no
  incremental patches ever again.

### R3. Numeric raw-lane keystone

The single highest-leverage perf arc: molt currently BOXES loop arithmetic
(every int/float op = NaN-box runtime call + refcount). CheckedMul peel is
LANDED (346e3563b) and is the pattern to generalize.
- R3a `molt-check` TIR translation validator: Repr may only move UP the
  lattice; built on `runtime/molt-passes/src/representation_facts.rs` +
  `typed_repr_report.rs`. This is the drift gate that catches silent-OOB
  class bugs (GAP-3) at IR level. Adapt the existing fuzz_tir_passes
  infrastructure; do NOT greenfield. (The old egg prototype
  egraph_simplify.rs rewrote a toy language, not TIR; HF-281 deleted it.)
- R3b Loop-body int/float RAW-LANE specialization: native
  iadd/imul/fadd/fdiv in loop bodies with box/unbox hoisted to loop
  boundaries. Ingredients: `runtime/molt-passes/src/tir/scalar_carriers.rs`,
  `value_range.rs`, the CheckedMul lowering
  (Cranelift `smulhi`, 64-bit-exact flag; Luau conservative; WASM boxed
  until R4a). Carrier disagreements between value_range/arith_division/
  scalar_carriers are P0 silent-wrong-answer bugs (the loop-IV modulo class)
  — one carrier authority, gated by R3a.
- R3c Dynamic-IV bounds-check elimination (GAP-3): UNBLOCKED ONLY after R3a
  can prove the widening safe (silent OOB risk if widened wrong).
- Acceptance: spectral_norm and numeric cluster A GREEN vs CPython
  (`python -m molt build --release`, differential harness serial), then the
  same kernels timed vs Codon and PyPy for the R8 scoreboard.

### R4. Full WASM + WebGPU lowering (binding 1000-year directive)

No boxed fallbacks on proven-typed hot paths; lowering into NATIVE
instructions and symbols.
- R4a Numeric ops lower to native wasm instructions (i64.add, f64.mul, ...)
  driven by the generated op_kinds authority; delete the boxed runtime-call
  lane for proven-typed ops in the same arc. Depends: R3b (shared Repr facts).
- R4b simd128 for vectorizable kernels (the wasm feature is already in the
  target contract; lowering must actually emit v128 ops).
- R4c WebGPU: `molt.gpu` (the tinygrad custody shim's target) lowers to real
  WGSL/WebGPU dispatch. No stubs; if a kernel class isn't supported it
  fails closed with a precise diagnostic.
- R4d Browser embed API per
  `docs/spec/areas/wasm/0970_BROWSER_NUMERIC_KERNEL_EMBED.md`.
- Standing rule: every runtime-visible WASM op keeps the synced triple
  (ABI import + op_loop handler + #[no_mangle] export); gate
  `test_wasm_runtime_export_no_mangle.py`; validate E2E with
  `--target wasm --linked` + molt_diff native,wasm.

### R5. Iteration-loop velocity

The compiler team's own loop is a first-class perf target. Budget: a
one-file edit reproves in <30s native / <60s wasm-link on a warm dir.
- R5a Extract `cpython_abi_hooks` crate per
  `docs/design/foundation/70_molt_runtime_crate_extraction.md` (measured
  47x: 282s→6s). Follow the doc exactly: pure move, precise pub widening,
  per-crate clippy gate registered in `tools/proof_plan.toml`. Sequence
  AFTER R1/R2-PR2 quiet modules.rs churn.
- R5b Further molt-runtime splits (same doc, same discipline), then the
  21_decomposition_program T1 `molt-tir` extraction (~100k-line midend,
  zero tir→backend edges) in a BACKEND-QUIESCENT window.
- R5c Frontend: finish the profiling arc (task #13); produce the ranked
  hot-pass table; lower the top passes to Rust one at a time, each with a
  differential gate proving identical output on the conformance corpus.
- R5d Toolchain config authority: wasm-opt/binaryen + zig + rustup target
  preflights all resolve through checked-in contracts (rust-toolchain.toml,
  `find_wasm_opt()`); any new tool follows the same pattern — pin → PATH →
  MOLT_TARGET_ROOT/toolchains discovery, never ad-hoc.

### R6. CPython >=3.12 parity floor

Version-gated semantics keyed on the TargetPythonVersion authority (never
silent single-version assumptions); Windows/macOS/Linux with explicit
platform gating; all within the verified subset with honest-early
fail-closed diagnostics outside it. Process: conformance shards through the
proof queue; differential harness `--jobs 1`. Every parity fix lands with
its version/platform gate expressed, not hardcoded.

### R7. Ecosystem custody generalization

Turn the numpy/scipy witness machinery into THE reusable primitives:
- `molt extension produce-set` is the canonical multi-extension package
  producer: one verified source commit, one upstream Meson setup, exact typed
  module/target/export ownership, real installed Python closure, deterministic
  per-artifact custody, and one rollback-safe atomic package seal. The Pact
  SciPy set must use this root exclusively; no union of historical per-module
  roots or package-specific config/closure/source-plan adapters.
- `molt extension build` (meson intro-targets + compile_commands source
  plans; zig as the wasm C++ toolchain; PyMODINIT_FUNC extern "C") is the
  one path for source-recompiled extensions — generalize beyond the
  vendored-meson fork specifics.
- Sealed-root curation (canary pruning, generated-file materialization,
  module-exec-level AST import closure) becomes a tool with a manifest, not
  a hand process.
- ndarray/tensor dtype/shape/stride ownership, buffer protocol, capsules,
  module state, extension object closure: each a shared primitive with one
  storage home. Missing C-API/ABI symbols close as primitives or fail
  closed with precise diagnostics — never per-package hacks.
- Reachability redesign ("Fact B": compute_intrinsic_manifest as the
  authority) kills the gratuitous-heavy-import class; lazy-gating imports
  requires this first (molt is AOT — no on-demand link).

### R8. Scoreboards + release gates

Per docs/design/foundation 54–67: perf scoreboards run quiescent and
classified, one row per benchmark/profile/target vs CPython AND Codon AND
PyPy; binary-size, startup, and throughput ratchets that only tighten.
A claimed support without a green scoreboard row is not claimed support.

### R9. Polish to freeze (at the end of each arc — not a phase to defer)

God-file ratchet back to green by DECOMPOSITION (cli.py ~41k,
function_compiler.rs ~28k, frontend/__init__.py ~27k) — never re-pinned.
duplicate_authorities stays 0. Docs (CANONICALS/INDEX/spec/STATUS/ROADMAP)
move in the same arc as semantics. Final recursive adversarial senior
review before any layer is declared frozen.

### Dependency spine (what blocks what)

```
R0.1 ──> R0.2, R0.3 ──> R0.4 ──> R0.5
R1 ──> R2-PR2 ──> R2-FREEZE ──> R5a (modules.rs quiet)
R3a ──> R3c;  R3b ──> R4a
R7 reachability ──> any import lazy-gating
Everything else: parallel, lane-owned.
```

## Agent tooling and Windows script rules

- Extend existing tooling for recurring measured friction when saved time exceeds
  maintenance cost. A repeated multi-line invocation alone does not require a new
  script. Prefer a compact verdict (rc + stage + first error) and a log path.
  `tools/witness_cycle.sh [entry] [build|run|cycle]` is the pattern.
- Windows/MSYS rule (incident 2026-07-02, hours lost): bash scripts that
  export paths into env vars MUST convert through `cygpath -m` — MSYS
  converts command arguments but NOT custom env vars, and Windows Python
  cannot resolve `/c/...`. The build now fails closed naming any missing
  MOLT_MODULE_ROOTS entry; if you see that diagnostic, fix your script's
  path style.

## Proof and cargo DX rules (binding — incident: 835s cold compile for one test)

- **DO NOT iterate on a full witness/wasm rebuild.** Editing `molt-cpython-abi`
  forces the `molt-runtime` god-crate (~230k lines) to recompile to wasm every
  cycle (~1700s+ per gap; the pact-witness-acceptance E2E lane is ~1500s). That
  is NOT a dev loop. `molt-cpython-abi` has NO dependency on `molt-runtime`, so
  `cargo test -p molt-cpython-abi` compiles ONLY cpython-abi (+ its deps) —
  seconds-to-low-minutes, no god-crate rebuild. Close every CPython C-API
  primitive (PyType_Ready slot inheritance, PyCFunction_NewEx, module exec
  slots, buffer descriptors, number/mapping protocol) behind a
  `runtime/molt-cpython-abi/tests/*.rs` unit test with stub hooks and iterate
  there. Reserve a wasm rebuild for BATCH integration confirmation only.
- **Batch C-extension-init closure via a full trace, not one-gap-per-build.**
  One instrumented wasm build with `capi_trace.rs` (MOLT_TRACE_CAPI) captures
  the ENTIRE C-API call sequence a numpy/scipy extension exec makes up to its
  failure. From that + the extension source, close ALL the needed primitives in
  the fast unit-test loop, then ONE wasm build to confirm the whole batch
  advanced. Target ≤2-3 wasm builds to close an exec, not 20.
- For behavior-only confirmation builds (does the import succeed?), use the
  fastest profile that reproduces it (dev-fast: lto=off, codegen-units=256,
  incremental) — NOT release-fast. Perf gates are separate.
- The molt-runtime god-crate rebuild cost is the structural root; the finer-
  crate extraction (roadmap R5b / decomposition T1) removes it permanently.
- Compile once per compatible profile/features/target configuration and reuse that
  producer for affected selectors. A cold compile can be necessary to exercise the
  actual changed boundary. Include other affected tests when their marginal cost
  is useful; do not add an unrelated shard to amortize a sunk compile cost.
- Reuse the stable checkout target for bounded development checks through
  `molt dx run`. Registered queue commands declaring `cargo-workspace` reuse
  their own exclusively leased target after terminal acknowledgement. Unknown
  direct commands and exact installed/release producers retain isolated output;
  warming another target does not warm those outputs. See PROOF_QUEUE.md for
  the execution-receipt versus source-exact producer distinction.
- Set an explicit `--timeout` matched to warm-compile reality; if a row is
  projected to blow it on a cold compile, re-shape, don't wait it out.
- Detach long proofs under their existing owner and advance independent in-scope
  work while they run. When dependencies leave no useful work, wait for completion;
  do not open speculative lanes to stay busy or repeatedly tail unchanged logs.
- Batch affected tests sharing a producer and custody into one proof row.
- Env for local iteration: `MOLT_MEMORY_GUARD_POLL_SEC=2.0`.
- When a row's time is dominated by compile, file ONE queue note naming the
  crate and move on.

## Conduct standards (binding)

- **Lane ownership is exclusive.** Check claims and coordination before editing.
  Read-only inspection does not acquire write ownership.
- **Evidence beats vigil.** Use terminal notifications or bounded waits; back off
  unchanged status reads. The parent may inspect supervised work. Do not create
  another deliverable merely to fill a dependency wait.
- **Check drift at decision boundaries.** At lane admission, inspect current
  published source and preserved WIP for an existing or superseded solution.
  Refresh when upstream changes affect the decision; the integration driver fetches
  and validates its exact upstream before qualification and publication. Local
  commits require source/ownership checks, not a network fetch or full witness
  sweep each time. Repair stale instructions against live evidence through the
  owner. `tools/tree_drift_check.py` remains available for explicit drift diagnosis.
- **Diagnosis is time-boxed.** 15 minutes per fault to a hypothesis with a
  bounded experiment; builds go detached while you work elsewhere. Never
  re-run a failed shape unchanged.
- **No unbounded filesystem scans, ever.** Derive exact paths from the
  pytest log, queue log, or artifact manifest.
- **Process spelunking is capped at one snapshot per incident.**
- **Write down what you learned the moment you learn it** (queue note or
  commit message). A finding that lives only in your context is a finding
  the team loses.
- **Classify apparatus failures.** Preserve the row ID and cause; repair recurring
  or blocking defects through the existing owner when expected savings justify it.
  A second inconvenience alone does not redirect a release lane into new tooling.
- **Side worktrees for runtime/backend edits.** The shared checkout's cargo
  state is everyone's build cache.
- **NEVER use `git stash` on this shared repo.** The stash stack is SHARED across
  all worktrees (`.git/refs/stash` is common): a `stash pop` in one worktree can
  race-apply and silently DROP another lane's stash, and can contaminate a clean
  worktree with a foreign/partial diff (validated 2026-07-07: a shared `stash pop`
  dropped a vfs-lane stash — recovered — and gutted a clean worktree's `locks.rs`
  to a 5-line truncation). Bank WIP to a `wip/*` branch (`git branch`/push), never
  `git stash`. If you find a contaminated file you didn't edit, preserve it to a
  patch and flag the orchestrator — do not commit it.
- Never revert or checkout files outside your lane, even transiently. A
  file you didn't edit that shows up dirty is another lane's live WIP.

## Working agreement (binding)

- Keep the shared tree compile-green: `cargo check` touched crates before
  any pause longer than a few minutes.
- Regenerate generated files in the same edit as their consumers; never
  leave a consumer referencing a symbol its generated file lacks.
- Commit with pathspecs only, options BEFORE the `--`: `git commit -m MSG --
  <files>`; never `git add -A`; never sweep another lane's dirty files. NOTE:
  `git commit -- <files> -m MSG` silently treats `-m MSG` as a pathspec, so the
  commit never happens — a real, easy-to-miss footgun. Keep `-m`/`-F` before `--`.
- Land small and complete: one coherent arc per commit, replaced code
  deleted in the same commit, tests with teeth (proven to fail on
  violation).
- `main` is protected by a GitHub ruleset: no force-push, no deletion. History
  was rewritten once (2026-10-06) to remove Claude attribution; translate an old
  commit ID with `docs/agent/history-rewrite-2026-10-06.commit-map`. Commit
  messages never carry Claude/AI attribution (hook + CI enforce it).
- Land through `python tools/molt_dev.py integrate --repo <clean-checkout>`.
  The publisher pins the post-rebase commit and upstream, reads that commit's
  canonical proof policy, runs all selected gates, rejects source or HEAD drift,
  and pushes that exact identity as a fast-forward. Preserve parallel WIP in its
  existing checkout; use an exclusive clean checkout for integration. Claims
  supply `--expected-upstream <full-commit>` so a competing claim cannot be
  silently rebased away. Foreign Git hooks remain active. Nothing-to-land is a
  successful no-op; no gate-skipping landing entry point is provided.
- Run the gates you touched before landing; cite queue run IDs as evidence.
- Compatibility floor: CPython >= 3.12 parity with explicit VERSION GATING
  keyed on the TargetPythonVersion authority, and Windows/macOS/Linux with
  explicit PLATFORM GATING — all within the verified subset, with
  honest-early fail-closed diagnostics outside it.
