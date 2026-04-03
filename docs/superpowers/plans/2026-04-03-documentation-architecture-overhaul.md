# Documentation Architecture Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restructure Molt's documentation into a newcomer-first, single-source-of-truth system with generated `STATUS.md` summary blocks and CI-enforced anti-drift rules.

**Architecture:** Keep only a few hand-authored top-level docs with sharply defined roles: `README.md` explains, `docs/spec/STATUS.md` states current support, `ROADMAP.md` plans, `docs/INDEX.md` navigates, and `docs/getting-started.md` handles first-run guidance. Volatile compatibility and benchmark summaries move into generated blocks inside `docs/spec/STATUS.md`, and a repo-local docs checker enforces ownership boundaries and stale-content bans in local lint and CI.

**Tech Stack:** Markdown, Python tooling scripts in `tools/`, pytest, existing generated compat docs, existing benchmark summary tooling

---

## File Map And Migration Matrix

### New / modified implementation files

| Path | Responsibility |
| --- | --- |
| `tools/update_status_blocks.py` | Update/check generated summary blocks in `docs/spec/STATUS.md` |
| `tests/test_update_status_blocks.py` | Unit tests for `tools/update_status_blocks.py` |
| `tools/check_docs_architecture.py` | Enforce doc ownership rules and stale-content bans |
| `tests/test_check_docs_architecture.py` | Unit tests for docs architecture checks |
| `tools/bench_report.py` | Retarget benchmark summary updates from README to `docs/spec/STATUS.md` |
| `tests/test_bench_report.py` | Unit tests for new `tools/bench_report.py` status-doc update behavior |
| `tools/dev.py` | Local lint integration for new docs checks |
| `.github/workflows/ci.yml` | CI enforcement for docs checks |

### Doc ownership and disposition

| Path | Role After Migration | Disposition |
| --- | --- | --- |
| `README.md` | OSS landing page for newcomers | Rewrite |
| `docs/getting-started.md` | Install, verify, first build/run, troubleshooting | Create |
| `docs/INDEX.md` | Navigation hub only | Rewrite / trim |
| `docs/spec/STATUS.md` | Current-state ledger with generated blocks | Rewrite |
| `ROADMAP.md` | Forward-looking priorities only | Rewrite |
| `SUPPORTED.md` | Thin pointer/alias to canonical current-state and proof docs; no support claims | Trim |
| `docs/CANONICALS.md` | Canonical docs map and role guidance | Update |
| `docs/DEVELOPER_GUIDE.md` | Contributor guidance with corrected doc-role references | Update |
| `docs/spec/README.md` | Spec index with corrected status/roadmap references | Update |
| `docs/ROOT_LAYOUT.md` | Root-surface contract updated for retained/trimmed top-level docs | Update |
| `docs/COMPATIBILITY_CORPUS_MANIFEST.md` | Proof-corpus manifest only, not current-support contract | Update |
| `docs/ROADMAP_90_DAYS.md` | Execution-slice doc aligned with `STATUS.md` current-state ownership and `ROADMAP.md` future-plan ownership | Update |
| `docs/BENCHMARKING.md` | Benchmark workflow docs targeting `STATUS.md` generated bench block | Update |
| `docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md` | Proof workflow links updated for trimmed `SUPPORTED.md` role | Update |
| `docs/spec/areas/tooling/0011-ci.md` | CI gate documentation updated for docs gate | Update |
| `docs/spec/areas/perf/0008-benchmarking.md` | Retarget stale README benchmark summary references | Update |
| `docs/spec/areas/perf/0603_BENCHMARKS.md` | Retarget stale README benchmark summary references | Update |
| `docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md` | Retarget stale README summary references | Update |
| `docs/spec/areas/perf/0512_ARCH_OPTIMIZATION_AND_SIMD.md` | Retarget stale README summary references | Update |
| `docs/spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md` | Retarget stale README summary references | Update |

### Generated `STATUS.md` blocks for Phase 1

These are the only generated blocks in this implementation tranche. Do not add more until they have stable inputs.

| Block | Markers | Input Sources | Writer |
| --- | --- | --- | --- |
| Compatibility summary | `<!-- GENERATED:compat-summary:start -->` / `<!-- GENERATED:compat-summary:end -->` | `docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md`, `docs/spec/areas/compat/surfaces/stdlib/stdlib_platform_availability.generated.md` | `tools/update_status_blocks.py` |
| Benchmark summary | `<!-- GENERATED:bench-summary:start -->` / `<!-- GENERATED:bench-summary:end -->` | explicit `--native` and `--wasm` benchmark artifact paths passed to `tools/bench_report.py` | `tools/bench_report.py` |

