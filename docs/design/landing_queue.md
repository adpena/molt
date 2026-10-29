# Landing queue

Status: design, 2026-10-17; landing engineering added 2026-10-20 (HF-373).
Owner: landing and main health. Part of HF-334.

Main advances only to a commit that passed its selected proofs on every
required cell. One queue on the owner's own machines enforces that rule; no
agent pushes to main by hand.

The current local driver implements immutable candidate custody and
selected-volume readiness through the existing proof plan. Unfiltered pytest
targets share one invocation using the largest declared owning envelope.
The driver still uses local rules. Its always gates run as one block through
the proof executor, which leaves a receipt per landing; rule, pytest and Cargo
gates still run one at a time. The rest of the scheduling migration belongs to
the proof executor and HF-334. The family queue,
cross-host scheduling and reusable action receipts below remain release work.
The [integration contract](../ops/INTEGRATION.md) owns implemented local
behavior. Input-closed action reuse requires finer declared dependencies and
hermetic execution; a passing log or unchanged command label cannot authorize it.

## Problem

Measured 2026-10-14 to 2026-10-17:

- CI main failed on 40 consecutive completed runs. The last green run was
  2026-10-14 at b5487aef3.
- One CI run (39090704454, d259ff8c1) failed in six jobs: 42 frontend tests
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
  commit passed. fe30fd315 reached main unformatted, though the always-run
  rustfmt gate rejects it (HF-338).

## Why not select tests from the import graph

Pants and Bazel select the tests a change reaches. A prototype did the same
for Molt: the static import graph of `src/`, `tools/` and `tests/`, each
test's `conftest.py` files, and Pants-style asset inference (a string literal
or `ROOT / "a" / "b"` chain that names a tracked file is a dependency).

- It selected the broken test for every break above that runs on this host
  (the uv fixture through `tools/proof_plan.toml`, the weakref scraper through
  its Rust file, the daemon test through `tests/molt_diff.py`).
- Over the last 150 first-parent commits on main, 58 reached all 990 test
  files. The others reached a median of 24 (65 when a test's own directory
  literals count as reads).
- `tests/conftest.py` alone reaches 458 modules, 202 of them in `molt.cli`
  and 82 in `molt.frontend`, so any CLI or frontend change reaches every test.

The shelved per-test result cache (HF-263) found the same density from
runtime records. Import-graph selection would only say "run everything" more
precisely for most compiler changes, and it cannot see platform-only
failures. Selection by family stays the unit; the graph can later trim
within a family once conftest imports are lazy.

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
- **Bazel** caches each test action by the digest of its inputs, so a change
  reruns only what it can affect.

## Design

1. **One rule.** main moves only to a commit whose selected pre-push commands
   passed on every cell those commands declare. The selection is the plan's
   family selection (`tools/proof_plan.py --path`), the one CI uses.
2. **One queue.** `molt_dev.py integrate` stops pushing. It runs the fast
   static gates (formatting, generators, structural audit; seconds), then
   enqueues the lane's exact commit. A queue runner on the primary host owns
   main: it builds a candidate (main plus the next lanes, rebased in order),
   runs the candidate's selected commands, and pushes the tested commit.
3. **The farm runs the cells.** Each command declares its cell. The runner
   sends the candidate to the machine for each cell over Tailscale (macOS
   here, Linux on `molt`, Windows on `bat00`), and each machine runs its
   commands through the proof executor sized by its own `molt.host_capacity`.
4. **Rollups and bisection.** The runner tests up to N ready lanes in one
   candidate, as bors rollups do. A failure tests the halves, so one bad lane
   costs O(log N) extra runs and the others still land.
5. **Receipts.** Every candidate leaves one receipt: each command's cell,
   host, duration, peak RSS and result. Longest-first ordering reads it.
6. **Speed by reuse, not by skipping.** A command whose declared inputs are
   unchanged since a passing receipt on that cell reuses the receipt. This
   needs per-command inputs finer than today's family globs.
7. **No second path to main.** A pre-push hook refuses a push to main whose
   commit has no passing queue receipt. The `[[rule]]` tables and their
   projection are deleted once the queue owns selection.

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
  `<git-common-dir>/molt/controllers/<key>`, where the key is the digest of
  those tree and blob ids. `uv sync --frozen --no-dev` builds the
  controller's environment from main's lock, and the driver runs there as
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
  branch from main in a slot worktree, writes the coordination record, and
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
  returns. The precheck runs, in the lane, the selected gates that only read
  the candidate's files (`SelectedGate.precheck`: toolchains within
  `PRECHECK_TOOLCHAINS`, not a pytest session; `tools/molt_dev_precheck.py`),
  so a lint, format, generator or policy failure costs that lane alone instead
  of a batch bisection; GitHub's merge queue likewise admits only changes whose
  required checks passed, and Zuul runs its check pipeline before its gate
  pipeline. `--no-precheck` skips it; the batch still runs every gate. It
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
  exits when the queue is empty, and `land` starts the next one.
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
  merge cell by cell and its key list merges as a set. The same driver merges
  the generated `tests/findings/finding_ids.py` by identity sets, ordered by
  `finding_status.finding_sort_key`, so the result equals the generator's
  output for the merged ledger. On 2026-10-29 both families conflicted as
  text and the queue rejected the graph-selection lane. Prose that both sides
  changed falls back to `git merge-file`, as does a checkout without the
  driver.
- **Cost.** One parse of the three 1.1 MB versions per merged ledger: 70 to
  80 ms through `uv run --no-project --offline`, measured 2026-10-21.

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
- This replaces three-day red stretches of main, each repaired by hand after
  the fact, with one bounded run per candidate.

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

1. Landing receipts, and landing gates run through the proof executor
   (HF-334 slice 3). The always-gate block and its receipt are done; rule,
   pytest and Cargo gates remain.
2. The queue runner on this host, with the plan's family selection for the
   macOS and host-independent cells.
3. Linux and Windows cells on the farm.
4. Rollups, bisection, and receipt reuse by command inputs.
5. The pre-push hook; delete the `[[rule]]` tables and their projection.
