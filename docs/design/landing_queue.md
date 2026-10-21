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
- **Design.** `integrate` and the queue runner fetch first. Then they run the
  driver from main's tip: `git archive <tip>` of `tools`, `src/molt`,
  `pyproject.toml`, `uv.lock` and `.python-version` extracts into
  `<git-common-dir>/molt/controllers/<key>`, where the key is the digest of
  those tree and blob ids. `uv sync --frozen --no-dev` builds the
  controller's environment from main's lock, and the driver runs there as
  `python -I`, so no module, dependency version or `PYTHON*` setting comes
  from the candidate. The candidate still supplies its declared data: gate
  selection from its `tools/proof_plan.toml`, and its tests. A change to the
  driver takes effect after it lands, as a stage0 bump does.
- **Escape.** `--candidate-controller` runs the candidate's driver for a
  repair of a broken controller on main. The progress stream and the receipt
  record it.
- **Cost.** Two `git rev-parse` calls and one no-op `uv sync` per landing. A
  new controller tree costs one extraction (1.5 s, 33 MB) and one environment
  build from the warm uv cache (0.8 s, 35 MB). Retention keeps the four most
  recently used controllers.
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
  to run a bootstrap script. `lane start` provisions every input at once.
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
- **Design.** `molt_dev.py lane start <slug>` creates the worktree and the
  `lane/<slug>` branch from main, writes the coordination record, and
  provisions it. `lane retire <slug>` accepts a landed lane, or an abandoned
  one after it writes a Git bundle. It removes the worktree, the branches and
  the checkout state (HF-313), and closes the record. `lane list` shows each
  lane's state. The queue retires each lane it lands.
- **Cost.** The same Git and provisioning work an agent does by hand now,
  without the forgotten steps.
- **Landed (slice 3).** `tools/molt_dev_lanes.py` implements `lane start`,
  `lane retire` and `lane list`. A lane is landed when every commit is on
  main by Change-Id or patch-id, so a replayed and amended lane retires.

### 5. A queue with speculative batches and bisection

One runner owns main. Agents enqueue and continue.

- **Prior art.** Zuul's dependent pipelines test each queued change on top of
  the changes ahead of it, in parallel. The GitHub merge queue tests merge
  groups up to a size limit with a build concurrency. bors-ng tests a batch
  and bisects a failed batch. Uber's SubmitQueue ("Keeping Master Green at
  Scale", EuroSys 2019) ranks speculative builds by their chance to land.
- **Design.** `molt_dev.py land` checks the lane and writes one request under
  `<git-common-dir>/molt/landing-queue/`, then returns. It starts the runner
  if none holds the runner lock. The runner runs from the pinned controller.
  It builds a candidate in its own sandbox worktree: main's tip, then each
  ready lane in queue order. A lane that conflicts leaves the batch with a
  report. The runner resolves change ids, takes the repository integration
  lock, runs the union of the batch's selected gates once, pushes and
  confirms. A failed batch splits into halves: a passing first half lands,
  and the rest retest on the new tip. A single failing lane leaves the queue
  with its log. A move of main rebuilds the candidate; it never needs a
  person. Speculation is a candidate chain (main + L1, main + L1 + L2, ...)
  that runs as far as the admitted capacity allows: one chain on this Mac,
  wider on the farm.
- **Cost.** N small lanes pay the fixed gate cost once, not N times. A bad
  lane costs O(log N) extra runs. The runner is one process per batch; it
  waits for the integration lock with the lock authority's 2 s retry and
  exits when the queue is empty, and `land` starts the next one.
- **Status.** Batches, bisection, conflict exclusion and the main-moved
  rebuild are implemented and tested on real repositories (2026-10-20). The
  candidate chain runs one batch at a time on this Mac; parallel speculation
  needs the farm's cells (Migration item 3).

## Costs

- A candidate that selects every family runs the whole pre-push tier. On
  this Mac the calibrated Python cells take 20 s to 601 s each (2026-10-17);
  the slowest, `python.unit.surface-contracts.macos`, now exceeds its 600 s
  budget (HF-263). The makespan is the slowest cell, not the sum, when the
  executor overlaps commands within the host's capacity.
- Agents do not wait: enqueueing returns at once, and the lane is retired
  when its commit lands.
- This replaces three-day red stretches of main, each repaired by hand after
  the fact, with one bounded run per candidate.

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