`README.md` and `ROADMAP.md` remain fully hand-authored in this tranche. Validation stays as a short hand-authored section in `docs/spec/STATUS.md` until there is a stable machine-readable validation summary source worth automating.

`docs/benchmarks/bench_summary.md` remains as the detailed generated benchmark report. The new `STATUS.md` bench block is a concise top-level summary derived from the same benchmark inputs, not a replacement for the detailed report artifact.

## Task 1: Add `STATUS.md` generated compatibility blocks

**Files:**
- Create: `tools/update_status_blocks.py`
- Create: `tests/test_update_status_blocks.py`
- Modify: `docs/spec/STATUS.md`

- [ ] **Step 1: Write the failing tests for marker replacement and source parsing**

Create `tests/test_update_status_blocks.py` with fixtures that:

- build a temporary `STATUS.md` containing:
  - `<!-- GENERATED:compat-summary:start -->`
  - `<!-- GENERATED:compat-summary:end -->`
- write sample `stdlib_intrinsics_audit.generated.md` content with:
  - `Total audited modules`
  - `intrinsic-backed`
  - `intrinsic-partial`
  - `python-only`
- write sample `stdlib_platform_availability.generated.md` content with:
  - `Modules with explicit Availability metadata`
  - `WASI blocked`
  - `Emscripten blocked`
- assert that the generated block contains those summarized values in a short Markdown bullet list
- assert `--check` fails when the block is stale

- [ ] **Step 2: Run the tests to confirm they fail**

Run:

```bash
./.venv/bin/python -m pytest -q tests/test_update_status_blocks.py
```

Expected: FAIL because `tools/update_status_blocks.py` does not exist yet.

- [ ] **Step 3: Implement `tools/update_status_blocks.py`**

Implement a small repo-local script with:

- `--write` to rewrite `docs/spec/STATUS.md`
- `--check` to verify the file is already up to date
- constants for the two compat input file paths
- a narrow parser that only reads the generated summary lines needed for the block
- exact marker handling for:
  - `<!-- GENERATED:compat-summary:start -->`
  - `<!-- GENERATED:compat-summary:end -->`

The output block should stay short and human-readable, for example:

```md
- Stdlib lowering audit: `877` modules audited; `41` intrinsic-backed; `836` intrinsic-partial; `0` python-only.
- Platform availability metadata: `66` modules with explicit availability notes; `41` WASI-blocked; `37` Emscripten-blocked in CPython docs.
- Deep evidence: see the stdlib intrinsics audit and platform availability matrices under `docs/spec/areas/compat/surfaces/stdlib/`.
```

- [ ] **Step 4: Add the generated block markers to `docs/spec/STATUS.md`**

Rewrite the future `Compatibility summary` section of `docs/spec/STATUS.md` so it contains the exact markers above and no hand-maintained numeric counts outside the markers.

- [ ] **Step 5: Run the writer and verify the tests pass**

Run:

```bash
./.venv/bin/python tools/update_status_blocks.py --write
./.venv/bin/python -m pytest -q tests/test_update_status_blocks.py
./.venv/bin/python tools/update_status_blocks.py --check
```

Expected:

- pytest PASS
- `--check` exits 0 after `--write`

- [ ] **Step 6: Commit**

Run:

```bash
git add tools/update_status_blocks.py tests/test_update_status_blocks.py docs/spec/STATUS.md
git commit -m "docs: add generated status compatibility block"
```

## Task 2: Retarget benchmark summary generation from README to `STATUS.md`

**Files:**
- Modify: `tools/bench_report.py`
- Create: `tests/test_bench_report.py`
- Modify: `docs/spec/STATUS.md`
- Modify: `docs/BENCHMARKING.md`
- Modify: `docs/spec/areas/perf/0008-benchmarking.md`
- Modify: `docs/spec/areas/perf/0603_BENCHMARKS.md`
- Modify: `docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md`
- Modify: `docs/spec/areas/perf/0512_ARCH_OPTIMIZATION_AND_SIMD.md`
- Modify: `docs/spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md`

- [ ] **Step 1: Write the failing benchmark status-doc tests**

Create `tests/test_bench_report.py` with tests that:

