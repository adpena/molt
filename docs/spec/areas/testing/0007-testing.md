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
arguments through the same helper on Windows, macOS and Linux. Tests that
re-execute their own image use `runtime/test_support/captured_runtime_children.rs`,
which retains the child's complete streams under the same custody and publishes
a source/image-bound descendant record (see `docs/agent/PROOF_QUEUE.md`).

`runtime/molt-runtime/src/test_support.rs` owns process-wide runtime test
transactions. Cleanup must preserve an original body panic and report a second
cleanup failure separately; cleanup failure cannot produce a successful result.
After terminal runtime failure, restoration must neither initialize the runtime
nor release detached owners through partially retired metadata. Pending-call
cleanup uses its existing callback-free queue operation, and the failed process
retains already detached runtime owners until exit. This does not permit tests
to continue using a failed runtime. Child evidence binds each actual mode to its
exact termination, completion and mode-specific proof marker through
`tools/runtime_descendant_receipts.py`; exchanging two modes' valid transcripts
must fail verification. These controls belong to development test binaries and
receipt tooling, not emitted programs.

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
- **Never write into the checkout.** A test puts files, generator outputs and
  scratch state in `tmp_path` or a fixture checkout; a gate that scans the
  repository takes `--root` so its teeth test can plant a violation there. The
  proof executor attests the checkout after every command, so even a file that
  lives for seconds fails every command running beside it.
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
- **Parallelism**: auto-selected: the CPU count, capped so that the jobs' per-job budgets fit the
  process-tree budget. Every job runs in one tree, which the suite sentinel and the guard that
  wraps the suite both bound (`tools/resource_pressure.py` `scheduler_max_jobs`).
  - Override with `--jobs <n>` or `MOLT_DIFF_MAX_JOBS=<n>`.
  - Tune memory budget with `MOLT_DIFF_MEM_PER_JOB_GB=<n>` or `MOLT_DIFF_MEMORY_AVAILABLE_GB=<n>`.
- **Development memory supervision**: repository pytest entry points and
  differential/conformance/regrtest harnesses use the developer guard before
  collection or execution. Configure requested process/tree budgets with
  `MOLT_DIFF_MAX_PROCESS_RSS_GB` and `MOLT_DIFF_MAX_TOTAL_RSS_GB`;
  `MOLT_DIFF_MAX_GLOBAL_RSS_GB` and `MOLT_DIFF_CHILD_RLIMIT_GB` are guard
  configuration, not proof of an aggregate host cap or portable hard RSS limit.
  Sampling, an attempted direct-child `RLIMIT_RSS`, actual platform enforcement
  and generation-owned cleanup have different scopes. Missing samples or census
  rows cannot establish closure. Process groups, numeric PIDs and invocation
  text do not grant custody over escaped or reparented descendants; preserve
  unrelated processes and the host control plane. The actual platform receipt
  and independent failure/cleanup controls must qualify each cell before it
  counts as release evidence. These obligations remain open under
  [V1-12](../../../agent/V1_HANDOFF_FINDINGS.md); see the
  [proof queue contract](../../../agent/PROOF_QUEUE.md). This supervision belongs
  to development and proof tooling; runtime deployment limits are a separate
  [resource contract](../../../RESOURCE_CONTROLS.md).
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

Hosted jobs export process-wide state that local runs lack: the hosted checkout
custody contract (`MOLT_CI_EPHEMERAL_CUSTODY_ROOT` and the GitHub provenance
fields `molt.dx` verifies) and the resource plan from `tools/ci_resource_env.py`
(RSS caps, Cargo jobs, xdist workers). To reproduce a failure that appears only
in CI, load the same state for the local checkout and rerun the test:

```bash
eval "$(python3 tools/hosted_ci_env.py --runner-temp /tmp/molt-runner)"
```

CI starts each partition through the proof plan, whose guarded executor roots
`MOLT_EXT_ROOT`, `TMPDIR` and the Cargo targets in the custody root. For the
same launch path, put the project venv first on `PATH` (the plan requires its
pinned Python) and run
`python3 tools/proof_plan.py --run-command <command-id> --receipt <file>`. Keep
the runner temp short: the backend daemon socket path derives from it.

A unit test that builds a synthetic project or asserts developer-host roots or
guard limits must not inherit that state: it uses the shared
`developer_host_context` and `no_ambient_guard_caps` fixtures from
`tests/conftest.py`. Hosted custody itself has its own cases in
`tests/test_dx_run_context.py`.

A host test that exercises Molt stdlib sources loads them by path, through
`tests/stdlib_intrinsic_registry.py` or `tests/helpers/tinygrad_stdlib_loader.py`,
or runs them in a child interpreter. It never puts `src/molt/stdlib` on the
host `sys.path`: Molt's stdlib would then shadow CPython's (`asyncio`,
`concurrent`, `datetime`) for every later test on the worker and every child it
spawns. `tests/conftest.py` fails the module or test that leaves it there.

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


## Proof executor failure and candidate binding

The proof-plan DAG executor records an ordinary failed partition without
cancelling independent work. Its transitive dependents are skipped with the
failed dependency identity; unrelated running or ready commands remain subject
to the existing dependency, resource, and custody limits. The enclosing receipt
remains failed even if all independent commands succeed.

