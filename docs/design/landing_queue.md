# Landing queue

Status: active design; presubmit/postsubmit slice added 2026-10-30; fast
landing (section 9) added 2026-11-04.
Owner: landing and main health. Part of HF-334.

One queue on the owner's machines publishes the immutable candidate that passed
presubmit, then checks main through postsubmit. The selection and main-health
policy belongs to [observed_inputs.md](observed_inputs.md). No agent pushes to
main by hand.

The current local driver implements immutable candidate custody and
selected-volume readiness through the existing proof plan. Unfiltered pytest
targets share one invocation using the largest declared owning envelope.
The driver retains local rules. Static checks run before heavy gates, which
share the proof executor and leave command receipts; shared Cargo writers remain
serialized. The family queue,
cross-host scheduling and reusable action receipts below remain release work.
The [integration contract](../ops/INTEGRATION.md) owns implemented local
behavior. Which tests a landing selects, and when a passing result may be
reused, follow [observed_inputs.md](observed_inputs.md); this document owns
what a landing runs, bisection and the queue.

## Problem

Measured 2026-10-14 to 2026-10-17:

- CI main failed on 40 consecutive completed runs. The last green run was
  2026-10-14 at c55df5661.
- One CI run (39090704454, 5b913336c) failed in six jobs: 42 frontend tests
  (HF-341), a source-scraping test, rustfmt, a Linux-only Darwin link test,
  33 Windows runtime-gate tests, and others. No landing caught any of them:
  HF-331, HF-332, HF-336 to HF-341 and the uv-fixture gap recorded on
  HF-334. HF-339 and the Windows cluster fail only on a host other than this
  Mac. The frontend fix itself changed four test files, and its landing ran
  none of them.
- `tools/molt_dev.py integrate` selects landing tests from hand-written
  `[[rule]]` globs in `tools/proof_plan.toml`. CI selects proof families from
  the families' declared inputs. These are two selection authorities, and
  the landing one misses tests by construction.
- `integrate` keeps no landing receipt, so nothing shows which gates a landed
  commit passed. 338e1c6c4 reached main unformatted, though the always-run
  rustfmt gate rejects it (HF-338).

## Import-graph selection density

A prototype selected the tests a change reaches through the static import
graph of `src/`, `tools/` and `tests/`, each test's `conftest.py` files, and
asset inference (a string literal or `ROOT / "a" / "b"` chain that names a
tracked file is a dependency).

- It selected the broken test for every break above that runs on this host
  (the uv fixture through `tools/proof_plan.toml`, the weakref scraper through
  its Rust file, the daemon test through `tests/molt_diff.py`).
- Over the last 150 first-parent commits on main, 58 reached all 990 test
  files. The others reached a median of 24 (65 when a test's own directory
  literals count as reads).
- `tests/conftest.py` alone reaches 458 modules, 202 of them in `molt.cli`
  and 82 in `molt.frontend`, so any CLI or frontend change reaches every test.