- create a temporary `STATUS.md` containing:
  - `<!-- GENERATED:bench-summary:start -->`
  - `<!-- GENERATED:bench-summary:end -->`
- pass small sample native/wasm JSON benchmark artifacts into `tools/bench_report.py`
- assert the script updates the `STATUS.md` benchmark block instead of requiring README markers
- assert the script errors clearly if the benchmark markers are missing

- [ ] **Step 2: Run the tests to confirm they fail**

Run:

```bash
./.venv/bin/python -m pytest -q tests/test_bench_report.py
```

Expected: FAIL because `tools/bench_report.py` still targets README markers.

- [ ] **Step 3: Modify `tools/bench_report.py` for the new status-doc target**

Change the interface and behavior as follows:

- remove the README-specific update path
- add a status-doc-specific update path such as:
  - `--update-status-doc`
  - `--status-doc-path docs/spec/STATUS.md`
- update markers to:
  - `<!-- GENERATED:bench-summary:start -->`
  - `<!-- GENERATED:bench-summary:end -->`
- keep the generated benchmark prose concise enough for `STATUS.md`
- keep generating `docs/benchmarks/bench_summary.md` as the detailed benchmark artifact; the new `STATUS.md` block is an additional concise summary, not a replacement

This is an intentional cleanup and does not need backwards compatibility with `--update-readme`.

- [ ] **Step 4: Add the benchmark markers to `docs/spec/STATUS.md`**

Place the benchmark markers under a dedicated `Performance summary` section in `docs/spec/STATUS.md`.

- [ ] **Step 5: Update benchmark docs to the new source of truth**

Update all in-scope benchmark docs so they point to the `STATUS.md` generated benchmark block rather than README:

- `docs/BENCHMARKING.md`
- `docs/spec/areas/perf/0008-benchmarking.md`
- `docs/spec/areas/perf/0603_BENCHMARKS.md`
- `docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md`
- `docs/spec/areas/perf/0512_ARCH_OPTIMIZATION_AND_SIMD.md`
- `docs/spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md`

- [ ] **Step 6: Run tests and a smoke command**

Run:

```bash
./.venv/bin/python -m pytest -q tests/test_bench_report.py
./.venv/bin/python tools/bench_report.py --help
```

Expected:

- pytest PASS
- help output includes the new status-doc update flag

- [ ] **Step 7: Commit**

Run:

```bash
git add tools/bench_report.py tests/test_bench_report.py docs/spec/STATUS.md docs/BENCHMARKING.md docs/spec/areas/perf/0008-benchmarking.md docs/spec/areas/perf/0603_BENCHMARKS.md docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md docs/spec/areas/perf/0512_ARCH_OPTIMIZATION_AND_SIMD.md docs/spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md
git commit -m "docs: retarget benchmark summaries to status doc"
```

## Task 3: Add docs ownership and anti-drift enforcement

**Files:**
- Create: `tools/check_docs_architecture.py`
- Create: `tests/test_check_docs_architecture.py`
- Modify: `tools/dev.py`
- Modify: `.github/workflows/ci.yml`
- Modify: `docs/spec/areas/tooling/0011-ci.md`

- [ ] **Step 1: Write the failing docs checker tests**

Create `tests/test_check_docs_architecture.py` using temporary repo fixtures that verify the checker fails on:

- `README.md` containing banned internal sections such as `Optimization Program Kickoff`
- `README.md` containing large current-state ledgers such as `Capabilities (Current)` or `Limitations (Current)`
- `ROADMAP.md` containing `Last updated:` or other current-state ledger patterns
- `docs/spec/STATUS.md` missing generated markers
- stale references in docs that still instruct benchmark summary updates into README
- `SUPPORTED.md` containing substantive current-support claims instead of acting as a pointer doc

- [ ] **Step 2: Run the tests to confirm they fail**

Run:

```bash
./.venv/bin/python -m pytest -q tests/test_check_docs_architecture.py
```

Expected: FAIL because `tools/check_docs_architecture.py` does not exist yet.

- [ ] **Step 3: Implement `tools/check_docs_architecture.py`**

Implement a repo-local checker that:

- scans the real repo by default
- exits nonzero on ownership violations
- has focused, explicit rules instead of fuzzy heuristics

Initial enforced rules:

- `README.md` must not contain banned section names or banned stale phrases
- `README.md` must link to `docs/getting-started.md` and `docs/spec/STATUS.md`
- `docs/spec/STATUS.md` must contain both generated marker pairs
- `ROADMAP.md` must not present itself as the current-state source of truth
- `SUPPORTED.md`, if present, must be a pointer/alias doc and must not contain a detailed support ledger
- docs must not instruct users to update benchmark summaries in README

