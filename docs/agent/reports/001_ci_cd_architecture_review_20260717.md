# 001 - CI/CD architecture review

Date: 2026-07-17

Scope: GitHub Actions, local hooks, tools/molt_dev_gates.toml, tools/ci_gate.py,
change classification, proof tooling, performance, formal verification,
sanitizers, security, portability, and release publication.

## Executive verdict

Molt has many individually valuable checks, but it does not currently have a
trustworthy CI system. It has three structural failures:

1. Main is unprotected. GitHub reports no branch protection and no ruleset for
   main, so every red check is advisory after broken code is already canonical.
2. Several green or named-authoritative jobs do not prove what their names say.
   Kani succeeds while skipping every proof; determinism and Tier-3 failures are
   intentionally swallowed; the canonical perf authority has not completed.
3. The same proof policy is hand-maintained in four places: workflow YAML,
   tools/ci_changed_paths.py, tools/molt_dev_gates.toml, and tools/ci_gate.py.
   The duplication causes stale path filters, redundant execution, and local/CI
   disagreement.

The correct end state is not fewer proofs. It is one proof-plan authority that
selects the smallest complete proof family for a change, runs fast checks before
merge, moves controlled expensive work to nightly/release lanes, and makes every
named result honest.

## Quantitative snapshot

Evidence was collected from origin/main and GitHub Actions on 2026-07-17. The
latest 200 workflow runs span only 25.6 hours (2026-07-16 20:10Z through
2026-07-17 21:46Z).

| Workflow | Recent runs | Success | Failure | Cancelled | Median run wall time | Observed run-wall hours |
|---|---:|---:|---:|---:|---:|---:|
| Perf Gate | 39 | 0 | 0 | 37 | 22.8 min | 21.9 h |
| Molt WASM CI | 35 | 0 | 24 | 10 | 8.7 min | 4.4 h |
| CI | 39 | 0 | 29 | 9 | 7.5 min | 4.2 h |
| Security Hardening | 39 | 1 | 37 | 0 | 1.3 min | 1.0 h |
| Kani verification | 39 | 38 | 0 | 0 | 1.2 min | 0.8 h |

These run-wall totals undercount matrix runner consumption. The latest eleven
completed Perf Gate runs created 22 backend jobs and consumed 19.3 runner-hours;
every job was cancelled. The latest nineteen completed CI runs consumed 184.7
runner-minutes. Every signal-bearing CI job had zero successes:

| CI job | Runs | Success | Failure | Cancelled | Median |
|---|---:|---:|---:|---:|---:|
| Docs Gates | 19 | 0 | 19 | 0 | 19 s |
| Python Tooling Smoke | 19 | 0 | 19 | 0 | 55 s |
| Rust Build And Unit Smoke | 19 | 0 | 17 | 2 | 180 s |
| LLVM Backend | 19 | 0 | 15 | 4 | 355 s |

The failures are useful defects, not runner flukes, but persistent red makes the
system unable to distinguish a new regression from the existing baseline. Run
29615445350, for example, immediately found Python type errors, two stale target-
root tests, a broken full-feature runtime build, and a broken LLVM build. The
problem is that those defects were detected after the commit reached unprotected
main and then masked all downstream checks.

Run 29616350435 demonstrates the masking directly. Once the earlier ty failures
were repaired, Docs Gates advanced to the next sequential step and found that
COVERAGE_INDEX still named deleted runpy_basic.py. Commit 3a44101e4 had replaced
that legacy case with the stronger runpy_run_path_basic.py family but did not
move the coverage row or regenerate the lane manifest. The gate was correct; its
signal had simply been hidden behind the earlier fail-fast boundary. The source
row and generated manifest were repaired together in 49c7861a8.

Additional observed topology:

- The latest 14 completed WASM jobs spent 112.4 runner-minutes: 11 failed at
  Build Molt WASM host and three were cancelled. No later runtime/parity step ran.
- The latest ten weekly Nightly runs all failed; the latest run consumed the full
  60-minute differential timeout. The latest six Sanitizer runs all failed.
- Run 29615445319 was reported successful after Kani 0.67 installed its Rust
  1.93 toolchain, detected Molt requires Rust 1.96.1, and skipped both proof
  steps. The workflow configuration continues to classify this as success.
- bench/scoreboard/quiet_native.json was generated 2026-06-08, is explicitly
  authoritative=false and gate_fails=true. No recent canonical perf workflow
  completed to replace it.
- The repository contains 13 workflow files, 32 jobs, 288 hand-authored steps,
  and 2,183 workflow lines. tools/molt_dev_gates.toml adds 35 path rules and 73
  unique commands; tools/ci_gate.py adds 66 more checks (56 Tier 1).
