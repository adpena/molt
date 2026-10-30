# Trunk proof on the farm

Stage 1, 2026-10-29. Integration owner: Claude integrator. The worker owns
farm proofs; landing, ledger edits and lane retirement remain with the integrator.

## Decision and boundary

`tools/trunk_loop.py run --host bat00 --commit COMMIT --cell ID --cell ID`
transfers an exact Git commit, admits the selected proof commands' prerequisites,
launches a detached host worker and collects its evidence. `collect` resumes
observation after controller interruption. Stage 1 ends with the two named Windows
cells and disconnect survival proved; continuous scheduling and failure routing
are Stage 2, not implicit claims of this implementation.

`tools/cross_hosts.toml` remains the only host inventory. Its typed host records
declare OS, shell, custody root, checkout, launcher and permitted proof commands.
The smoke runner and trunk runner share SSH transport and nonce-owned staging.
No task is a shell command supplied by an operator: the selected commit's
`tools/proof_plan.toml` owns argv, dependencies, budgets and toolchain closure.
Commands run serially on a host; a failed cell retains its result and does not
authorize changing workers, deadlines or selecting another workload.

The canonical checkout is a Git object donor, never reset or moved. A new
detached worktree is allocated under its custody root for each run, using a
controller-created bundle and bounded Git-transfer semantics. Staging and
worktree identities are retained in a local run record before launch. No source,
toolchain, environment or seal is placed on bat00's exFAT volume. Existing
`E:\\Molt\\trunk` and the hand-written scratch runner are donors for evidence only;
the integrator reconciles and retires them after landing.

## Provision before testing

The selected command/dependency closure supplies required toolchains. Existing
`ProofPlan`, `toolchain_env.selected_toolchains`, `molt.checkout_provisioning`
(which owns locked `uv sync`) and `proof_plan.admit_toolchains` retain admission.
Pinned releases use `molt.tool_releases`, not a second downloader or installer.
Unsupported or absent providers fail provisioning before any test starts.
This does not claim whole-suite product preparation (HF-586); future cells must
declare their actual product dependencies in the proof plan before selection.

Choose **controller-seeded pinned archives**, rather than adding pip's
`truststore` as a new bootstrap dependency. The controller caches each selected
host asset by pinned SHA-256 and size, transfers it, and the host imports it
through the same size/digest/publication boundary as an HTTPS download. No
certificate checks are disabled or system settings changed. This removes the
farm's dependency on its incomplete OpenSSL chain construction for github.com.
An explicit host download remains available through the existing verified HTTPS
path and exact pin; it is not a retry after a failed seed. OS verification would
repair chain building for unrestricted host downloads, but requires a separately
admitted dependency before bootstrap and does not provide offline input custody.

## Execution and evidence

One launcher interface uses a new session on POSIX and
`Invoke-CimMethod Win32_Process.Create` on Windows. CIM starts outside the SSH
session's job, so closing or killing the SSH client cannot terminate the worker.
The worker redirects binary stdout/stderr directly to `run.log`, writes `cmd`,
per-cell `rc`, JUnit and the canonical proof receipt, then publishes terminal
status last. The controller records launcher acknowledgement separately from
completion; missing terminal evidence is unknown, never success. Collection
retains original bytes and hashes in
`<artifact-root>/evidence/trunk-loop/<commit>/<host>/<cell>/<run>/`, preserving
older results. Recoverable records name exact remote allocations. Nothing
deletes resources while process closure is unknown.

JUnit and pytest duration reporting are diagnostic additions through the pytest
environment; they do not change selectors, workers or timeouts. Canonical proof
receipts remain the source of guard outcomes. Stage 2 will consume
`ci_triage --junit --json` as its only JUnit failure parser, adopt HF-422's
completion/guard/policy/parent-key corrections, and propose `ledger_edit.py`
commands plus owning-lane input files. There is no automatic revert.

## Reconciliation and prior art

The parked postsubmit donor `37e82b522c34` supplies useful exact-source and
single-runner concepts; its revert policy and landing-policy edits conflict with
the owner-approved fix-forward workflow and are not imported. The triage donor
`b3e790d89462` and its owning design remain preserved for Stage 2; importing its
second JUnit parser or uncorrected review-v2 behavior would duplicate authority.

- [LUCI Swarming bots](https://chromium.googlesource.com/infra/luci/luci-py/+/HEAD/appengine/swarming/doc/Bot.md)
  separate task custody from execution and retain a content-addressed cache
  across tasks. Stage 1 borrows that separation without deploying a service.
- [Chromium sheriffs](https://www.chromium.org/developers/tree-sheriffs/) and
  [Sheriff-o-Matic](https://chromium.googlesource.com/infra/infra/+/main/appengine/sheriff-o-matic/)
  group actionable failures for owners; proposed ledger edits serve that role.
- [LLVM developer policy](https://www.llvm.org/docs/DeveloperPolicy.html#buildbots)
  uses post-commit, platform-specific builders and author notification; retain
  commit/parent identity instead of attributing every red to the latest author.
- [Google TAP culprit finding](https://research.google.com/pubs/archive/45794.pdf)
  motivates continuous postsubmit and narrowed failure investigation; no culprit
  claim follows from a single failing run.
- [Rust bors](https://forge.rust-lang.org/infra/docs/bors.html) separates merge
  ownership; [crater](https://github.com/rust-lang/crater) compares explicit
  experiments and records platform limitations. The farm does not own landing.
- [Bazel repository cache and distdir](https://bazel.build/run/build#repository-cache)
  reuse hash-identified downloads; [remote execution](https://bazel.build/remote/rbe)
  distinguishes remote work from cache hits. A retained log is not a fresh proof.
- The owner's Spawn write-up (2026-10-29 dispatch: trunk every 15 minutes,
  fix-forward, judges) informs Stage 2's bounded cadence and routing. Its URL was
  not supplied or located; those details are owner-provided requirements, not an
  independently verified external source.

## Proof obligations

Unit controls must reject corrupt, truncated or substituted archives without
network access or publication; execute a real POSIX detached child across parent
exit; and verify Windows CIM command quoting and failure acknowledgement against
its public API. The real bat00 disconnect experiment is the Windows lifetime
oracle. Both named proof cells must return source-bound receipts, even when the
tests are red. No broader suite or repeated matching PASS is required here.