- [ ] **Step 4: Wire the checker into local lint**

Modify `tools/dev.py` so `tools/dev.py lint` runs:

```bash
python3 tools/update_status_blocks.py --check
python3 tools/check_docs_architecture.py
```

Keep these checks near the other repo-policy gate scripts.

- [ ] **Step 5: Wire the checker into CI**

Add a docs gate step to `.github/workflows/ci.yml` that runs at least:

```bash
python3 tools/update_status_blocks.py --check
python3 tools/check_docs_architecture.py
```

Update `docs/spec/areas/tooling/0011-ci.md` to describe the docs gate accurately.

- [ ] **Step 6: Run the checker and tests**

Run:

```bash
./.venv/bin/python -m pytest -q tests/test_check_docs_architecture.py
./.venv/bin/python tools/check_docs_architecture.py
```

Expected:

- pytest PASS
- checker exits 0 against the repo once the rewrites land

- [ ] **Step 7: Commit**

Run:

```bash
git add tools/check_docs_architecture.py tests/test_check_docs_architecture.py tools/dev.py .github/workflows/ci.yml docs/spec/areas/tooling/0011-ci.md
git commit -m "tools: enforce documentation architecture rules"
```

## Task 4: Rewrite newcomer-facing top-level docs

**Files:**
- Modify: `README.md`
- Create: `docs/getting-started.md`
- Modify: `docs/INDEX.md`
- Modify: `tests/test_compatibility_contract_docs.py`

- [ ] **Step 1: Update the doc contract tests for the new top-level roles**

Extend `tests/test_compatibility_contract_docs.py` so it asserts:

- `README.md` still states the CPython `>=3.12` target, no-host-Python story, and design exclusions
- `README.md` links to `docs/getting-started.md` and `docs/spec/STATUS.md`
- `README.md` no longer contains the old internal-section phrases banned by the docs checker
- `docs/getting-started.md` contains install, verify, and build/run guidance

- [ ] **Step 2: Run the tests to confirm they fail**

Run:

```bash
./.venv/bin/python -m pytest -q tests/test_compatibility_contract_docs.py
```

Expected: FAIL because `docs/getting-started.md` does not exist yet and README still contains the old shape.

- [ ] **Step 3: Rewrite `README.md` as the landing page**

Rewrite `README.md` to contain only:

1. project definition
2. why Molt exists / differentiators
3. what it supports today
4. what it intentionally does not support
5. five-minute quickstart
6. install options
7. honest status snapshot
8. deeper-doc links

Explicitly remove or relocate:

- optimization kickoff sections
- long capability and limitation ledgers
- operator-level inventories
- benchmark detail tables
- install verification deep detail

- [ ] **Step 4: Create `docs/getting-started.md`**

Add a dedicated quickstart doc with:

1. prerequisites
2. install methods
3. `molt doctor --json`
4. build/run `examples/hello.py`
5. a simple compare or benchmark path
6. platform pitfalls and recovery

Keep install and quickstart detail here, not duplicated in `README.md`.

- [ ] **Step 5: Trim `docs/INDEX.md` into a navigation-only hub**

Update `docs/INDEX.md` so it:

- links to the new `docs/getting-started.md`
- links to `README.md`, `docs/spec/STATUS.md`, `ROADMAP.md`, and the spec index
- removes internal sprint/status notes that do not belong in a navigation hub

- [ ] **Step 6: Run tests and the docs checker**

Run:

```bash
./.venv/bin/python -m pytest -q tests/test_compatibility_contract_docs.py
./.venv/bin/python tools/check_docs_architecture.py
```

Expected: PASS.

- [ ] **Step 7: Commit**

Run:

```bash
git add README.md docs/getting-started.md docs/INDEX.md tests/test_compatibility_contract_docs.py
git commit -m "docs: rewrite newcomer-facing documentation"
```

## Task 5: Rewrite `STATUS.md`, `ROADMAP.md`, and `SUPPORTED.md` around clear ownership

**Files:**
- Modify: `docs/spec/STATUS.md`
- Modify: `ROADMAP.md`
- Modify: `SUPPORTED.md`
- Modify: `docs/COMPATIBILITY_CORPUS_MANIFEST.md`

- [ ] **Step 1: Extend tests/checker expectations for the new ownership boundaries**

