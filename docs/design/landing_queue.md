# Landing queue

Status: design, 2026-10-17. Owner: landing and main health. Part of HF-334.

Main advances only to a commit that passed its selected proofs on every
required cell. One queue on the owner's own machines enforces that rule; no
agent pushes to main by hand.

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
- `tools/molt_dev.py integrate` selects landing tests from 58 hand-written
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

1. Landing receipts, and landing gates run through the proof executor
   (HF-334 slice 3).
2. The queue runner on this host, with the plan's family selection for the
   macOS and host-independent cells.
3. Linux and Windows cells on the farm.
4. Rollups, bisection, and receipt reuse by command inputs.
5. The pre-push hook; delete the `[[rule]]` tables and their projection.