- tests/test_ci_workflow_topology.py is 728 lines of mostly string-level YAML
  assertions. It prevents accidental textual changes but does not prove that a
  selected job executed its named semantic contract.

## Findings

### P0 - Main has no admission control

The branch-protection API returns 404 and the repository has no rulesets. Direct
pushes are the dominant integration path. There is therefore no pre-merge
required-check boundary, no merge queue, and no guarantee that a commit has ever
seen a green proof set before becoming canonical.

Required structural move:

- Restore a green baseline, then protect main with required checks generated by
  the proof plan.
- Require pull requests or a merge queue for ordinary integration. If an
  emergency direct-push identity remains, it must not bypass post-push rollback
  policy and must be separately audited.
- Require linear history and forbid force pushes/deletion.

### P0 - Kani is a false-green authority

kani.yml installs Kani, compares its bundled compiler to the workspace floor,
and deliberately skips proofs when incompatible. The job then succeeds. This is
honest logging but dishonest status: a job named Kani bounded verification must
not be green when zero harnesses ran.

Required structural move:

- A required Kani job either runs all registered harnesses or fails/returns a
  clearly non-required unavailable status.
- Until upstream Kani supports the crate floor, move it to a scheduled advisory
  compatibility probe or maintain a genuinely supported proof-crate toolchain
  boundary. Do not lower Rust requirements or ignore rust-version merely to make
  the badge green.
- Emit harness totals and proof totals as a machine-readable artifact. Zero
  executed proofs is always a failure for a required proof lane.

### P0 - The canonical performance trigger is structurally incapable of finishing

Perf Gate starts native and LLVM, five repeats each, on every main push. Molt had
39 main pushes in the sample window. cancel-in-progress repeatedly destroys both
matrix jobs after they have spent minutes or hours compiling/measuring. This is
not debouncing; it is work amplification. A hosted shared runner also cannot be a
stable absolute performance authority merely because a load-average check passed.

Required structural move:

- Delete the main-push trigger. Keep manual and scheduled triggers, plus an
  explicit perf-impact label or proof-plan class on a candidate merge.
- Queue/debounce before runner allocation. Once a measurement begins, do not
  cancel it for a newer push; mark it superseded after artifact publication.
- Run paired base-versus-candidate A/B measurements on the same pinned machine,
  alternating order to control thermal and temporal drift.
- Use stable self-hosted performance workers with recorded CPU, microcode,
  kernel, governor, topology, compiler, linker, and toolchain identities.
- Separate native, LLVM, WASM, linker/build, startup/size, RC/GC, allocation,
  and memory boards. No backend can proxy for another.

Observed immediate benefit: removing per-push perf allocation would have avoided
19.3 runner-hours in just the latest eleven completed runs while preserving a
weekly/manual authority.

### P0 - Named correctness lanes swallow failures

nightly.yml has build-failed examples counted as SKIP, deterministic runtime
failures followed by true, IR verification continue-on-error, three Quint checks
continue-on-error, and both Tier-3 execution and its report continue-on-error.
ci.yml also keeps the LLVM end-to-end differential lane non-blocking. These can
be useful diagnostics, but they are not gates.

Required structural move:

- A job containing advisory experiments must be named advisory and cannot be a
  required status.
- A named determinism/formal/Tier-3 gate must fail if any registered case fails,
  errors, times out, or executes zero cases.
- Track expected unsupported cases in typed manifests with down-only budgets;
  do not encode them as shell true/continue.
- Publish a complete result artifact even on failure, then fail the job.

### P0 - Release publication is not an atomic verified promotion

release.yml builds each OS/architecture matrix cell and uploads that cell directly
to the GitHub release before a downstream verifier exists. It does not install and
smoke the built wheel/bundle on a clean host. A late matrix failure can therefore
leave a partially published release. The same workflow also deploys Cloudflare
and two unrelated Modal services after manifest generation.

Required structural move:

1. Build immutable candidate artifacts only.
2. Download each candidate into a clean platform/Python matrix and test install,
   CLI startup, compile, native execution, bundle integrity, and uninstall.
3. Generate SBOM, checksums, signatures, provenance, and reproducibility verdicts.
4. Promote all artifacts in one publication job only after every candidate passes.
5. Move Cloudflare/Modal deployments to separately versioned deployment workflows.

### P0 - Proof selection has four source authorities