Add or update expectations so that:

- `docs/spec/STATUS.md` is the only current-state ledger
- `ROADMAP.md` is forward-looking only
- `SUPPORTED.md` is a thin pointer doc, not a second support contract
- `docs/COMPATIBILITY_CORPUS_MANIFEST.md` speaks in proof-corpus terms, not current-support terms

Implement these as:

- additional assertions in `tests/test_compatibility_contract_docs.py` where appropriate
- explicit rules in `tools/check_docs_architecture.py` for content classes that must not appear in `ROADMAP.md` and `SUPPORTED.md`

- [ ] **Step 2: Run the tests/checker to confirm they fail**

Run:

```bash
./.venv/bin/python -m pytest -q tests/test_compatibility_contract_docs.py tests/test_check_docs_architecture.py
./.venv/bin/python tools/check_docs_architecture.py
```

Expected: FAIL until the docs are rewritten.

- [ ] **Step 3: Rewrite `docs/spec/STATUS.md`**

Restructure `docs/spec/STATUS.md` into short sections only:

1. project scope and target
2. supported today
3. intentionally unsupported
4. known major gaps / blockers
5. validation summary (short hand-authored)
6. compatibility summary (generated block)
7. performance summary (generated block)
8. deep links

Delete long historical narrative and inline sprint diaries instead of trying to keep them in sync.

- [ ] **Step 4: Rewrite `ROADMAP.md` and trim `SUPPORTED.md`**

For `ROADMAP.md`:

- keep only strategic target, priorities, milestones, active blockers, and deferred work
- remove “current state” prose that duplicates `STATUS.md`

For `SUPPORTED.md`:

- trim it to a short alias/pointer doc
- keep only links to `docs/spec/STATUS.md`, proof workflows, and roadmap
- remove current-support bullets and “Last updated” ledgers

- [ ] **Step 5: Update the proof-corpus manifest**

Edit `docs/COMPATIBILITY_CORPUS_MANIFEST.md` so it no longer treats `SUPPORTED.md` as an operator-facing support contract. Keep it as a proof-corpus manifest only.

- [ ] **Step 6: Regenerate and verify**

Run:

```bash
./.venv/bin/python tools/update_status_blocks.py --write
./.venv/bin/python tools/check_docs_architecture.py
./.venv/bin/python -m pytest -q tests/test_compatibility_contract_docs.py tests/test_check_docs_architecture.py tests/test_update_status_blocks.py
```

Expected: PASS.

- [ ] **Step 7: Commit**

Run:

```bash
git add docs/spec/STATUS.md ROADMAP.md SUPPORTED.md docs/COMPATIBILITY_CORPUS_MANIFEST.md tools/check_docs_architecture.py tests/test_compatibility_contract_docs.py tests/test_check_docs_architecture.py
git commit -m "docs: separate current state from roadmap"
```

## Task 6: Update adjacent docs and remove stale cross-references

**Files:**
- Modify: `docs/CANONICALS.md`
- Modify: `docs/DEVELOPER_GUIDE.md`
- Modify: `docs/spec/README.md`
- Modify: `docs/ROOT_LAYOUT.md`
- Modify: `docs/ROADMAP_90_DAYS.md`
- Modify: `docs/BENCHMARKING.md`
- Modify: `docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md`
- Modify: `docs/spec/areas/tooling/0011-ci.md`

- [ ] **Step 1: Find the remaining stale references**

Run:

```bash
rg -n "README\\.md.*status|README\\.md.*Performance|update-readme|SUPPORTED\\.md|README and \\[ROADMAP\\.md\\] are kept in sync|source of truth for Molt's current capabilities" docs README.md ROADMAP.md docs/spec/STATUS.md
```

Expected: a short list of now-stale cross-references to clean up.

- [ ] **Step 2: Update adjacent docs to the new ownership map**

Make the following role clarifications:

- `docs/CANONICALS.md`: newcomer path is `README.md` + `docs/getting-started.md`; current state is `docs/spec/STATUS.md`; roadmap is `ROADMAP.md`
- `docs/DEVELOPER_GUIDE.md`: contributor docs references updated to the new top-level roles
- `docs/spec/README.md`: clarify that `docs/spec/STATUS.md` is current-state only and `ROADMAP.md` is future-only
- `docs/ROOT_LAYOUT.md`: retain `SUPPORTED.md` only as a thin top-level pointer doc
- `docs/ROADMAP_90_DAYS.md`: describe itself as an execution slice derived from `ROADMAP.md`, not a competing current-state document
- `docs/BENCHMARKING.md`: point benchmark summary publishing to `docs/spec/STATUS.md`
- `docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md`: update support-contract links to reflect `SUPPORTED.md` as a pointer and `docs/spec/STATUS.md` as the detailed current-state source
- `docs/spec/areas/tooling/0011-ci.md`: mention the docs gate explicitly if not already covered by Task 3 edits

