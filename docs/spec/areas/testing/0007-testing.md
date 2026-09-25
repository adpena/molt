# Molt Testing & Verification Strategy

See `README.md` for quick-start testing commands and CI parity job summaries.
Minimum required gates are defined in
`docs/spec/areas/testing/0008_MINIMUM_MUST_PASS_MATRIX.md`.

Rust tests that compile helper objects or executables use
`runtime/test_support/cargo_test_artifacts.rs`. The running Cargo test image
owns their output root and retention; tests do not create a second temp-root or
destructor cleanup policy. Exclusive owner directories have compact identities,
with descriptive labels retained as metadata so linker intermediate filenames
do not inherit unbounded path lengths. Commands use lossless owner-relative
arguments through the same helper on Windows, macOS and Linux.

## Test quality and agent-written tests

This is the shared test-authoring contract for human and agent contributions.
Optimize for distinct defects detected and trustworthy claims per unit of
maintenance and execution cost, not generated lines, test counts, or coverage
percentages. Required conformance and release matrix gates still apply.

- **Start with a contract and an independent oracle.** Identify the observable
  behavior and plausible defect the test distinguishes. Derive expected results
  from the specification, a version/platform-matched CPython run, an independently
  justified invariant, or a reviewed fixture. Do not compute the expectation
  with the implementation being tested or record its current output as truth.
- **Reuse the owning suite.** Search existing cases and fixtures before adding
  another test file, harness, helper, or matrix. Extend or parameterize cases
  when they exercise the same contract; retain distinct boundaries and failure
  modes. Small diffs still need tests when their semantic risk warrants them;
  reversible low-impact edits do not need implementation-mirroring tests.
- **Establish that a regression test can fail for the intended reason.** Prefer
  observing it fail on the old behavior, then pass after the fix. For already
  implemented changes, use a bounded negative control or relevant mutation when
  practical. An import error or broken fixture is not the desired red phase.
  Do not revert shared WIP to manufacture one, or claim sensitivity you did not
  establish. Passing current tests alone does not establish it.
- **Exercise the consumer named in the claim.** Mock external or expensive
  boundaries only when the mocked behavior is outside that claim. Keep the
  decision under test real. A stub compiler can test payload transport, but
  cannot prove compilation; calling bootstrap directly cannot prove a shipped
  launcher works. Component tests complement installed CLI, ABI, native and WASM
  execution, never substitute for those acceptance paths. Static source checks
  are appropriate for structural contracts, not evidence of runtime behavior.
- **Use assertions with consequences.** Check results, relevant side effects,
  error type/diagnostic, and state preservation or cleanup where contractual.
  "Did not crash", truthiness, or a mock being called is insufficient when the
  result is the contract. Assert interaction order/count only when that is the
  invariant. Do not weaken assertions, regenerate expectations, swallow errors,
  or add skips/xfails merely to make an implementation pass; changes to expected
  behavior need an independently justified contract change.
- **Make failures reproducible.** Control randomness, time and environment at
  their real boundaries; use bounded synchronization rather than sleep-based
  timing assumptions. Preserve minimized fuzz/differential inputs, seed and
  applicable target coordinates in existing replay evidence. Normalize only
  differences the contract allows; do not normalize away semantic mismatches.
- **Make fixtures and claims portable.** Cover Windows, macOS and Linux,
  applicable architectures, CPython versions, native/WASM, and claimed
  backends/profiles through the canonical matrices. Distinguish the host running
  a test from the compilation target. Reuse capability/version authorities for
  gates; avoid host-derived target expectations, path/shell/word-size assumptions,
  and blanket skips that hide supported cells. Simulated coordinates test policy
  selection, not execution on that OS/architecture/interpreter. Unexecuted cells
  remain unverified; explicit exclusions need a contract reason.
- **Budget the execution, not the coverage.** Use the smallest fixture and
  dependency closure that preserves the invariant. Profile slow setup/build/run
  stages before optimizing. Reuse immutable fixture inputs without sharing
  mutable state across tests; batch integration builds and keep them out of the
  fast unit loop. Run required checks, then repeat or widen only for changed
  inputs, failures, or unresolved claims. Timing assertions belong in controlled
  performance lanes, not noisy functional tests.

During review, ask: which realistic defect would pass unnoticed, is the oracle
independent, and does this add distinct signal beyond existing tests? Remove or
merge redundant cases without losing their unique contract coverage. Report the
actual checks, results, omitted surfaces and evidence paths through the existing
handoff/proof records; do not add a second checklist or per-test receipt system.

### Sources and limits