Cancellation requests use the existing `GuardedCommand` launch capability and
its sticky cancellation file. The guard remains the sole descendant owner,
including the Windows Job. The five-second cancellation observation window is
not a cleanup deadline and never authorizes terminating the guard. A pending
observation yields a global infrastructure outcome, with the unique launch,
startup, summary, cancellation and custody references in the ordinary failed
receipt. The library preserves the exact handle on its custody exception; the
CLI returns failure while the autonomous guard continues to own cleanup. A
later terminal receipt must match the launch and child identities and attest
`descendants_closed` before closure is claimed. A missing or failed terminal
report cannot become an accepted cancellation merely because stop was requested.
The raw guard cancellation code remains 137; the executor projects code 130
only from an admitted cancelled terminal outcome and retains `guard_returncode`
in its command record.
The current native sampling and reaper calls do not establish a hard upper bound
on actual cleanup; observation expiry reports unresolved closure, not cleanup
failure. An eventual close does not retroactively turn the failed proof into a
successful receipt.

The canonical proof plan separately declares `job_reserve_seconds` for each CI
job, matrix family and scheduled job. Admission requires its resource-aware
command-deadline schedule plus that positive reserve to fit the workflow cap;
each matrix runner must fit independently. The reserve covers setup, identity
capture, guard finalization and artifact transport outside command deadlines.
Observed allowances rounded upward to whole minutes and retained declared
scheduled allowances are minimum planning allocations, not hard bounds on
network provisioning or OS cleanup. Generated headroom subtracts both command
work and reserve. Job allowance never extends a child's deadline or turns
unresolved closure into success. Workflow-wide families retain their separate
job topology and do not pretend to have one modeled execution budget.

Global cancellation covers unsafe memory pressure, missing or invalid guard
metrics, unresolved guard or Cargo-quarantine ownership, uncertain descendant
closure, source changes, guard or child signals and host exceptions, lost
executor outcomes, dependency deadlock, and operator or control-plane
interruption. A source-change stop names the dirty Git status entries in its
failure reason, and a failed run prints each failing command with its reason,
so the log identifies the cause without the receipt. Exit code 124 alone does
not establish a safe deadline: the guard must attest its timeout and completed
process closure, and any Cargo recovery must have completed with exact
ownership and no errors.
A complete birth-custodied native interruption inventory with no active
incremental compiler records recovery as unnecessary and retains completed
caches only with a native process-birth fence through termination. Windows Job
lifetime accounting must retain the same process generation from before the
inventory reads until the Job is empty; even a short-lived unseen child changes
that generation. A changed generation, unknown arguments or an unfenced POSIX
snapshot leaves recovery uncertain and preserves the global stop. Missing
observations alone cannot establish that state.
Rustc and in-process Clippy share one compiler-argument authority. Unexpanded
response files and unknown compiler wrappers cannot grant recovery ownership.
Quarantining an observed cache is insufficient if another compiler's ownership
is unknown; both recovered and unnecessary states require a complete inventory
before the executor admits a partition failure.
The executor never performs a second Cargo recovery or a process-name sweep.
Operator interruption drains classified outcomes from cancelled siblings before
writing the final failed receipt and propagating the interruption.

Receipts bind the actual checkout HEAD and immutable Git tree identities. A
provided `GITHUB_SHA` must equal that HEAD. A clean working tree is an additional
condition, not a substitute for candidate identity; a clean checkout switch or
new commit invalidates the active run. The executor compares these identities
before scheduling waves and after partition completion and preserves the initial
candidate in the receipt. These are boundary observations, not proof that no
transient mutation occurred between observations, nor a claim of deterministic
semantics. Existing immutable source-snapshot consumers retain their own stronger
source custody.


## Suite RSS victim attribution

Suite trip decoding retains every positive PID-and-birth pair, reports the
number of unidentified samples, and rejects an entry only when no victim birth
can be identified. Missing root birth does not discard an independently
captured descendant birth. PID-only matching is never sufficient.

Persistent batch builds use the shared suite-trip result merger before strict
retry or subprocess fallback. The client captures the server launch birth and
each serialized request's monotonic start. The existing suite sentinel records
the sample clock and ancestor chains captured by ProcessTreeTracker during live
admission. New custody edges require positive exact-integer parent and child
births with the parent no younger than the child. This shared rule also governs
live descendant adoption and live or persisted Cargo incremental observations;
an old child carrying a reused parent PID cannot enter membership or request
ancestry. Equal native timestamps are admitted. Previously admitted exact
instances retain custody after reparenting, while released identities stay cut.
A descendant can identify a batch request only when its exact birth
entered that server's custody during that request. Old trip records, reused
PIDs, and earlier or suite-adopted daemons cannot identify a later request.
These remain suite-level resource evidence. Success, deadlines, and existing
infrastructure failures retain their precedence. No extra sampler, scheduler,
request worker, or post-exit ancestry lookup is introduced.


## WASM stage audit boundaries

`MOLT_WASM_STAGE_AUDIT=1` enables the existing stage stream. Module emission
begins before the module emitter and closes with `after-module-emission`,
including final byte size. `before-module-finalization` follows the function
loop and precedes resolver, trampoline, table, registry, section, and diagnostic
emission. `after-module-finalization` follows module finishing, optional import
stripping and validation, and relocation sections. The final byte count belongs
to the returned artifact; an interrupted stage has no closing marker.

The audit selection is resolved once per compilation and passed through TIR,
function emission, finalization and import stripping. Disabled auditing performs
no per-function environment lookups. Audit shapes and elapsed projections are
lazy at the shared emission entry point. Audit-only clocks start only when enabled, and the shared TIR module
pipeline samples its observer-only duration only when an observer is installed.
The WASM observer is absent when this audit is disabled. Ordinary TIR progress
and optimization timing retain their existing independent controls.