- [ ] **Step 3: Run the checker and grep again**

Run:

```bash
./.venv/bin/python tools/check_docs_architecture.py
rg -n "update-readme|Optimization Program Kickoff|Capabilities \\(Current\\)|Limitations \\(Current\\)" README.md docs
```

Expected:

- checker exits 0
- grep returns no stale references in the rewritten doc set

- [ ] **Step 4: Commit**

Run:

```bash
git add docs/CANONICALS.md docs/DEVELOPER_GUIDE.md docs/spec/README.md docs/ROOT_LAYOUT.md docs/ROADMAP_90_DAYS.md docs/BENCHMARKING.md docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md docs/spec/areas/tooling/0011-ci.md
git commit -m "docs: align adjacent references with new ownership model"
```

## Task 7: Final verification and repository hygiene

**Files:**
- Verify only; no intentional edits unless cleanup is required

- [ ] **Step 1: Re-run the generated block writers**

Run:

```bash
./.venv/bin/python tools/update_status_blocks.py --write
```

If benchmark artifacts are available for the chosen canonical pair, run:

```bash
./.venv/bin/python tools/bench_report.py --native bench/results/bench.json --wasm bench/results/bench_wasm.json --update-status-doc --status-doc-path docs/spec/STATUS.md
```

If the canonical benchmark artifact names differ, use the real artifact paths in the repo and update docs accordingly in the same change.

- [ ] **Step 2: Run the repo-local docs gates**

Run:

```bash
./.venv/bin/python tools/update_status_blocks.py --check
./.venv/bin/python tools/check_docs_architecture.py
```

Expected: both exit 0.

- [ ] **Step 3: Run the targeted test suite**

Run:

```bash
./.venv/bin/python -m pytest -q tests/test_update_status_blocks.py tests/test_bench_report.py tests/test_check_docs_architecture.py tests/test_compatibility_contract_docs.py
```

Expected: PASS.

- [ ] **Step 4: Inspect the final diff**

Run:

```bash
git status --short
git diff --stat -- README.md docs/getting-started.md docs/INDEX.md docs/spec/STATUS.md ROADMAP.md SUPPORTED.md docs/CANONICALS.md docs/DEVELOPER_GUIDE.md docs/spec/README.md docs/ROOT_LAYOUT.md docs/COMPATIBILITY_CORPUS_MANIFEST.md docs/BENCHMARKING.md tools/update_status_blocks.py tools/check_docs_architecture.py tools/bench_report.py tools/dev.py .github/workflows/ci.yml tests/test_update_status_blocks.py tests/test_bench_report.py tests/test_check_docs_architecture.py tests/test_compatibility_contract_docs.py
```

Expected: only the planned documentation/tooling files are changed, with no unrelated files reverted.

- [ ] **Step 5: Commit the final cleanup if needed**

Run:

```bash
git add README.md docs/getting-started.md docs/INDEX.md docs/spec/STATUS.md ROADMAP.md SUPPORTED.md docs/CANONICALS.md docs/DEVELOPER_GUIDE.md docs/spec/README.md docs/ROOT_LAYOUT.md docs/COMPATIBILITY_CORPUS_MANIFEST.md docs/ROADMAP_90_DAYS.md docs/BENCHMARKING.md docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md docs/spec/areas/tooling/0011-ci.md docs/spec/areas/perf/0008-benchmarking.md docs/spec/areas/perf/0603_BENCHMARKS.md docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md docs/spec/areas/perf/0512_ARCH_OPTIMIZATION_AND_SIMD.md docs/spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md tools/update_status_blocks.py tools/check_docs_architecture.py tools/bench_report.py tools/dev.py .github/workflows/ci.yml tests/test_update_status_blocks.py tests/test_bench_report.py tests/test_check_docs_architecture.py tests/test_compatibility_contract_docs.py
git commit -m "docs: enforce single-source documentation architecture"
```

If there is no final cleanup delta beyond the prior task commits, skip this commit.