- OpenAI's [Astra prompting guidance](https://developers.openai.com/api/docs/guides/latest-model/gpt-6-astra#testing-and-verification)
  recommends meaningful, task-appropriate tests and reruns only when justified.
  [Rethinking skills and prompts](https://developers.openai.com/blog/rethinking-skills-and-prompts-for-gpt-6-astra)
  explains how older testing instructions can induce unnecessary verification.
- Simon Willison's [Red/green TDD](https://simonwillison.net/guides/agentic-engineering-patterns/red-green-tdd/)
  motivates observing the intended failure to avoid tests that pass without
  exercising the new behavior. Use that sensitivity check where useful, not as
  a mandatory full-suite or test-first ritual for every edit.
- Meta's [TestGen-LLM study](https://arxiv.org/abs/2402.09171) filters generated
  tests for build validity, reliable execution and incremental coverage. It
  supports reviewing generated tests as candidates, not accepting their volume
  as quality; coverage alone does not prove a correct oracle or semantic adequacy.

The Meta study concerns earlier models; it does not measure Astra's defect rate.
These sources inform the engineering policy, not a claim that one model always
writes bad tests. Prompt effectiveness must be judged from actual contributions.

## Version Policy

Molt targets **CPython 3.12+** semantics within the verified subset. Use
`TargetPythonVersion` in `src/molt/target_python.py` for target-version decisions,
not the host interpreter's version. Release OS/architecture coordinates are
declared in `config/release_targets.toml` and projected by `tools/gen_release_matrix.py`;
`src/molt/verified_subset.py` binds their conformance closure. Extend those
authorities and their proofs when admitting a future Python version, OS or
architecture; do not fork local support lists or equate parseable future versions
with verified support.

For differential cases, express version/platform/architecture/backend
applicability through `MOLT_META` consumed by `tools/compat/test_policy.py`.
Compare with the matching CPython reference and document intentional differences.
Portability policy applies to test fixtures and developer/release tools as well
as generated programs; a pass on one host cannot establish the whole matrix.

## 1. Differential Testing: The `molt-diff` Harness
`molt-diff` is a specialized tool that ensures Molt semantics match CPython. The current harness lives in `tests/molt_diff.py` and builds + runs binaries via `molt build` with `--build-profile dev` (Molt dev profile maps to Cargo `dev-fast` by default).

### 1.0 Performance + Memory Controls
- **Parallelism**: auto-selected based on CPU and available memory (default budget: 2 GB/worker).
  - Override with `--jobs <n>` or `MOLT_DIFF_MAX_JOBS=<n>`.
  - Tune memory budget with `MOLT_DIFF_MEM_PER_JOB_GB=<n>` or `MOLT_DIFF_MEM_AVAILABLE_GB=<n>`.
- **Memory guard**: enabled by default with adaptive per-process,
  per-test-tree, global RSS, and a direct-child `RLIMIT_RSS` backstop. Configure
  deliberate investigation caps with `MOLT_DIFF_MAX_PROCESS_RSS_GB`,
  `MOLT_DIFF_MAX_TREE_RSS_GB`, `MOLT_DIFF_GLOBAL_RSS_LIMIT_GB`, or
  `MOLT_DIFF_CHILD_RLIMIT_GB`. Test execution is not allowed to bypass memory
  custody; direct pytest sessions re-exec through `tools/memory_guard.py` before
  collection, and differential/conformance/regrtest harnesses keep their RSS
  guards active by policy. The child limit never constrains virtual-address
  reservations through `RLIMIT_AS` or `RLIMIT_DATA`; recursive RSS polling
  remains authoritative. The lineage tracker keeps
  reparented/session-changing descendants inside RSS accounting, while teardown
  stays scoped to the guarded root process group plus exact escaped descendant
  PIDs; repo sentinels must exclude ancestor and Claude/Codex/control-plane
  process groups from kill sets even when those groups contain repo-looking
  children.
- **OOM retry**: OOM failures are retried once with `--jobs 1` (disable via `--no-retry-oom` or `MOLT_DIFF_RETRY_OOM=0`).
- **Warm cache**: `--warm-cache` or `MOLT_DIFF_WARM_CACHE=1` prebuilds all tests to seed `MOLT_CACHE`.
- **Failure queue**: failed tests are written to `MOLT_DIFF_ROOT/failures.txt` (override with `--failures-output` or `MOLT_DIFF_FAILURES`).
- **Summary sidecar**: `MOLT_DIFF_ROOT/summary.json` (or `MOLT_DIFF_SUMMARY=<path>`) includes run metadata and RSS aggregates when enabled.
- **Memory report**: run `uv run --python 3.12 python tools/diff_memory_report.py --run-id <id>` to list top RSS offenders (uses `rss_metrics.jsonl`).
- **Top offenders printout**: when `MOLT_DIFF_MEASURE_RSS=1`, the harness prints top 5 RSS offenders at the end (override with `MOLT_DIFF_RSS_TOP=<n>`).
- **Summary top list**: `summary.json` includes `rss.top` with the top offenders (file + build/run RSS).

### 1.1 Methodology
1.  **Input**: A Python source file `test_case.py`.
2.  **Execution**:
    - Run `uv run --python 3.12 python test_case.py` -> Capture `stdout`, `stderr`, `exit_code`.
    - Run `uv run --python 3.12 python tests/molt_diff.py test_case.py` -> Build with Molt, run the binary, capture outputs.
3.  **Comparison**: Assert that all captured outputs are identical.

### 1.2 State Snapshoting
For complex tests, we use `molt.dump_state()` to export a JSON representation of global variables and compare the JSON output between runs.

### 1.3 Curated Parity Suite
Differential cases are organized by lane:
- `tests/differential/basic/`: core language + builtins parity.
- `tests/differential/pyperformance/`: pyperformance manifest/runner integration smoke lane.
- `tests/differential/stdlib/`: stdlib module/submodule parity.
- `tests/differential/moltlib/`: Molt-specific library surface (optional lane; add only for non-CPython APIs).

Run lane sweeps via:
```
uv run --python 3.12 python tests/molt_diff.py --build-profile dev tests/differential/basic
uv run --python 3.12 python tests/molt_diff.py --build-profile dev tests/differential/pyperformance
uv run --python 3.12 python tests/molt_diff.py --build-profile dev tests/differential/stdlib
```

The verified-subset contract uses `tools/compat/test_policy.py` to project one
canonical, duplicate-free path closure for each Python/OS/architecture/backend
coordinate. `tools/verified_subset.py run --coordinate ...` passes that exact
list to this harness and retains raw, resolved, and backend outcomes. Only the
full source-bound CI receipt closure proves the subset; `check` validates policy
and reports debt.

### 1.4 Differential coverage reporting
Generate metadata coverage summaries from `# MOLT_META` headers:
```
uv run --python 3.12 python tools/diff_coverage.py
```
The report is written to `tests/differential/COVERAGE_REPORT.md` by default.

Validate lane organization + coverage index integrity:
```
uv run --python 3.12 python tools/check_differential_suite_layout.py
```

### 1.5 Verified-Subset Scope Policy For Too-Dynamic Cases
- Use this only for intentionally unsupported semantics called out by the
  vision/break-policy docs and the dynamic execution policy contract
  (for example `exec`/`eval` heavy behavior).
- Canonical per-test declaration:
  `# MOLT_META: verified_subset_scope=dynamic_execution_policy expect_fail=molt expect_fail_reason=too_dynamic_policy`.
- `tools/compat/test_policy.py` parses the declaration and projects the scope
  for every consumer. There is no parallel path registry.
- Harness behavior (`tests/molt_diff.py`):
  - CPython pass + Molt fail on expected-failure test => `[XFAIL]` and counted as pass.
  - CPython pass + Molt pass on expected-failure test => `[XPASS]` and counted as failure.
- Guardrail: scoped expected failures are not a substitute for lowering; remove
  all three metadata fields as soon as support lands.
- Verified-subset behavior: the `dynamic_execution_policy` scope is a projected
  language-policy exclusion. Every applicable expected failure outside a
  configured verification scope, including an XFAIL translated to the harness's
  resolved pass, blocks a verified-subset receipt.

## 2. Automated Test Generation (Hypothesis)
We use `Hypothesis` to generate random Python ASTs that fall within the Molt Tier 0 subset.
- **Rules**:
    - Only use supported primitives.
    - No `exec`/`eval`.
    - Valid scope resolution.
- **Goal**: Find edge cases in type inference or codegen that cause divergence from CPython.

## 3. Metamorphic Testing
To verify the optimizer:
1.  Take a program `P`.
2.  Apply a semantics-preserving transformation `T` (e.g., `inline_function`, `rename_variable`) to get `P'`.
3.  Ensure `Molt(P)` and `Molt(P')` produce the same output and similar performance characteristics.

## 4. Guard/Deopt Validation
To test Tier 1:
- Create "Bait" tests that trigger deoptimization (e.g., passing a `float` to a function that was specialized for `int`).
- Verify that the runtime correctly switches to the slow path without crashing or losing state.

## 5. Continuous Integration Gates
- **Rust**: `cargo test` (runtime + core unit tests).
- **Python**: `uv run --python 3.12 pytest` (unit and integration tests under `tests/`).
- **Differential**: run `uv run --python 3.12 python tests/molt_diff.py <case.py>` for curated parity cases (expand over time).
- **Benchmarks**: `tools/bench.py` for local validation; add CI regression gates as they stabilize.

### Execution acceptance and deadline controls

An execution with incomplete process custody or guard infrastructure failure
provides diagnostic observations, not semantic acceptance. Queue audits retain
product errors from its transcript but must not promote them into the semantic
frontier; the infrastructure failure remains an actionable audit error.

WASM execution tests distinguish guest-VM deadlines from native child-process
deadlines. The guest control executes a nonterminating WASM loop in Node and
requires the VM timeout diagnostic. The process control re-executes the native
test binary, confirms child readiness, then exercises the same retained-Child
termination, reap and captured-output path used by execution tests. It does not
depend on Node availability or require a forcibly terminated language runtime
to deliver a graceful custody handshake. Neither control relaxes proof custody.