The local integration manifest, CI classifier, tiered gate catalog, and workflow
YAML each describe overlapping path-to-proof relationships. Concrete drift is
already present: formal.yml still filters on the deleted
runtime/molt-backend/src/luau.rs while tools/check_formal_methods.py reads
runtime/molt-backend-luau/src. Luau changes can therefore bypass the formal
workflow.

Required structural move: introduce one declarative proof-plan manifest. Each
proof family declares:

- owned inputs and transitive authority inputs;
- command/executor, target, backend, profile, Python version, OS and architecture;
- local/PR/nightly/release tier and required/advisory status;
- timeout, memory class, cache domain, artifact schema, and zero-work policy;
- dependencies and mutually exclusive resource classes.

Generate the GitHub matrix, local pre-push plan, workflow topology tests, and
human documentation from this manifest. ci_gate becomes the executor; the
separate coarse classifier and hand-maintained YAML command lists disappear.

### P0 - Main-push classification intentionally runs everything

tools/ci_changed_paths.py returns all_true for every event except pull_request.
Thus each direct main push launches Python, Rust, LLVM, Kani, and both dependency
audits regardless of changed paths. Three workflows independently perform a full-
history classifier checkout before doing so.

Replaying the classifier against the actual 39 recent commit diffs shows what the
selection should have been:

| Family | Actual launches | Diff-required launches | Avoidable |
|---|---:|---:|---:|
| Python tooling | 39 | 33 | 15% |
| Rust | 39 | 13 | 67% |
| LLVM | 39 | 9 | 77% |
| Kani | 39 | 6 | 85% |
| Python dependency audit | 39 | 3 | 92% |
| Rust dependency audit | 39 | 2 | 95% |

Use the push event before/after SHAs (handling forced/null before values by
failing open to the full plan), and compute the plan once. Do not run separate
classifier jobs in CI, Kani, and Security.

### P1 - Mega-jobs destroy signal and redo compilation

Docs Gates serializes type checking, 27 generators/audits, and a large pytest
batch. One ty error skips every generator. Rust Build And Unit Smoke serializes a
runtime check, two cross-target checks, cargo build, all workspace tests, multiple
per-crate clippy invocations, workspace clippy, and more tests. The initial build
and warning grep duplicate what tests and clippy compile; per-crate/default
workspace clippy overlap substantially. One early runtime error hides all of it.

Required structural move:

- Partition jobs by artifact/cache and failure domain, not by historical YAML
  section: Python static, generated facts, structural contracts, Rust compile,
  Rust tests, Rust lint, target smoke, backend E2E.
- Use one cargo invocation per feature-equivalent family. Generate feature
  partitions from Cargo metadata; do not hand-list satellite crates.
- Remove the generic cargo build plus warning grep when clippy -D warnings and
  test builds already prove the same feature set.
- Keep no-fail-fast within test families and parallelize independent families.
- Upload results from every partition so one failure does not erase siblings.

### P1 - Coverage does not match the stated product matrix

Routine CI is Ubuntu x86_64 and Python 3.12. Proof Queue Portability covers only
the queue subsystem on Ubuntu/macOS/Windows. Release builds additional platforms
but does not validate compiler semantics on the produced artifacts.

Missing or materially incomplete routine proof:

- CPython 3.13 and 3.14 semantic/version-gating matrices;
- cp313t/cp314t and Molt native free-threaded feature builds;
- Windows and macOS compiler/runtime smoke before release;
- MLIR crate build/test and MLIR-to-LLVM translation;
- Luau execution parity (WASM CI only checks non-empty Luau files);
- WASM browser engines, wasm-browser/webgpu/webnn profiles, and release-fast WASM;
- native aarch64 execution (only one leaf is cross-checked); broader target ABI
  and linker checks for x86_64/aarch64 across Windows/macOS/Linux;
- deterministic GIL-default versus explicit free-threaded behavior;
- TSan/Loom/Shuttle-class concurrent schedule proof. The weekly ASan/Miri lanes
  are currently persistently red and therefore provide no green floor.

Adopt support tiers. Tier A must execute on every PR or merge queue; Tier B runs
nightly on real/emulated hosts; Tier C cross-compiles and verifies ABI/link
metadata. A target is claimed only at the tier its evidence supports.

### P1 - Local hooks are not an installed verification floor

.pre-commit-config.yaml defines Ruff, formatting, ty, secret guard, whitespace,
and YAML checks. However tools/install_git_hooks.py intentionally installs only
the drift pre-push hook and explicitly avoids activating .githooks/pre-commit.
Therefore the repository has config, but no canonical installation path for its
quality hooks. The only enforced pre-push action is worktree drift.

Target split:

- Pre-commit, target under 10 seconds: staged whitespace, secret scan, YAML/TOML
  syntax, Ruff/format on changed files, generated-file ownership check.
- Pre-push, target under 2-5 minutes for normal changes: the proof-plan's changed
  class, including full ty, generator checks, focused pytest, rustfmt, targeted
  cargo check/clippy/test, and drift. Cache results by tree/toolchain digest.
- CI repeats security- and correctness-critical checks; local hooks accelerate
  feedback but are never the only authority.
- Heavy differential, sanitizer, formal, performance, and release checks stay on
  controlled CI workers.

### P1 - Supply-chain and workflow hardening is incomplete

- Five workflows omit explicit least-privilege permissions.
- Third-party actions are pinned to mutable version tags, not immutable SHAs.
- No uv.toml or tool.uv.required-version pins the uv executable. Run
  29616350435 shows setup-uv could not resolve a project version, fetched the
  remote versions manifest, and installed latest (0.11.29), while the canonical
  workstation had uv 0.11.24. A frozen dependency lock does not make two
  different resolver executables reproducible.
- Formal/nightly/release pipe remote installer scripts into a shell.
- LLVM is downloaded and installed from the network on every LLVM/perf job.
- cargo-audit/cargo-deny are installed without checked-in binary/version custody.
- The PR trust gate and labeler duplicate the same policy and use substring
  identity matching. An allow token can match a longer attacker-controlled name
  or email; exact normalized identity is required.

Consolidate trust classification and labeling into one exact-match policy. Pin
all actions/tools by digest with a reviewed update mechanism. Provision LLVM,
Kani, Lean, Quint, audit tools, and linkers through the same checked-in toolchain
manifest and cache immutable tool archives. Add an exact uv version to uv.toml
or tool.uv.required-version, have setup-uv consume it everywhere, and update it
through the same reviewed dependency-update lane as uv.lock.

### P1 - Telemetry is not a CI product

Molt already emits resource-plan and guarded-command data, but most workflows
leave it only in console logs. There is no unified per-run artifact that answers
which proof ran, cache hit rate, compile/link time, peak process-tree RSS,
allocations, artifact size, or test count. A cache can silently miss and a proof
can execute zero cases without a system-level alarm.

Every job should emit one versioned proof-result envelope containing:

- commit/base/tree/toolchain/runner identity and dirty-state proof;
- selected proof-plan entries and why each was selected/skipped;
- executed/passed/failed/skipped/xfail counts, with zero-work policy;
- wall/CPU time, peak RSS, process-tree peak, I/O, cache requests/hits/misses;
- cargo crate timings, linker time/map/size, Python import/collection time;
- produced artifact hashes, size, sections, exported/imported symbol counts;
- benchmark samples, confidence intervals, allocations, peak live bytes, RC/GC
  event counts, lock contention and atomic operation counts where available.

Upload JUnit for tests, SARIF for static/security findings, and the Molt proof
envelope for cross-run trend analysis. Retain failures longer than successes and
publish a compact rolling dashboard.

## Keep, move, consolidate, delete, add

| Family | Decision | Destination |
|---|---|---|
| ty, generated authorities, docs/structural ratchets | Keep; parallelize | pre-push + required PR fast plane |
| Ruff, format, whitespace, YAML, staged secret guard | Keep | installed pre-commit; repeat in PR |
| Full runtime compile, cargo-test truth, LLVM lowering tests | Keep; split by feature family | required PR/merge queue |
| WASM host build and executable parity | Keep; stop behind one mega-job | required when WASM closure changes; nightly full matrix |
| Per-push canonical perf | Delete trigger | scheduled/manual/labeled controlled workers |
| Kani green-on-skip | Delete status shape | scheduled compatibility probe until executable; required when real |
| Generic cargo build + warning grep | Delete duplicate | clippy/test feature partitions |
| Per-crate plus workspace duplicate clippy | Consolidate | generated Cargo-metadata feature partitions |
| Standalone capability/harness import one-liners | Fold into Python smoke tests | required Python test partition |
| Nightly duplicate Quint/Tier-3 continue-on-error jobs | Consolidate and make honest | one reusable formal/heavy workflow |
| Trust gate + trust labeler | Consolidate; exact match | one pull_request_target policy job |
| Dependency audits on every main push | Move by lockfile class | PR lock changes + weekly schedule |
| Full conformance/differential/regrtest | Keep and shard | nightly; release candidate subset |
| ASan/Miri/Kani/formal | Keep and restore green baseline | scheduled, target-triggered, release where relevant |
| Release build direct upload | Replace | build -> verify -> attest -> atomic promote |