The shelved per-test result cache (HF-263) found the same density from
runtime records. The landing now selects this way
(`tools/test_dependency_graph.py`, [testing spec](../spec/areas/testing/0007-testing.md#landing-test-selection)).
On 2026-10-30 each hub module's transitive closure selected 983 of 983 test files.
That closure now belongs to postsubmit; presubmit selection follows the linked
policy. Platform-only failures still require the corresponding farm cell.
Observed-input records remain deferred.

## Selecting by plan entry, not by the plan file (HF-457)

`tools/proof_plan.toml` is a shared input: 361 KB of rules, toolchain
policies and CI commands. Three rules globbed the whole file, so any edit to
it ran their suites. On 2026-10-22 a dependency landing changed one CI command
and nothing else in the plan; `dev-tooling` and `source-extension-proof-policy`
matched only through the glob and cost 278 s of its 347 s of gates, 84% of it
the landing driver's own end-to-end tests. Of the 12 commits before it that
edited the plan, each changed one to five entries.

The plan's sections are a closed set (`proof_plan.PLAN_SECTIONS`); a table
section changes entry by entry, by its `name` or `id`. Selection diffs the
upstream and candidate plans into changed entries
(`changed_plan_entities`). A rule claims what it reads with
`plan_entities` (`"section"` or `"section:name"`; a toolchain policy also
claims the policies it depends on) and matches when an entry it claims
changes; a changed rule selects itself, so its new tests run. A plan that
cannot be read selects every rule. `ProofPlan.load` refuses a rule that globs
the plan file and a section that no rule claims whole. Bazel invalidates
only the targets a BUILD-file edit changes for the same reason.

## Prior art

- **bors and homu** (Rust): the "not rocket science rule": main always passes
  its tests. The bot tests each approved change merged onto main and pushes
  only that tested commit. **Rollups** batch small changes into one run.
- **Zuul** (OpenStack): speculative gating. It tests queued changes in
  parallel, each on top of the ones ahead of it, and on a failure removes the
  change and retests the ones behind it.
- **GitHub merge queue** and **Chromium's CQ**: required checks on the merge
  result, not on the branch.

## Design

1. **One selection authority.** `select_landing_gates` reads the candidate's
   committed plan, with typed `LandingMode` supplied by its caller.
2. **One queue.** `land` prechecks and enrolls an exact commit. The runner
   merges ready lanes onto main in its registered sandbox, invokes `integrate`
   for presubmit and confirmed publication, then finishes managed retirement.
3. **One executor.** Both modes prepare toolchain closures and output capacity
   through the same authorities. Cross-host farm scheduling remains future work;
   local results do not qualify cells on other hosts.
4. **Rollups and bisection.** The runner tests up to N ready lanes in one
   candidate, as bors rollups do. A failure tests the halves, so one bad lane
   costs O(log N) extra runs and the others still land.
5. **Receipts.** Every candidate leaves one receipt: each command's cell,
   host, duration, peak RSS and result. Longest-first ordering reads it.
6. **Main health receipts.** Postsubmit records green, red, cancelled or
   inconclusive in the existing integration evidence store. Failure records order later presubmits;
   no verdict reuse or action-record store is implemented here.
7. **Integration custody.** The existing integration lock, source fences and
   exact-commit push confirmation remain authoritative. The one pre-push hook
   refuses a main update without a landing receipt; `integrate` and
   `land --fast` (section 9) are the two paths that write one.

### Postsubmit mechanics

After pending requests drain, the runner retains its queue lock and fetches main.
The clean registered sandbox switches to that exact head. Selection compares it
with `refs/molt/last-green`; when that ref is absent it selects all tracked source
inputs rather than inventing a passing baseline. Rule tokens that require a Git
base bind to the selected head for this initial full-source run.

`_run_postsubmit_gates` uses the ordinary gate preparation and executor. Pytest
writes JUnit beside each gate's logs, using its built-in `--junitxml` and xunit1
file attributes. The aggregate `<head>.<run>.postsubmit-result.json` receipt records
the outcome, test node IDs, prior green, suspect commits, exact restoration
commands, gate-output directory and any infrastructure error. A failed receipt
orders later work; a new invocation can requalify the head after a repair. `queue status`
includes these receipts; there is no additional registry.

A pending request sets a sticky cancellation predicate. Active gates pass it to
their existing guards, and unscheduled work stops. A cancelled outcome is admitted
only after the guard reports descendant closure; unresolved custody remains an
error. The runner then services the request, and postsubmit selects the newest
head. A passing run re-fetches main before `git update-ref refs/molt/last-green
<head> <old>`; an absent old ref uses Git's zero object ID. A changed head cancels
the result, and a failed compare-and-swap cannot overwrite another writer.

`PostsubmitStatus` is the verdict authority. RED requires a
`CandidateFailure`: a gate failed for its own reason, this run's own JUnit lists
a failing test (even when its gate's command passed), or main's own gate policy
is invalid. Any other driver, guard or storage stop, with no evidence of a
candidate failure, is INCONCLUSIVE, with its error in the receipt; so is a
failed last-green compare-and-swap. CANCELLED
requires a closed cancellation. Only RED lists suspects and restoration
commands, and only RED orders later presubmits. An inconclusive run exits
unsuccessfully and waits for an explicit `queue run`. LUCI Buildbucket makes
the same split: FAILURE for the input, INFRA_FAILURE for the infrastructure
(https://chromium.googlesource.com/infra/luci/luci-go/+/ad07866c5533/buildbucket/proto/common.proto).

Red receipts list every commit since the previous green as suspect. Restoration
commands walk first parents newest first, using `git revert --no-edit -m 1` for
merges, so lane commits are not reverted twice. Before the first green exists,
there is no proven restoration target and the receipt states that fact. The
integrator owns any restoration; the runner executes none of these commands.

## Landing engineering (HF-373)

Measured on 2026-10-20, one landing costs an agent a manual loop. The agent
rebases, rewrites the ledger's commit ids by hand, and starts a private
wrapper that polls the host lease every 60 s. A move of main stops the
wrapper, and the agent must repeat the loop. Each lane pays the
always-run gates alone (26 policy gates, 56 s; HF-35). The candidate's own
`tools/molt_dev.py` drives its landing: on 2026-10-20 the cleanup change
crashed with `ImportError` after a confirmed push, because the driver
imported a module from the worktree it had just removed. Five mechanisms
remove this loop.

### 1. A pinned controller

The code that decides whether a change lands is never the change.

- **Prior art.** rustc builds stage 1 with the compiler that `src/stage0`
  pins. Go's `cmd/dist` builds with `GOROOT_BOOTSTRAP`. Zuul tests job
  changes from untrusted projects speculatively, but a change to a trusted
  config project takes effect only after it merges. bors and the GitHub merge
  queue run as services, not from the pull request.
- **Design.** `integrate`, `land`, the queue runner, `queue withdraw`,
  `lane start` and `lane retire` fetch first. Then they run the driver from
  main's tip: `git archive <tip>` of `tools`, `src/molt`, `src/sitecustomize.py`,
  `config` (the data
  the driver reads, such as tool release pins), `pyproject.toml`, `uv.lock`
  and `.python-version` extracts into
  `<artifact-root>/molt-state/controllers/<repository>/<key>`, where the key is the digest of
  those tree and blob ids. `uv sync --frozen --no-dev --group controller`
  builds the controller's environment from main's lock. The `controller`
  group holds pytest and pytest-xdist: gate selection reads pytest arguments
  with pytest's own parser and installed plugin options (HF-179, HF-655). A
  source from before that group syncs without it. The driver runs there as
  `python -I`, so no module, dependency version or `PYTHON*` setting comes
  from the candidate. The candidate still supplies its declared data: gate
  selection from its `tools/proof_plan.toml`, and its tests. A change to the
  driver takes effect after it lands, as a stage0 bump does.
- **Adoption.** `--candidate-controller` runs the candidate's driver for a
  reviewed controller repair or controller-owned input-contract transition
  that main cannot prepare. The complete selected gates remain mandatory;
  this is an explicit bootstrap, never a post-failure retry policy. Follow the
  [canonical integration contract](../../ops/INTEGRATION.md#the-driver-lifecycle-integrate).
  The adopted queue runner and supervisor execute from the exact clean committed
  candidate's immutable controller source and its canonically synchronized
  dependency environment. Isolated startup keeps editable caller hooks out of
  the adopted driver. Source use overlaps acknowledged
  startup and terminal publication, so retiring the lane cannot remove their
  code. An older in-lane runner leaves retirement pending, releases its queue
  lock and hands already-published requests to the surviving main controller;
  exact enrolled custody closes there without repeating gates.
  Once landed, the next default invocation uses main's newly pinned controller.
  A landing's progress stream and
  receipt record which controller ran; a lane command prints it
  (`==> pinned controller <key>`). Each command refuses a pinned claim that
  does not name the controller it runs from
  (`molt_dev_controller.integration_controller`).
- **Cost.** Two `git rev-parse` calls and one no-op `uv sync` per landing. A
  new controller tree costs one extraction (1.5 s, 33 MB) and one environment
  build from the warm uv cache (0.8 s, 35 MB). Retention keeps the four most
  recently used controllers. Shared source use protects controller startup and
  execution; retention requires an exclusive lock and a fresh age check.
  Automatic checkout maintenance reuses this same source publisher and detached
  supervisor, with its own retained terminal result.
- **Status.** Implemented 2026-10-20. A first stdlib-only design ran the
  driver as `python -I -S`; the end-to-end landing test found that worktree
  cleanup imports `msgspec`, so the controller pins main's dependencies as
  rustc's stage0 brings its own libraries.

### 2. Declarative checkout provisioning

A checkout's inputs come from its lock files, on demand, as `uv run` does.
Managed Python freshness, including the landing sandbox, follows the
[managed image contract](foundation/56_dx_buildspeed_tooling.md#managed-python-images).
Gate preparation and generated-merge regeneration both call canonical `ensure`,
including explicit `UV_PROJECT_ENVIRONMENT` selections. Neither a valid recipe
nor a stored selection can call an interpreter current after its patch changes.
The runner clears inherited environment selectors before selecting its own
sandbox, so ensure can delete an idle drifted generation in place and rebuild
it with an exact patch request. An explicit caller selection instead refuses
replacement, and live reader locks preserve the old image. The pinned
controller owns the implementation of ensure; existing immutable controllers
are refreshed from landed main, never patched in place.

- **Prior art.** `uv run` syncs the environment from `uv.lock` before each
  run. rustup installs the toolchain that `rust-toolchain.toml` names on first
  use. `npm ci` installs exactly the lock file. Nix and devcontainers declare
  a checkout's whole environment.
- **Design.** `molt.checkout_provisioning` owns one table of checkout inputs:
  the uv environment (`uv.lock`) and the Acorn parser tree
  (`tools/browser_asset_graph/package-lock.json`). Each input keeps a stamp of
  its lock digest and tool version. A consumer calls `ensure(<input>)`: a
  matching stamp costs one `stat` and one small read; a mismatch runs
  `npm ci --ignore-scripts --prefer-offline` or `uv sync --frozen`, which read
  from the shared caches. `gen_browser_asset_graph.py` stops asking a person
  to run a bootstrap script. `lane start` provisions the default prerequisites. Optional registered inputs,
  such as the Edgebox TypeScript tree, are provisioned only when explicitly
  selected. Status and lane start share the default selection. Queue gate
  preparation follows the selected proof toolchain closures instead: only an
  environment-role prerequisite requests a source development image, and
  non-Python inputs are provisioned only when selected. Preparation binds tool
  identity to the selected interpreter before execution; every launch still
  checks the image. Explicit selectors remain fail-closed even for generic
  shell gates. Retirement recognizes all registered inputs so an explicitly
  provisioned tree keeps its custody.
- **Editable-source custody.** The uv stamp also binds its resolved checkout.
  A current editable environment rechecks its exact PEP 610 metadata path; a
  foreign checkout is refused, even with identical lock bytes. Cold
  unstamped admission uses the existing isolated no-site Python probe. Explicit
  `MOLT_VENV` borrowing remains read-only; manual noneditable environments keep
  their existing contract. The existing Git-scoped provisioning lock is unchanged.
  This does not exclude simultaneous first writers from different checkouts,
  arbitrary external `uv` writers, or same-checkout sync during reads: shared use
  protects retirement, not content mutation.
- **Cost.** 2.2 s for a new worktree (uv 1.5 s, npm 0.7 s, both from warm
  caches); about 1 ms per consumer afterwards.
- **Landed (slice 3).** `src/molt/checkout_provisioning.py` holds the uv
  environment, the Acorn tree and the managed Git hooks. Admission and gate
  environment selection ensure them; `tools/bootstrap_browser_asset_graph.py`
  is deleted. Measured on a fresh clone: uv 1.0 s, npm 0.8 s, hooks 0.4 s;
  a current stamp costs under 0.3 ms. See the
  [integration contract](../ops/INTEGRATION.md).

### 3. Stable change ids

A commit keeps one identity across rebases, and the ledger cites it.

- **Prior art.** Gerrit's `commit-msg` hook adds a `Change-Id:` trailer that
  survives rebase and amend. Jujutsu gives every change a stable change id
  apart from its commit id.
- **Design.** The `commit-msg` hook adds `Change-Id: I<40 hex>` when it is
  absent. A ledger row cites a commit of the same lane as `change:I<8+ hex>`.
  After the rebase and the patch-id check, `integrate` maps each token to the
  rebased commit that carries that trailer. It rewrites the token to the
  9-character commit id and amends the lane's ledger commit, so gates and the
  push see the final text. A token that no candidate commit carries fails
  before any gate. A row cites a landed commit by its commit id, as now.
- **Cost.** One `git log` over the candidate range, and only when a touched
  document under `docs/` holds a citation.
- **Status.** Implemented 2026-10-20 (`.githooks/commit-msg`,
  `tools/molt_dev_change_ids.py`). Code and tests may hold citation-shaped
  data, so only Markdown under `docs/` resolves.

### 4. Orchestrator-owned lanes

The driver creates, provisions and retires each lane.

- **Prior art.** Zuul's Nodepool and GitHub's ephemeral runners create a
  fresh node for each job and delete it after. Bazel runs each action in a
  sandbox it owns.
- **Design.** `molt_dev.py lane start <slug>` creates the `lane/<slug>`
  branch from the policy-selected base in a slot worktree, writes the coordination record, and
  provisions it. The checkout selected by `--repo` is preferred only when its
  Git registration is a clean, detached, unlocked, non-prunable slot under the
  selected root; otherwise selection keeps path order. Protected registrations
  reserve their numbers. This preserves a selected checkout's artifact namespace
  without weakening provisioning or build identity checks, and starting a lane
  still consumes an active WIP position. Detach is the availability commit
  point: a new lane may claim the slot while the old lane finishes deleting its
  refs and closing its record, with no further checkout mutation by retirement.
  `lane retire <slug>` accepts a landed lane, or an abandoned
  one after it writes a Git bundle. It deletes the branches, detaches the
  clean slot at main and closes the record; the slot keeps its checkout state
  for the next lane (HF-491). A long-lived monorepo checkout and Bazel's
  per-workspace output base keep build state the same way: the build keys it
  by checkout path (Cargo fingerprints and sccache keys hold absolute paths),
  so a fresh path per lane rebuilt everything cold. `tools/drift_harvest.py`
  counts a free slot as neither live nor prunable. A lane recorded before
  slots existed keeps the old retirement: its worktree and checkout state
  (HF-313) are removed. `lane list` shows each
  lane's state. The queue automatically retires each managed lane it lands,
  including requests already represented on main. Enrollment transfers a clean,
  exact tip/path under its existing coordination record; the owner stops editing.
  Primary and ordinary source checkouts are retained. Locked, advanced, dirty,
  mismatched or unharvested lanes are held, not force-removed. Ignored content is
  disposable only within current provisioning-owned trees, verified through the
  existing lock-stamp/sentinel authority (`land` first makes the trees a lane
  owns current, as `uv run` syncs before it runs; a stale input is held, whatever
  its writer tagged), or, outside the declared trees, when it holds no signal:
  bytecode in a PEP 3147 `__pycache__` (the import system reads it only beside
  its source, so it is cache even after its source is gone; this releases the
  one trace of a module that was never committed and then deleted; a `.pyc`
  outside `__pycache__` imports without its source and is held), a directory
  tagged by the
  [Cache Directory Tagging Specification](https://bford.info/cachedir/) (Cargo,
  Ruff, pytest, mypy), a setuptools `*.egg-info` that holds only the files
  `egg_info` writes, an empty checkout-state lock (`molt.checkout_state`, which
  leaves only with its checkout), and a plain directory that holds only these.
  `lane start` leaves bytecode and the editable build's egg-info behind; tool
  runs add tagged caches. Git names an untracked directory beside each ignored
  path inside it, and those paths are judged, not the directory's name.
  Unknown ignored signal requires harvest.
  Persistent publication locks are classified by their canonical owner and leave
  only with the whole checkout, never by a lock-deletion cleanup step. Multiple
  Git registrations of one lane branch are shared custody and block retirement.
  Checkout-state/process custody still protects owned work, without claiming a
  global lease over arbitrary writers. Exact-tip/path retirement intent is
  durable in the existing coordination record before deletion; a failed terminal
  record write can resume only after all owned resources are confirmed absent.
  Bulk idle retention, cache policy and WIP limits belong to the
  [DX tooling design](foundation/56_dx_buildspeed_tooling.md); this boundary
  closes the already-published managed lane and adds no TTL or retention registry.

  Publication and retirement are separate outcomes. A request reports `landed`
  only after its managed retirement completes; `landed_retirement_failed`
  retains the published SHA and instructs the integration owner to finish the
  existing retirement command, without retesting publication. Failed receipts
  remain evidence even after the lane record records a later completion. V2
  requests remove the old opt-in; deployment drains old requests before the
  controller update, and any late v1 record is preserved and refused explicitly.

  This adapts the owner-driven completion principle in Google's
  [Deprecation chapter](https://abseil.io/resources/swe-book/html/ch15.html):
  migration includes consumer closure and removal, not only an announcement.
  For old WIP, two source/history passes (an independent pass when available)
  establish whether an idea is useful now, already satisfied, or superseded for
  a verified reason. Harvest useful work into current owners and land it first;
  preserve unresolved evidence with an indexed path and an explicit owner or
  question. Archiving alone does not close reconciliation.
- **Cost.** The same Git and provisioning work an agent does by hand now,
  without the forgotten steps.
- **Landed (slice 3).** `tools/molt_dev_lanes.py` implements `lane start`,
  `lane retire` and `lane list`. A lane is landed when every commit is on
  main by ancestry, or (for ordinary commits only) Change-Id or patch-id.
  Historical ordinary replay/amend landings remain represented. Merge commits
  require ancestry because manual resolution changes have no equivalent parent
  patch; matching a merge trailer cannot authorize retirement.

### 5. A queue with speculative batches and bisection

One runner owns main. Agents enqueue and continue.

- **Prior art.** Zuul's dependent pipelines test each queued change on top of
  the changes ahead of it, in parallel. The GitHub merge queue tests merge
  groups up to a size limit with a build concurrency. bors-ng tests a batch
  and bisects a failed batch. Uber's SubmitQueue ("Keeping Master Green at
  Scale", EuroSys 2019) ranks speculative builds by their chance to land.
- **Design.** `molt_dev.py land` checks the lane, runs its precheck, and
  writes one request under `<git-common-dir>/molt/landing-queue/`, then
  returns. It first applies the lane plan's `repair` steps and commits what
  they change, so a stale projection never reaches a gate. The precheck then
  runs, in the lane, the selected gates that only read the candidate's files
  (`SelectedGate.static`: toolchains within `PRECHECK_TOOLCHAINS`, not a
  pytest session) and the try run (`SelectedGate.try_run`: the dependency
  graph's presubmit cohorts and the gates of `precheck = true` rules, such as
  changed-crate Clippy; `tools/molt_dev_precheck.py`) within
  `PRECHECK_BUDGET` (the static gates run to their verdict outside it; the
  budget clock starts with the try run; repairs are unbounded and outside the
  try-run budget), so a lint, format, generator, policy or direct-reader
  test failure costs that lane alone instead of a batch run; GitHub's merge
  queue likewise admits only changes whose required checks passed, Zuul runs
  its check pipeline before its gate pipeline, and Chromium's commit queue and
  Google's TAP run a fast presubmit of affected targets. The budget cancels
  what is still running, names every refused gate and queues the lane, as a
  disk-capacity refusal of the try run does; the batch still runs every gate.
  The repair commit runs the installed hooks, takes only the lane's own diff
  and declared generator outputs, and is refused for a branch that a pending
  request pins. `--no-precheck` skips repairs and precheck.
  `--precheck-only` runs it and stops: nothing is queued and no runner
  starts, so a worker proves its handoff on the landing path itself.
  `--static-only` repairs and runs only the static gates, the cheap
  deterministic tier (rustc's `x.py test tidy`, Chromium's upload
  presubmit). `--fast` is the integrator's push (section 9). `land`
  starts the runner if none holds the runner lock. `land` and the runner run from the pinned
  controller: a lane branched before a driver fix once queued itself with
  its own old driver, which started a runner that could not import `tools`.
  It builds a candidate in a repository-keyed external sandbox worktree,
  selected by `molt.custody_layout.landing_sandbox_path`. The Git common
  directory still owns queue metadata and locks. Keeping candidate source
  outside every other checkout prevents ancestor Cargo configuration from
  changing the selected build inputs. Registration must prove the sandbox
  belongs to the same repository before reuse; source aliases into another
  checkout are refused. When the first enrolled
  tip already contains captured main, it checks out that tip directly and adds
  a dedicated candidate commit. Otherwise it starts at main. Remaining tips
  merge with `--no-ff` in queue order, preserving source DAGs
  and resolution-only changes through the final citation amendment. A conflicting
  merge is aborted and the preceding candidate verified before proceeding;
  a failed restoration stops the runner. The conflicting lane leaves with a report. The runner resolves change ids, takes the repository integration
  lock, runs the union of the batch's selected gates once, pushes and
  confirms. A failed batch splits into halves: a passing first half lands,
  and the rest retest on the new tip. A single failing lane leaves the queue
  with its log. A move of main rebuilds the candidate; it never needs a
  person. Speculation is a candidate chain (main + L1, main + L1 + L2, ...)
  that runs as far as the admitted capacity allows: one chain on this Mac,
  wider on the farm.
  Direct materialization uses Git's
  [`merge-base --is-ancestor`](https://git-scm.com/docs/git-merge-base) predicate
  and ordinary [`commit --allow-empty`](https://git-scm.com/docs/git-commit),
  including its hooks. The candidate's tree and parent must remain exactly the
  enrolled tree and tip; a hook that changes either stops admission. This keeps
  citation amendment off the source commit without the old main-then-lane
  checkout, which rewrote byte-identical Rust inputs on every failed retry.
  Unchanged inputs and ignored build products stay in place; their timestamps
  never establish compiler-artifact admission. No gate result is reused.
- **Cost.** N small lanes pay the fixed gate cost once, not N times. A bad
  lane costs O(log N) extra runs. The runner is one process per batch; it
  waits for the integration lock with the lock authority's 2 s retry and
  checks main after the queue drains, and exits after that health run. `land`
  starts the next runner; a pending request interrupts the health run.
- **Status.** Batches, bisection, conflict exclusion and the main-moved
  rebuild are implemented and tested on real repositories (2026-10-20). The
  candidate chain runs one batch at a time on this Mac; parallel speculation
  needs the farm's cells (Migration item 3).

### 6. Ledger rows merge by ID

- **Problem.** Most lanes end with a ledger commit, and two lanes that each
  add a row at the top of one section conflict as lines. On 2026-10-21 the
  queue left two of three lanes out of a batch for exactly that, and each
  needed a manual rebase.
- **Prior art.** Git's merge drivers (`gitattributes(5)`): Debian's
  `dpkg-mergechangelogs` merges changelog entries by version, and lock-file
  drivers merge packages by name rather than by line.
- **Design.** `.gitattributes` names `merge=molt-ledger` for
  `docs/agent/V1_HANDOFF_FINDINGS.md`. `tools/install_git_hooks.py` registers
  the driver in the common Git config, so checkout provisioning installs it
  for every worktree and the queue's sandbox. `tools/ledger_merge.py` keys
  each findings row by its first cell: a row one side changed or moved takes
  that side's version, a row one side added goes after its predecessor there,
  a row one side deleted stays deleted, and a row both sides changed is a
  conflict. A family row of the dependency order is keyed by its letter:
  since every lane that opens or closes a row edits one, its name and cause
  merge cell by cell and its key list merges as a set; on 2026-10-29 one such
  edit on each side rejected the graph-selection lane. Generated projections
  (`tests/findings/finding_ids.py`, `docs/agent/PROOF_PLAN.generated.md`) are
  not merged here: the driver stays stdlib-only, and the generated-merge
  resolver below regenerates them after source paths merge. Prose
  that both sides changed falls back to `git merge-file`, as does a checkout
  without the driver.
- **Cost.** One parse of the three 1.1 MB versions per merged ledger: 70 to
  80 ms through `uv run --no-project --offline`, measured 2026-10-21.

### 7. Generated projections merge after sources (HF-654)

`tools/generator_manifest.toml` owns generated paths and reproducibility.
`tools/generated_merge.py` generates `.gitattributes` from its fixed `outputs`:
rows with `ci_checkable = true` (the default) and `idempotent = true` use
Git's built-in `merge=binary`. Both-sided changes keep our bytes and an
unmerged index entry; no custom driver or hook can silently turn them into
a textual merge. The other rows retain textual merging.

The landing runner calls `resolve_generated_merge` only when Git fails.
Workers use the same resolver through their selected image:

```sh
python -B tools/generated_merge.py merge origin/main
```

When every unmerged path has an eligible manifest owner, the resolver runs
the affected producers in dependency order over the merged source tree,
including declared downstream consumers. It uses the checkout's selected
development image; the generator runner refuses a different interpreter or
project Python pin. Each producer renders once. Publication uses the same
byte comparison as `--check` against that retained render, then the resolver
stages the complete produced families and commits with Git's merge message.
The ordinary generated check gate still qualifies the resulting candidate.

Any source conflict leaves the merge and its index unchanged. An excluded
generator conflict names its producer and manifest reason. Generator errors
leave the merge pending with the original error; workers may fix sources and
rerun the resolver, or abort the merge. Unstaged edits outside the conflict
set are refused before regeneration, so they cannot enter a projection.

A clean Git merge invokes no resolver, generator or added Git process.
A generated conflict pays one generator-runner process for all selected
producers, plus bounded Git inspection, staging and commit. The worker command
itself is the explicit coordinator; no background process or installed hook is
involved. Git's [binary merge contract](https://git-scm.com/docs/gitattributes#_performing_a_three_way_merge)
defines the deferral boundary. Tests in `tests/tools/test_ledger_merge.py`
exercise real temporary repositories and the committed generated bytes.

### 8. Ledger lanes first, one ledger writer in flight (HF-711)

- **Problem.** Ledger-only lanes are cheap but waited behind code lanes in one
  FIFO queue. On 2026-11-02 `ledger-cut-20261102` landed in 13 min and
  `dag-current-20261102` in 16 min of gate time, while `ledger-20261102` sat
  behind code lanes whose attempts take 47 to 268 min. Concurrent ledger
  writers also caused 4 ledger conflicts that day.
- **Prior art.** bors-ng takes a priority with each approval (`r+ p=N`) and
  tests higher priorities first; Zuul gives each pipeline a `precedence` for
  its resource requests. Both keep one queue and change its order.
- **Design.** The class of a lane head is computed from Git, never stored:
  `head_class` (`tools/molt_dev_queue.py`) diffs the head against its merge
  base with main (`git_identity.merge_base`, `changed_paths`), and
  `landing_class` (`tools/molt_dev_policy.py`) returns `LEDGER` when every
  changed path is in `LEDGER_PATHS` and there is at least one, else `CODE`.
  `head_closes_rows` is true when a row open at the merge base is not open at
  the head (`finding_status.project`); a missing ledger at either revision
  raises `DriverError`. Before each batch the runner classifies the ready
  requests (`classify_ready`). Queue order: `LEDGER` requests that close no
  row first, then every other request (`CODE` and closing `LEDGER`), each in
  enqueue order. A closing request keeps its place, so it cannot overtake a
  code request that still cites the row, and no citation scan is needed. A
  fresh batch is the first request in this order and the contiguous run of
  same-class requests behind it, up to `MAX_BATCH`; a bisection plan's group
  never consults the order. `next_batch` stays free of Git: it takes the
  precomputed `RequestOrder` map.
- **One writer.** Only the integrator writes ledger-only lanes; a code lane
  that fixes a finding still closes its own row in the same change. `land`
  refuses, immediately before it queues (also with `--no-precheck`), a
  `LEDGER` head without the ledger (`ledger-missing`), then a `LEDGER` head
  while a `LEDGER` request is pending (`ledger-pending`). The check and the
  enqueue take no lock: two concurrent `land` runs could both pass, which the
  one-integrator rule accepts. The ledger merge driver stays for code lanes
  that close rows.
- **Failure handling.** Only a `DriverError` from classification is caught:
  that request records `failed` with Git's error and leaves the queue, and
  the rest still run; if none remain, the runner returns without building a
  candidate. Any other error propagates as before. A host-wide Git failure
  records every ready request `failed`; they are landed again after the host
  is fixed.
- **Checks.** A ledger lane runs the always gates (among them `python
  tools/finding_coverage.py --check`) plus the rule `finding-ids-static`
  (`python -m ty check src tests/findings`), so a closed row fails every
  finding test that still cites it.
- **Cost.** One `merge-base` and one `diff` per ready request per batch, plus
  two ledger reads and projections for each `LEDGER` request (about 14 ms per
  projection of the 1.7 MB ledger, measured 2026-11-02).

## 9. The fast landing (`land --fast`, HF-701)

The integrator's only push to main. It tests the exact merged tree on a
mechanically computed affected set, pushes it with a lease, and leaves a
receipt that the commit message and the pre-push hook read.

### 1. Problem, measured on 2026-11-04

- **Main went red.** The integrator landed HF-205 by hand: lane tests, a
  merge of `origin/main`, `land --static-only`, grep-selected Python readers,
  then `git push`. Nobody ran the touched Rust crates' tests on the merged
  tree. Two `molt-backend-native` lib tests then failed on main. The repair is
  `469dda571e`; its message records "failed under the dev and dev-fast
  profiles and passed under release-fast".
- **Local selection runs fewer Rust cells than CI.** The `backend-native`
  rule (a queue-batch gate, `tools/proof_plan.toml` "Native (Cranelift)
  backend Rust") tests `molt-backend-native` only under `release-fast`. CI's
  `rust.test.compiler-authorities` tests it under `dev-fast`. Debug assertions
  select `HardenedPlain` refcount storage, so the two HF-205 tests fail only
  under `dev`/`dev-fast`. Even the full queue batch would have missed it.
- **Features.** `cargo test -p molt-backend-wasm --lib` runs 0 tests: the
  crate's default features are empty, and its tests need `test-util`
  (`086fbc6f2f`'s message: "molt-backend-wasm with --features test-util 357
  passed").
- **Hand-picked tests.** HF-764's worker ran 56 focused `molt-runtime` tests,
  not the crate. Two `molt-runtime` lib regressions surfaced at a later merge.
- **The precheck is waived.** 11 of 27 landings since 11-01 waived the try run
  (integrator's count; landing subjects such as `5145540938` "with the try run
  waived" and `db54a2f968` "with the precheck waived" show the pattern). One
  retained receipt shows why:
  `/Volumes/VertigoDataTier/Molt/evidence/integration/9746bff8a580e4ac/03ffde220d54dcc906d763cbfb06f0157a7ce54e.try-run.heavy.json`:
  changed-crate Clippy 510.6 s, one `tests/cli` pytest cohort 599.1 s until
  the 900 s budget cancelled it, 12 gates never started.
- **The landing commit text is hand-written.** `086fbc6f2f`'s body lists what
  the integrator ran from memory. Nothing checks it.

Root cause: the push to main is a manual act with no mechanical link to a
tested tree, and the local Rust test selection is not derived from the cells
the plan declares for CI.

### 2. Prior art

- **The "not rocket science rule"** (Graydon Hoare): automatically maintain a
  repository of code that always passes all the tests. bors and homu (Rust)
  test the exact merge of an approved change onto main and push exactly that
  commit. Zuul gates the speculative merge. GitHub merge queues and Chromium's
  commit queue require checks on the merge result, not the branch. bors pays
  for this with queue latency (one change at a time); this design keeps that
  serial shape for the integrator's own landings and bounds latency by testing
  only the affected set.
- **Affected-target selection.** Chromium's CQ and Google's TAP run the
  targets a change reaches. Bazel answers it with `rdeps(universe, set)`.
- **In-repo precedent.** `tools/check_changed_clippy.py` (HF-318) lints the
  Clippy cells the plan declares, for the Cargo packages a change reaches.
  This design gives lib tests the same shape.
- **Compare-and-swap publication.** `git push --force-with-lease=<ref>:<expect>`
  publishes only when the remote ref still holds the expected value.

### 3. What already exists

| Fact | Canonical owner | Use |
| --- | --- | --- |
| Changed paths since a base, reach, reverse deps | `check_changed_clippy.changed_paths`, `Workspace`, `dependency_reach(workspace, paths, read_base)` | reused; the workspace manifests give the same path-dependency edges `cargo metadata` reports |
| Cargo argv parsing | `command_admission.parse_cargo_invocation` | reused |
| Gate selection, static vs try run | `select_landing_gates`, `SelectedGate.static/.try_run` | reused |
| Python direct-reader cohorts | `tools/test_dependency_graph.py` presubmit mode | reused unchanged |
| Repairs before gates | `repair_landing` | reused |
| Change-id citations | `molt_dev_change_ids.resolve_change_references` | reused |
| Gate execution, guards, logs | `run_selected_gates`, `GateRuntime`, `gate-NNN.stdout.log` | reused |
| Lane precheck | `precheck_landing` | **modified**: `budget=None` means no deadline; it returns its run records |
| Generated-file merge conflicts | `resolve_generated_merge` | reused |
| Publication lock | `integrate`'s repository integration lock | reused |
| Push confirmation (fetch + ancestor) | `integrate` step 7 | reused |
| Managed hooks | `install_git_hooks.py`, `.githooks/`, `checkout_provisioning` | extended |

Retired: the hand `git push` fast path, and design item 7's sentence "No new
pre-push hook ... is added" (edited in this change).
Not touched: the queue runner, batching and bisection. `land` without
`--fast` still enqueues and remains the path for a lane whose affected set is
too large for the integrator to wait on.

### 4. Mechanism

#### 4.1 Command

```
molt_dev.py land --fast --repo <slot> --subject "<one line>" [--no-push] [--dry-run]
```

`--fast` joins the existing mutually exclusive group (`--no-precheck |
--precheck-only | --static-only | --fast`). It runs from main's pinned
controller like every `land`; this lane's own first landing uses the existing
`--candidate-controller` bootstrap.

Steps:

1. **Source.** A clean committed checkout on a branch
   (`_assert_integration_source`); a managed `lane/` branch also passes
   `validate_lane_handoff`, as `land` requires. A pending queue request for
   the lane refuses (the existing `EnrolledLaneRefusal`).
2. **Lock (publication only).** The repository integration lock
   (`integrate`'s) is taken for steps 10 and 11 only, so the queue is never
   blocked by a fast landing's tests. When the queue publishes during the
   tests, step 12 re-merges and re-tests (optimistic, as bors retries a
   batch whose base moved).
3. **Base.** `git fetch origin main`; `B` = full SHA of `origin/main`.
4. **Merge.** When `B` is not an ancestor of `HEAD`: `git merge --no-ff
   --no-edit B`. Conflicts whose paths all have eligible generated owners go to
   `resolve_generated_merge`; any source conflict runs `git merge --abort` and
   stops with the paths. No rebase: lane commit ids stay, so integrate's
   patch-id survival check (a rebase hazard) does not apply, and every lane
   commit is an ancestor of the result by construction. Main already holds
   merge commits from hand landings (`e625c9e053`); the queue's candidates
   are merges too.
5. **Repair** (`repair_landing`, may add one commit), then **citations**:
   `resolve_change_references(..., amend=False)` rewrites `change:I…` tokens
   and records them as a **new** commit (hooks run), never an amend, so no
   existing lane commit id changes. Because the lane is merged, not rebased,
   a citation of the lane's last commit stays valid (`tip=None`). `H` = the
   resulting `HEAD`, `T` = its tree. Content markers stay `integrate`
   options; `--fast` has none.
6. **Select** with `select_landing_gates(H, base=B)`. The affected set is
   every selected gate with `gate.precheck`: the static gates, and the try run
   (direct-reader pytest cohorts and `precheck = true` rule gates, which now
   include `rust-test-changed`, 4.2). Every other selected gate is **waived**
   to the queue batch and named in the receipt and the commit message.
   The try run used to drop a direct reader that a matched rule's pytest list
   also ran, leaving it to the batch: a lane that changed `tools/molt_dev.py`
   never ran `tests/test_molt_dev.py` before the queue. In presubmit a rule
   cohort (`pytest` or `pytest_select`) now splits: the graph's direct
   readers form a try-run cohort with the rule's own envelope and selector,
   and the rest stays with the batch, which runs both.
7. **Run** through `precheck_landing(..., budget=FAST_LANDING_BUDGET)`, and
   fail unless the result is `complete`: static gates first, then the try run under a 90 min
   ceiling (`FAST_LANDING_BUDGET` in `molt_dev_policy`, beside
   `PRECHECK_BUDGET`; the default `budget=None` keeps meaning the policy's
   900 s). Any refusal (budget or disk capacity) or failure stops the landing;
   no receipt is written. Gate guards keep their per-command timeouts.
8. **Check** `HEAD == H` and a clean status (the gate runtime already checks
   the source at every gate boundary).
9. **Receipt** (4.3) for `(T, B)` listing `H`. `--no-push` stops here and
   prints the receipt path.
10. **Landing commit.** Fetch. When `origin/main` is still `B`: `git commit
    --allow-empty -F <message>` (hooks run; `commit-msg` adds `Change-Id`).
    Check `L^{tree} == T` and `L^ == H`, then append `L` to the receipt.
11. **Push** `git push --force-with-lease=refs/heads/main:B origin
    L:refs/heads/main`; **confirm** by fetch + `merge-base --is-ancestor L
    origin/main` (the exit code is not trusted).
12. **Outcomes after the push** (the push exit code is never trusted):

    | Observation | Action |
    | --- | --- |
    | step 10 fetch: `origin/main != B` | no `L`; repeat from step 3 |
    | `ls-remote origin refs/heads/main` == `L` (confirm fetch failed or not) | landed; done |
    | `ls-remote` == `B` | lease or network refusal; stop with the evidence, `L` kept unpublished |
    | `ls-remote` is a third value | main moved: `git reset --keep H` (only when `HEAD == L`), repeat from step 3 |
    | `ls-remote` fails | stop; no reset, no retry |
    | the `L` commit fails or its tree or parent differs | `git reset --keep H` when `HEAD != H`, stop |

    At most 3 attempts. After a confirmed landing, `land --fast` ensures the
    `git-hooks` input in the lane checkout (now identical to main), which
    installs the pre-push hook for the repository (4.4 Bootstrap).

    A repeat after main moved from `B` to `B'` reruns the static gates and only
    the try-run gates the move can affect (owner, 2026-11-11; measured: one
    landing spent 6,056 s, about half of it re-running every gate after an
    unrelated landing): a pytest cohort that the move's own selection also
    picks; a Cargo gate when the reverse-dependency closures of the lane's
    paths and the move's paths share a package, or either reaches the whole
    workspace or the toolchain; every other try-run gate. A move that changes
    no path reruns none. The receipt for `(T', B')` carries each skipped
    gate's record from `B` with `carried_from_base`, and the landing commit
    says "Carried from base"; postsubmit still runs everything on main.
    Chromium's commit queue and TAP make the same trade.

Idempotence: a re-run whose `HEAD` is a commit listed by the receipt for
`(T, B)` with `B` still current goes straight to step 10. Same tree and same
base, so no result crosses inputs. `--dry-run` fetches, prints the merge it
would make and the affected and waived sets, and changes nothing.

Lane retirement after a fast landing stays the integrator's `lane retire
<slug>` (the lane is landed by ancestry); `land --fast` prints the command.

Goal (not an acceptance gate): a tools-only lane in minutes and a runtime
or frontend lane well under the 90 min ceiling on a warm target. The proof
measures three recent lanes against their retained precheck receipts. The
receipt's evidence is censored (the budget cut the cohort at 599 s), so the
numbers come from the measurement, not from it. A miss is reported with its
dominant gate; there is no waiver flag (section 10).

#### 4.2 Changed-crate lib tests: `tools/check_changed_crate_tests.py`

A sibling of `check_changed_clippy.py`, run by a new `[[rule]]
rust-test-changed` (`precheck = true`, the same globs as
`rust-clippy-changed`, gate `python tools/check_changed_crate_tests.py --base
{integration_upstream_commit}`). It runs in `land --fast`, the canonical
precheck and the queue batch.

**Reach.** `paths = check_changed_clippy.changed_paths(base)`;
`reach = dependency_reach(Workspace.load(ROOT), paths, read_base)` with
`read_base` reading `git show <merge-base>:<path>`, exactly as
`check_changed_clippy.main` does. `reach.packages` are the reached members
(touched plus reverse dependencies; all members for root Cargo inputs or a
toolchain change). `reach.projects` (separate Cargo projects such as
`deploy/browser/simd-ops-rs`) have no declared lib-test cell; the selector
prints each as unverified.

**Sources.** A source is a `cargo test` invocation in the plan: each gate
string of every `[[rule]]` (rules have no tiers: they are the local tier),
and each `[[command]]` argv whose `tiers` include `pre-push`, both parsed by
`parse_cargo_invocation`. Its packages are its
`-p` values; a pkgid `name@version` is matched by its name (members such as
`cranelift-object@0.137.0` and `zlib-rs@0.6.8` keep the spec as written in
the cell; registry crates never match a member). A source is not
runnable here when it has `--no-run`, or a `--target` that is not the host
triple; the selector prints each such (source, crate) pair as unverified.

**Cells.** For each reached member `C` with a lib target:

1. **Runner-owned:** `molt-runtime` only. `runtime_test_gate_plan.SEMANTIC_IMAGES`
   names it with no case list, so the runtime gate runs its whole libtest
   (the parallel remainder plus the fresh-process lifecycle witnesses that a
   plain `cargo test --lib` cannot isolate). The cell is the `[[rule]]` gate
   that runs `tools/run_runtime_test_gate.py` with the semantic policy (today
   `runtime-lib`, `--profile dev-fast`), run unchanged. Its own child
   accounting is the 0-test and per-test authority. `molt-runtime-core` and
   `molt-runtime-math` run only declared cases there, so they get ordinary
   cells below.
2. **Declared:** for each runnable source `S` naming `C`, one cell keyed by
   `S`'s build flags: every Cargo token except `-p` entries, target selectors
   (`--lib --bins --tests --test X --bin X --examples --benches --all-targets
   --doc`) and `--timings`. `--profile`, `--release`, `--config`, `--target`,
   `--locked`, `--no-default-features`, `--all-features` and `--features`
   stay. The libtest options kept are `--skip X`, `--include-ignored`,
   `--ignored`, `--test-threads N`, `--nocapture`; positional name filters
   are dropped, so the cell runs the whole lib.
3. **Undeclared:** when no source names `C`, one cell `cargo test --locked -p C
   --lib --no-fail-fast`: the `test` profile and default features, the same
   unit `rust.test.default-truth` runs for `C` through
   `run_cargo_test_truth.py` (`cargo test --locked --workspace --tests
   --no-fail-fast`). The wrapper itself is never invoked.

**Execution.** Per source, features are filtered first (rule below), the key
is computed from the filtered build flags, then cells with an equal key merge
their packages and libtest options (a `--skip` list is a union of exclusions
the plan declares). Each cell runs once:

```
cargo test <build flags'> -p <C...> --lib --no-fail-fast [-- <libtest options>]
```

The cell is lib-only by declaration: it is the whole-crate lib proof the
worker contract names, not a replay of the source. The source command stays
CI's (and for rules, the batch's). Features: a qualified feature `pkg/feat`
stays when `pkg` is selected in the cell; Cargo refuses a qualified feature of
an unselected member (probed: `cargo tree -p molt-backend-native --features
molt-backend/native-backend` → "does not contain this feature"). An
unqualified feature stays when the source names exactly one package. A
dropped feature never changes `C`'s own features, but it can change the
features of a dependency `C` shares, so the cell is **narrowed**: the selector
prints the dropped features, and the receipt and commit message list the
crate as narrowed in that cell. CI still runs the source verbatim. HF-205 is reproduced exactly: the `compiler-authorities` cell for
`molt-backend-native` keeps `--profile dev-fast`, its `--config`,
`--no-default-features` and the `molt-backend/...` features when
`molt-backend` (a reverse dependency) is reached.

The selector runs each cell as a child of its own process under the gate's
environment, guard and Cargo target, as `check_changed_clippy` does (it also
runs the registered `run_runtime_test_gate.py` wasi-clippy wrapper as a
child). The rule's toolchains (`python`, `rustc`, `cargo`) resolve to
shared Cargo target access, so the gate runtime admits output capacity for
the gate as it does for `rust-clippy-changed`. The runtime gate cell (rule 1)
is invoked as its declared argv.

**Accounting.** Cargo runs with stderr joined into stdout on one pipe. Cargo
runs test binaries one at a time, so each `Running unittests <src> (<path>)`
line (regex `^\s*Running unittests \S+ \((?P<path>.+)\)$`) precedes its
binary's libtest output. The path's file stem `<lib_name>-<hash>` maps to the
package through each member manifest's `[lib] name` (default: the package
name with `-` → `_`). A `Running` line that maps to no selected package
fails the cell. A selected crate whose `[lib]` sets `test = false` or
`harness = false` has no libtest summary: it is printed unverified, never
counted. A golden Cargo log fixture pins the parser. Each binary's
`test result: (ok|FAILED). N passed; M failed; K ignored; …` line is bound to
that package. The selector prints one line per (cell, crate) and one final
JSON line (`molt.changed-crate-tests.v1`: cells, argv, exit, per-crate
counts, unverified, dropped features). It fails when:

- a cell exits non-zero (a compile failure prints Cargo's error and has no
  summary);
- a selected crate has no summary in its cell;
- a crate that a **declared** source names runs 0 tests across all its cells
  (the `molt-backend-wasm` trap), unless it is in the typed
  `LIBS_WITHOUT_TESTS` list of members measured to have no lib unit tests
  (`molt-backend`: its tests live in its bins and integration targets); an
  entry that runs tests fails too, so the list only shrinks. An undeclared
  crate's 0 is printed, not failed: the plan names no tests for it.
- Each cell runs with the union of its sources' declared toolchains,
  projected through `tools/toolchain_env.py` (the LLVM cell needs
  `llvm-sdk`); a toolchain that is not provisioned stops the gate with its
  install command, as changed-crate Clippy does.

**Cost by reach class.** A leaf crate change runs its own cells. A shared
crate (`molt-ir`: 9 members) runs about 8 cells. A root Cargo input or
toolchain change reaches all 56 members: every cell, including the runtime
gate. The selector prints the cell count first; such a lane should land
through the queue. The batch's per-crate rules (`backend-native`,
`runtime-lib`) stay; in the batch they duplicate part of this gate, a known
cost to measure before any removal.

#### 4.3 The landing receipt: `tools/landing_receipt.py`

One stdlib-only module owns the schema, the store, the message projection and
the push check.

- **Store:** `<git-common-dir>/molt/landing-receipts/<tree>.<base>.json`,
  beside the queue's `landing-queue/`. Every worktree, the queue sandbox (a
  registered worktree of the same repository) and the hook share the common
  directory. Writes are atomic (temporary file, then `os.replace`).
  Retention: receipts older than 30 days retire when a receipt is written
  (the queue's `RESULT_DAYS`); a pruned receipt makes the hook refuse with
  the `land --fast` command, which re-tests.
- **Schema** `molt.landing-receipt.v1`: `{schema, status: "passed", mode:
  "fast"|"queue", base, tree, tested_commit, commits, controller,
  started_at, finished_at, selection_sha256, gates: [{phase, command, seconds,
  counts, log}], waived: [command], unverified: [text], narrowed: [text]}`.
  `selection_sha256` hashes the selected gate commands, so a hand-written
  receipt that matches no selection is detectable. `counts` is `{passed, failed,
  ignored}` or null.
- **Counts** come from each gate's `gate-NNN.stdout.log`: libtest `test
  result:` lines summed, or the pytest terminal summary (`N passed, M skipped
  …`); null when neither appears (static gates) or when gates ran on the farm
  (no local logs).
- Written only after every selected gate passed.
- **Writers:** `land --fast` (mode `fast`) and `integrate` (mode `queue`,
  `base` = its captured upstream, `commits` = its selected commit, written
  after gates and before its push; it runs every selected gate, so `waived` is
  empty). `integrate` keeps its plain fast-forward push; the hook checks the
  remote value it replaces.
- **Trust:** a receipt is an unsigned local file. It guards against mistakes,
  not against an adversary.

**Commit message.** `landing_receipt.commit_message(subject, receipt)` is the
only producer:

```
<subject>

Landed by molt_dev.py land --fast: tree <T> passed on base <B> (tested commit <H>).
Static gates: <n> passed.
Python readers: <k> pytest cohorts, <p> passed, <s> skipped.        (omitted when k = 0)
Rust lib tests: <crate> [<cell>]: <p> passed; ...                   (omitted when none)
Waived to the queue batch: <command>; ...                           ("none" when empty)
Unverified here: <text>; ...                                        (omitted when empty)
Receipt: landing-receipts/<T>.<B>.json
```

Lines wrap at 72 columns. The `commit-msg` hook then appends `Change-Id`.

#### 4.4 The pre-push hook

`.githooks/pre-push` is a new active managed hook (marker
`molt-landing-receipt-hook`; the retired drift-gate entry stays retired). For
every stdin line `<local ref> <local sha> <remote ref> <remote sha>` whose
remote ref is `refs/heads/main`, it runs `tools/landing_receipt.py check-push
--remote-ref ... --local-sha ... --remote-sha ...` through
`molt-hook-launch.sh`. A main update is refused unless:

- the local sha is not the zero id (deleting main is refused);
- the remote sha is not the zero id (main must already exist);
- `git rev-parse <local sha>^{tree}` succeeds (an unknown object refuses);
- `landing-receipts/<tree>.<remote sha>.json` exists, parses, has
  `status == "passed"`, and lists the local sha in `commits`.

A refusal exits 1 and prints, for example, `pre-push: refused main update to
<sha>: no landing receipt for tree <T> on base <B>; land through
\`molt_dev.py land --fast\``. `integrate` writes its receipt for exactly the
commit it pushes (its `selected_commit`).

Other refs (lane branches, tags) pass. Multi-ref pushes check each line. A
checkout whose `tools/` lacks the checker refuses a main push and names
`land --fast`.

**Scope (I1).** The hook guards managed checkouts. `git push --no-verify`, a
clone without the hooks, and a non-Molt client bypass it. Break-glass for a
red main stays the owner's explicit `--no-verify` push, recorded in the
ledger.

**Bootstrap.** `checkout_provisioning` installs hooks only from a checkout
whose `.githooks` matches `origin/main`, and `land` never provisions the
`git-hooks` input (`test_land_leaves_the_shared_hooks_to_lane_start`), so a
lane that changes `.githooks` lands without installing unpublished hooks; no
provisioning change is needed. The hook table (`install_git_hooks.HOOKS`)
keeps the retired drift-gate `pre-push` entry first, which removes only a
hook that carries its own marker, then the active landing-receipt entry
(tests prove both orders). A chained foreign `pre-push` hook gets its own
copy of stdin, so it cannot consume the refs the check reads. Rollout: this
lane lands `integrate`'s receipt writer, `land --fast` and the hook together.
I1 takes effect when the hook is installed: `land --fast` installs it after
a confirmed landing of a managed lane (step 12; the lane then equals main),
and `lane start` provisions it for every later lane.

#### 4.5 Worker contract

`tools/subagent_contract.py` gains `WHOLE_CRATE_TESTS`:

> WHOLE-CRATE TESTS: before reporting ready, run the whole library tests of
> every touched Rust crate and its reverse dependencies, `--lib
> --no-fail-fast`, in each cell the proof plan declares for it: `python -B
> tools/check_changed_crate_tests.py --base origin/main` computes and runs
> them, and the handoff precheck runs it too. A focused test filter is not
> this proof. When a root Cargo input reaches the whole workspace, say so and
> leave the full run to the queue.

`check_subagent_contract.py` requires it with key phrase
`--lib --no-fail-fast`; `MECHANIZES` anchors it to M05.

### 5. Invariants

- **I1.** Once the hook is installed, a push from a managed checkout
  advances `main` only to a commit `L` whose tree has a passing receipt for
  the main it replaces, listing `L`. It guards against mistakes; anything that
  can push can also write a receipt or pass `--no-verify`.
- **I2.** A receipt lists only commands that exited 0 on `T`; a failed,
  refused or cancelled run writes none.
- **I3.** A crate's lib-test cells come from declared plan commands; Cargo
  defaults appear only as the declared truth unit for a crate no command names.
- **I4.** A crate named by a declared source that runs 0 tests fails.
- **I5.** `land --fast` pushes with `--force-with-lease=refs/heads/main:B`
  under the integration lock; a moved main is re-merged and re-tested; a
  published `L` is never reset.
- **I6.** A landing commit's verification and waiver text is a projection of
  its receipt; only the `Change-Id` trailer follows it.

### 6. Consumers changed

| Path | Change |
| --- | --- |
| `tools/molt_dev_queue.py` | Modify: `--fast`, `--subject`, `--dry-run`, the fast landing flow |
| `tools/molt_dev_precheck.py` | Modify: `budget=None`, run records |
| `tools/molt_dev.py` | Modify: `integrate` writes a receipt before its push |
| `tools/proof_plan.toml` | Modify: rule `rust-test-changed` |
| `tools/check_changed_crate_tests.py` | Create |
| `tools/landing_receipt.py` | Create |
| `.githooks/pre-push` | Create |
| `tools/install_git_hooks.py` | Modify: active pre-push hook |
| `tools/check_subprocess_guard_coverage.py` | Modify: allow the hook checker's bounded raw `git` (stdlib-only, before any environment) |
| `tools/molt_dev_land_fast.py` | Create: the fast landing flow |
| `tools/molt_dev_change_ids.py` | Modify: `amend=False` records resolved citations as a new commit |
| `tools/check_changed_clippy.py` | Modify: `base_reader` shared with the crate-test gate |
| `tools/subagent_contract.py`, `tools/check_subagent_contract.py` | Modify |
| `docs/design/landing_queue.md`, `docs/ops/INTEGRATION.md` | Modify |
| `docs/agent/PROOF_PLAN.generated.md` and other plan projections | Regenerate (`tools/generators.py write`) |
| `tests/tools/test_check_changed_crate_tests.py`, `tests/tools/test_landing_receipt.py`, `tests/test_molt_dev.py`, `tests/tools/test_install_git_hooks.py`, `tests/tools/test_subagent_contract.py` | Create or modify |

### 7. Costs

- Hook: one Python start per push through `uv run --no-project --offline`
  (measured in the proof); a main push reads one JSON file and runs one `git
  rev-parse`.
- Selection: manifest and plan parsing; no Cargo process before the cells.
- Crate cells: one Cargo build per distinct cell key into the checkout's
  shared target; a warm target rebuilds only changed crates and dependents.
  `molt-backend-native` has 5 sources and up to 4 distinct keys
  (`native-callable-abi` and `compiler-authorities` at dev-fast;
  `backend-native` and `backend-llvm`/`llvm.test.lowering` at release-fast).
- `land --fast` versus the precheck: the same static and try-run gates plus
  one merge, without the budget; measured per 4.1's acceptance.

### 8. Test plan

`tests/tools/test_check_changed_crate_tests.py` (real plan and synthetic
plans): path → reach through `dependency_reach`; cell derivation for
`molt-backend-native` (dev-fast with `--config`, release-fast), for
`molt-backend-wasm` (never the empty default), for `molt-runtime` (the
runtime gate), for `molt-runtime-core` (ordinary cells), for an undeclared
crate (truth unit), for a foreign `--target` source (unverified); a golden
argv; feature filtering; accounting binds summaries to crates through lib
names, fails a missing summary and a declared 0, prints an undeclared 0.

`tests/tools/test_landing_receipt.py` (real temporary Git repositories with
the real hook): no receipt refuses; a matching receipt admits; a base
mismatch refuses; a commit not listed refuses; non-main refs and tags pass;
a multi-ref push checks each line; deleting main refuses; the message
projection is golden, and `Change-Id` follows it.

`tests/test_molt_dev.py` (real-controller fixtures): `land --fast` on a lane
behind main merges, tests, pushes `L` with the lease (`origin/main == L`);
main moved after the receipt: retry re-merges and re-tests; a failing gate
pushes nothing and writes no receipt; `integrate` writes a receipt the hook
admits.

Real landing: `land --fast --no-push --candidate-controller` on this lane,
then the parent's `land --fast`. Measurement: `land --fast --no-push` on three
recent lanes against their retained precheck receipts.

### 9. Out of scope for this change

- Python reader over-selection for `src/molt` changes: measured in the proof;
  a fix belongs to the dependency graph's presubmit policy, with numbers.
- Removing the batch's per-crate Rust rules: they duplicate part of
  `rust-test-changed` in the batch; removal waits for measurements.

#### Review

Two Elephant Goldfish rounds (fresh-context reviewers given only the design
file): round 1, design, implementation-readiness and codebase-details
reviewers (8 blockers, 20 majors in total); round 2, design and
implementation-readiness reviewers (3 blockers, 10 majors). Every blocker and
major was resolved in the text above or recorded under "Out of scope".

### 10. Expected failures and slow cases in the landing presubmit (HF-623)

Owner, 2026-11-05: "Do not turn every bug into a new registry, receipt
schema, orchestrator, or policy gate." and "Before landing, complete the
affected consumer migration and run the focused checks required to trust
that change." The landing presubmit runs the static gates and the focused
readers of the changed authorities, within about 600 s; heavy suites run in
postsubmit and at release milestones.

**Expected failures.** The typed `flaky(OpenFindingId)` marker
(`tests/findings/__init__.py`) is the only expected-failure form, as LLVM lit
`XFAIL:` lines, rustc compiletest `//@ ignore-*` directives and Chromium
[TestExpectations](https://chromium.googlesource.com/chromium/src/+/HEAD/docs/testing/web_test_expectations.md)
keep theirs in the tree, reviewed like code and tied to a tracking issue.
Collection refuses a marker whose finding is not open, so the marker expires
with its row. Presubmit skips a marked test; postsubmit's `--run-flaky` runs
it with normal pass and fail. There is no per-landing waiver flag, no waiver
receipt field and no audit tool: the marker is in the tree, so
`git grep -n "flaky(" -- tests` is the audit. A per-landing `--waive` design
was reviewed twice and retired by the owner before any code landed.

**Slow cases.** The `slow` marker (pyproject.toml) already names the tests
that exercise long toolchain paths, and the CI fast cells deselect them.
Presubmit runs no `slow` case: every selected file's cohort passes
`--skip-slow`, and `tests/conftest.py` skips its `slow` cases, even in a test
file the lane edits. Owner, 2026-11-06: presubmit stays within minutes and
postsubmit on the farm owns the slow cases (revert on a new failure). Readers
and edited files used to run whole, and the linked WASM end-to-end cases (one
`molt-wasm-host` peaked at 9.8-11.0 GB on main and on the batch alike) failed
landings on the per-gate memory slice, not on the change. A skip, not
`-m "not slow"`, keeps a cohort whose cases are all slow from ending with
pytest's exit status 5. Postsubmit keeps each owner's selector and runs every
slow case. The provisioning cases that publish real Python images
(`PUBLISHES_IMAGES` in `tests/test_checkout_provisioning.py`, 22-214 s per
case in landing gate logs) are now `slow`; `repository.docs-tests` declares no
selector, so CI coverage is unchanged.

Measured on this host: over the last 36 first-parent commits on `main`, the
presubmit graph selected a median of 3.5 and a p90 of 101 test files (914 at
most, for `src/molt/cli/__init__.py`); 2.5 and 22 of them (89 at most) import
the change. A marshal change selects 90 files and runs one whole; the
image-publishing provisioning cases now skip there and still run when
`src/molt/checkout_provisioning.py` changes.

**Importers only (owner decision, 2026-11-06).** Presubmit runs the changed
tests and the tests that import or run a changed non-stdlib source
(`DirectReaders.importers`). A test that reads a changed path as data, through
a directory or by compiling a changed stdlib source is a data reader: presubmit
leaves it to postsubmit, which selects it through the whole graph. Over 36 main
commits this moves the p90 from 101 to 22 files (89 at most). Two exceptions
keep a data reader in presubmit: a recent postsubmit failure, and a reader that
a curated rule runs anyway. A data reader with no landing envelope (a build
cell, or no cell) keeps its `deferred` entry, because postsubmit cannot run it
either. The landing receipt names the readers left to postsubmit
(`postsubmit_only`, part of `selection_sha256`); the commit message counts them.
The trade-off is accepted: presubmit no longer runs data-authority readers (for
example the readers of `tools/proof_plan.toml`); `tools/generators.py check`
still runs as a static gate. Landing attempt 16 (a combined candidate) had
selected about 1,000 files and stopped twice on host memory pressure. Postsubmit
must still prove that every left reader ran before last-green moves (next
step).

**Definitions, not files (2026-11-12, HF-623).** An importer is selected when
its first hop reaches a definition the change touches, not any part of a
touched file. The graph already holds one node per top-level definition and
one for module-level code (`path::name`, `path#init`); a changed module that
parses on both sides is refined to the definitions whose syntax differs, plus
those that name a changed module-level binding, closed over the module's own
callers (`changed_module_nodes`, `DependencyGraph.changed_nodes`). A change to
module-level code selects the tests that import the module themselves; tests
that reach it only through a conftest run it at import the same way, in
postsubmit. A test that holds the whole module (an attribute call the graph
cannot resolve), a new or deleted module, a module that does not parse, and a
module-level statement with an import-time effect beyond binding names stay
file-level. The consumer gate refines against `HEAD`; a landing against its
upstream (`GateConfig.change_base`). Measured on the venv_exec bootstrap change
(`dx.py`, `checkout_provisioning.py`, `molt_dev.py`, `venv_exec.py`): 992 test
files file-level, because the root conftest uses two `dx.py` functions, and 60
refined. Postsubmit still selects every dependent through the whole graph.

## Costs

Commit metadata comes from one typed, NUL-framed Git history read: original
identity, parents, subject, Change-Ids and optional patch equivalence. Integration
retains the selected range's immutable records for its subject display instead
of starting one Git process per commit. Representation checks obtain parent
counts and native patch-equivalence marks from a right-only symmetric-difference
walk, replacing separate merge enumeration and cherry processes. The chosen
merge-base window still bounds Change-Id matches, and merges still require
actual ancestry. This uses [Git's native history and cherry comparison](https://git-scm.com/docs/git-log)
with Python orchestration, following [Bazel's amortized worker execution](https://bazel.build/remote/persistent)
and [Ninja's minimum incremental work](https://ninja-build.org/manual.html).
The selected immutable records are reused only within that operation; mutable
ref and source fences remain fresh. Subject rendering changes from O(commits)
processes to one history read. Patch comparison now occurs during that walk even
when Change-Ids could suffice, an explicit trade whose graph cost must be
included in latency measurements.

Real-controller tests share one immutable committed driver history per pytest
session across xdist workers and the driver/claim families. The existing
`shared_session_tree` lock publishes it once, following
[pytest-xdist's session-fixture guidance](https://github.com/pytest-dev/pytest-xdist/blob/master/docs/how-to.rst#making-session-scoped-fixtures-execute-only-once).
Driver, generic integration and claim fixtures copy prepared repository pairs
through `copy_git_fixture` with an explicit `GitFixtureRole`. Seed publication
uses a sibling-relative remote so atomic staging renames preserve it. A
materialized subject binds its canonical absolute private origin, so linked
worktrees at another depth share a valid remote. This
keeps source, Git objects, indexes, refs and config private, without alternates
or cross-test hardlinks. Generic fixtures publish their tiny prepared pair once;
driver and claim families share the committed driver history. Actual
fetch/rebase/push, pinned-controller selection and retirement still execute in
the subject tests. Like
[LLVM lit's per-test output directories](https://github.com/llvm/llvm-project/blob/main/llvm/utils/lit/lit/TestRunner.py),
mutable test outputs remain separate from shared inputs. Tests of Git clone
itself and lane-provisioning fixtures retain their distinct setup contracts.

Copying an index also copies its donor stat records. The canonical copy now
refreshes that private index once through `git update-index --refresh`, before
read-only source fences. Those fences intentionally disable optional index
writes; an unrefreshed copy otherwise rehashes unchanged files on later reads.
Regular worktree files receive fresh mtimes, as Git checkout would give
them. Preserving old donor mtimes plus a refreshed index can hide an edit
inside a coarse ctime tick; fresh checkout mtimes preserve Git's racy-file
check. Git metadata keeps its copied times. The refresh retains Git's content
comparison and ctime checks. A real-Git
control changes a tracked file without changing size or mtime, then requires
the ordinary source observation to reject it. Config/ref and direct object-byte
mutations also prove isolation beyond Git's atomic ref replacement. Each
published seed's HEAD, tracking main and remote main agree before a subject
can repair stale state.

The cold cost remains one driver indexing/commit and tracking-ref preparation.
Each subject copies private metadata and shares blocks where supported,
otherwise copies bytes, rewrites one remote and refreshes one index. On macOS
arm64, three alternating comparisons including setup and two real source
observations measured medians of 1.019 s for cloning and 0.894 s for the
fresh-mtime prepared copy, including its one index refresh. Subsequent source
observations took 78–79 ms for clones and 79–91 ms for prepared copies; the
saving was in preparation. These three-pair fixture-cost observations do not
establish a whole-gate or compiler-throughput improvement. Other host
performance cells remain unmeasured. Git commands retain the existing executor,
state-use custody and mutation/transfer guards.

- A candidate that selects every family runs the whole pre-push tier. On
  this Mac the calibrated Python cells take 20 s to 601 s each (2026-10-17);
  the slowest, `python.unit.surface-contracts.macos`, now exceeds its 600 s
  budget (HF-263). The makespan is the slowest cell, not the sum, when the
  executor overlaps commands within the host's capacity.
- Enqueueing returns at once, transferring managed lane custody. Retirement
  adds Git identity/status checks and a metadata-only ignored-path census; it
  checks the finite provisioning inventory without spawning provisioners or
  reading artifact payloads. This work is cold and scales with checkout path
  metadata, not per-user executable work. A held lane remains recoverable and
  its terminal result identifies the remaining integration-owner action.
- Local presubmit and postsubmit qualification remain distinct from hosted
  release acceptance. Their wall time must be measured against real lanes.

## Hosted evidence handoff

The existing classifier publishes its canonical selection as
`molt.proof-selection.v1`, separately from executable receipts. The artifact
binds the exact tested commit/tree, proof authority, declared tier and GitHub
run/attempt. Its changed paths or full-plan mode reconstruct the complete
family selection; received receipt files never determine the required set.
The same `family_outputs` projection continues to feed CI job outputs and its
aggregate verdict. This adds transport for an existing fact, not a second
selection registry or scheduler.

`ci_triage.py --verify` joins that artifact with the existing receipt verifier
and strict ZIP reader. It pins a completed successful run, retrieves immutable
artifact IDs and digests, verifies source and coverage, then rechecks the run
and source before reporting success. Unique earlier-attempt receipts can
prove the same unchanged source; duplicates remain errors. A missing current
classifier is unavailable, including a partial rerun that retained its older
classifier. No timestamps, newest-artifact choice, automatic rerun or checkout
mutation substitute for evidence. CI lifecycle and coverage are separate
results, and neither grants installed-release acceptance.

This follows [Bazel's structured build-event contract](https://bazel.build/remote/bep):
announced work and its completion must be joined programmatically. It retains
Molt's existing proof plan and receipts rather than introducing Bazel or a
second execution system. [rustc's supported workflows](https://rustc-dev-guide.rust-lang.org/building/suggested.html)
likewise keep routine development behind maintained project commands.

Ordinary triage keeps its existing lazy startup and completed-log cache.
Explicit verification costs one paginated artifact inventory, one streamed
download per selected artifact, bounded archive validation/extraction and the
existing evidence hash checks, plus source/run fences. Time and storage scale
with downloaded evidence bytes; no compiler, environment publication or proof
execution occurs. Failed downloads and receipts remain recoverable. Guard
incidents/current-test evidence are exported by one shared CI action; no
additional profiler or per-test write is introduced by this handoff.

## Migration

HF-373 lands in this order, one slice per landing: the pinned controller;
stable change ids; checkout provisioning and lane commands; the queue with
batches and bisection, which replaces every private landing wrapper.

1. Local gate blocks and command receipts use the existing proof executor.
2. The queue on this host uses typed presubmit/postsubmit selection and health
   receipts; cross-host qualification remains separate.
3. Linux and Windows cells on the farm.
4. Rollups and bisection are implemented. Result reuse remains deferred under
   the observed-input design.

## Executor-owned landing budget and stall evidence (HF-623, HF-779, HF-780)

The 2026-11-05 host sample found 15 stalled-child failures in 174 landing
try runs: nine managed-Python commands at 120 seconds, five fresh executable
launches at 10/60 seconds and one guardian acknowledgement at 30 seconds.
Four pytest gates each sizing against all 18 host CPUs could start 72 workers.
The operator cap of three avoided that oversubscription, but a capped try run
took 1423 seconds against its 900-second deadline. These are the motivating
measurements, not qualification of the changes below.

The proof executor owns one CPU budget for its batch, using the existing
`molt.host_capacity` authority: affinity/quota, without the efficiency cores
on macOS (until 2026-11-14 it took only `hw.perflevel0`, 6 of an M5 Max's 18
cores, so every batch ran its pytest gates at `-n 3`). It divides that budget among the possible concurrent pytest gates
within their resource limits. Every pytest gate reserves at least one CPU,
including serial `-n 0`; a one-CPU host serializes pytest sessions. The runner
bounds each share by its existing memory slice and declared per-worker memory,
then passes an explicit `-n`. Receipts retain declared and executed argv,
worker count, wall duration and child CPU usage from the guard's sole reaper.
No operator worker override or cross-process jobserver owns this allocation.
Non-pytest build commands retain their existing toolchain resource policies.

Keep deadlines, not a progress-based hang detector: CPU silence cannot tell
blocked I/O from a hang. Managed-Python consumers reuse the existing 900-second
identity-probe bound, rather than imposing an outer 120-second deadline.
Fresh native launches and guardian handoffs use their owning named bounds,
with the measured scan/host-wait cause beside each definition. The try-run
budget follows the owner's 2026-11-05 19:50Z correction: a 600-second presubmit
p90 target, met by selecting actual readers rather than allowing longer runs.
Failure history prioritizes readers without admitting unrelated failures.
The enclosing cancellation still wins over a child deadline or retry. Diagnostic
and cleanup grace remain bounded. A reader that the change reaches only
through its pytest harness or its data runs its fast cases and leaves its
`slow` cases to postsubmit (section 10 of the fast landing). Such readers can
still number in the hundreds for a broad change, so the full presubmit wall
time is not yet qualified from receipts. It does not promise admission against
foreign processes or prove that Gatekeeper caused a particular wait.

D1's `flaky(OpenFindingId)` quarantine is distinct from strict expected failures:
collection validates the open identity, presubmit skips with its finding reason,
and postsubmit passes `--run-flaky` and executes normally. It is the only
expected-failure form (section 10 of the fast landing). A slow test is not a
flake; no current test is quarantined without evidence. `ci_triage` owns the
exhaustive infrastructure-retry classification. Only a typed launch resource
failure (EAGAIN or ENOMEM) before payload execution can receive one retry.
`Popen` may fork and reap a child whose exec failed, so this is not a claim
that no child was ever born; pre-exec CPU usage is unknown. Its controlled
pre-exec hook only sets limits and writes its startup pipe. See
[CPython's launch exception contract](https://docs.python.org/3/library/subprocess.html#exceptions).
Source drift, cancellation, product failures, timeout alone and unresolved
custody do not qualify. Both attempts retain separate streams and classified
records; they share the enclosing try-run bound. There is no retry ladder.

For a timed-out direct Python child, enable CPython's `-X faulthandler` and use
its fatal SIGABRT dump before the existing tree termination. This is a timeout
termination signal, not a harmless diagnostic request. Send only to the owned
root under the sole reaper's signal lock; do not signal arbitrary descendants.
The flag is part of the launch: as with `subprocess.run`, the result's `args`,
a timeout's or failure's `cmd` and every receipt's executed argv name the
launched argv, while a repro command names the request, because the guard adds
the flag again. An exec, disabled handler or interpreter not yet initialized can
leave the dump unavailable. Windows has no POSIX signal dump. Output tails stay attached to the
timeout exception and its diagnostic; incident profiles follow the run's durable
evidence directory, outside reclaimable scratch. Never let diagnostic collection
block the guard's independent deadline backstop.

`molt doctor` reports a fresh directly executed shebang script's first and warm
launch durations on macOS, along with the limited read-only Developer Tools
policy observation available on that host. Timing indicates first-exec overhead,
not proof of a native Gatekeeper scan; per-app permission may remain unknown.
It links the owner instructions and never changes security settings.

The implementation must migrate executor callbacks, both command runners,
receipt projections, timeout readers and test witnesses together. Unsupported
shell pytest programs refuse before spawn; recognized argv wrappers use the
one proof-command parser. Independent tests observe simultaneous allocations
on 1/2/6/18-CPU hosts, actual child argv, memory pressure, serial mode,
retry refusal/attempt retention, a real timeout's named blocked frame, and
flaky collection versus postsubmit execution. Native/WASM program semantics
are unaffected. Worker precheck is distinct from parent-owned landing under
worker load.

Design review, 2026-11-05: two sequential isolated read-only Goldfish reviews
(design and implementation readiness). The first identified seven gaps;
dispositions tightened diagnostic signal custody, launch retry eligibility,
enclosing-budget precedence, serial/memory floors, supported wrapper parsing,
probe interpretation and evidence required to quarantine a test. The second
identified a shell-budget escape; unsupported shell pytest now must refuse.
No design finding remains unresolved. Full design and review records belong
in the lane's external evidence, not tracked per-run logs. Primary precedents:
[pytest-xdist explicit workers](https://pytest-xdist.readthedocs.io/en/stable/distribution.html),
[Ninja job pools](https://cmake.org/cmake/help/latest/prop_gbl/JOB_POOLS.html), and
[CPython faulthandler](https://docs.python.org/3/library/faulthandler.html).