## Dependency-ordered target architecture

### Phase 0 - Restore truth and admission

1. Fix the current red baseline or explicitly quarantine known debt in typed,
   down-only manifests.
2. Remove green-on-skip and swallowed required failures.
3. Protect main and enable the merge queue with a small set of stable required
   aggregate contexts.
4. Stop per-push perf allocation immediately.

Exit: no required context can succeed after zero work; main cannot advance on a
red required plan.

### Phase 1 - Establish one proof-plan authority

1. Define the declarative proof manifest.
2. Make tools/ci_gate.py the executor of manifest entries.
3. Generate the changed-path planner, GitHub job matrix, local integration plan,
   and topology tests.
4. Delete ci_changed_paths' duplicate tables and hand-maintained gate command
   lists after equivalence tests prove the cutover.

Exit: one changed-path set produces byte-identical local and CI proof plans.

### Phase 2 - Fast developer loop

1. Install the fast pre-commit floor canonically.
2. Merge drift and changed-class proof into one pre-push driver.
3. Cache proof results by input/toolchain digest and print one compact verdict
   with evidence path.

Exit: common Python-only changes receive useful local feedback under 30 seconds;
normal pre-push proof remains under five minutes without omitting required work.

### Phase 3 - Required PR/merge-queue plane

1. One planner job emits a dynamic matrix.
2. Parallel fast/static, Python, Rust compile/test/lint, LLVM/MLIR, WASM, and
   platform partitions consume it.
3. Aggregate results into a stable required status while retaining every
   partition's artifact.
4. Add explicit job timeouts to all jobs.

Exit: a newly introduced failure is visible in its own partition; unrelated
proof families are not skipped or launched.

### Phase 4 - Complete target matrix

Add CPython 3.12/3.13/3.14 plus free-threaded interpreter probes; Windows,
macOS, Linux x86_64/aarch64; native, LLVM, MLIR, WASM/WASI/browser, Luau, Rust;
and dev/dev-fast/release-fast/release profile evidence according to support tier.
Use emulation only for behavior cells whose limitations are explicit.

Exit: every claimed target/backend/profile cell has a current executable proof,
not a compile-only proxy from another cell.

### Phase 5 - Performance and memory authority

Provision controlled workers and paired base/candidate measurement. Add compiler
wall time, incremental reuse, linker time/map/size, startup, throughput,
allocations, peak-live/process-tree memory, RC/GC/weakref/finalizer activity,
free-threaded contention, and WASM size/startup/runtime boards. Baselines tighten
only after repeated authoritative samples.

Exit: every performance claim has a fresh machine-checkable row and every
optimization proves that it fired.

### Phase 6 - Release promotion

Build once per target, verify the actual artifacts on clean hosts, generate SBOM
and attestations, reproduce or explain nondeterminism, then publish atomically.
Deploy services in independent workflows consuming the released version.

Exit: no partial release can become public and every published artifact was the
artifact tested.

## Acceptance metrics for the CI redesign

- 100% of main commits entered through a green required proof plan.
- 0 required jobs with executed_test_or_proof_count == 0.
- 0 swallowed failures in required jobs.
- At least 95% of ordinary PRs receive first actionable failure within 5 minutes.
- At least 90% cache hit rate on unchanged dependency/toolchain layers, reported
  rather than assumed.
- Per-push runner minutes fall by at least 60% on non-Rust changes without losing
  any selected proof family; the observed recent diff mix already supports this.
- Weekly/nightly/release lanes have a named owner and green-or-triaged SLA; ten
  consecutive red nightlies or six consecutive red sanitizer runs is forbidden.
- Every claimed OS/arch/Python/backend/profile cell has a fresh proof envelope.
- Release publication is all-or-nothing and consumes only verified artifact hashes.

## Evidence commands

The review used read-only commands against canonical C:/Molt/molt-src or the
clean origin/main worktree:

- gh run list --repo adpena/molt --limit 200 --json ...
- gh run view RUN_ID --repo adpena/molt --json jobs
- gh run view RUN_ID --repo adpena/molt --log
- gh api repos/adpena/molt/branches/main/protection
- gh api repos/adpena/molt/rulesets
- replay of tools/ci_changed_paths.py against git diff-tree for 39 CI head SHAs
- static parsing of all workflow YAML, tools/molt_dev_gates.toml, and
  tools/ci_gate.py's check catalog

Representative run IDs: CI 29615445350 and 29616350435; Kani 29615445319; Security 29615445300;
WASM 29613998510; Proof Queue Portability 29615825188; Nightly 29229860507;
Sanitizers 29315510559.
