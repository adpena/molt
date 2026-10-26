# Integration & Agent-Ops: the canonical workflow

> **Cite this document, not prompt-lore.** The integration and agent-ops
> discipline is encoded in ONE executable driver — `tools/molt_dev.py` — modeled
> on rustc's `x.py` and Go's `dist`. When a session needs to integrate work,
> salvage WIP, confirm a push, or verify a toolchain, it runs the driver. The
> driver is the source of truth; this doc explains it.
>
> **Non-goals (read first).** The driver drives **git + gates**. It does **NOT**
> replace human or code review, it does **NOT** decide what is *correct*, and it
> never force-pushes or rewrites published history. It *refuses* rather than
> guesses. A green `integrate` run means "the mechanical floor is satisfied and
> the push is confirmed landed" — not "this change is good." Review still owns
> correctness.

## Why this exists

A night of integration incidents produced a hazard inventory: rebases silently
dropping commits, push exit codes lying under the rtk/sandbox proxy, worktree
cleanup losing unpushed work, `diff`/`ls`/`stat` returning misleading output,
stale binaries getting credited for a passing test, `.venv` interpreter version
flips between `uv` calls. Each of those is fragile when handled "by hand from
memory." The mandate: *manual is fragile, brittle, error-prone, and not
canonical or world-class OSS.* So every hazard became a **machine-enforced,
fail-loud, tested** countermeasure inside the driver.

## The driver lifecycle (`integrate`)

`tools/molt_dev.py integrate` runs these steps in order. Every step is loud
(prints `==>`/`OK`/`FAIL`), idempotent, and safe to re-run. `--dry-run` plans
the whole pipeline without mutating anything. Use an exclusive clean checkout
containing the intended committed source; preserve parallel WIP in its existing
checkout. The driver checks source cleanliness and the selected HEAD at gate
boundaries. These checks are not a continuous filesystem monitor.

```
controller  →  fetch  →  rebase  →  verify-commits  →  cite  →  markers  →  gates  →  push  →  confirm  →  cleanup
```

0. **controller** — a live `integrate` fetches, then runs the driver of main's
   tip by default. It extracts `tools/`, `src/molt`, `src/sitecustomize.py`, `config/`,
   `pyproject.toml`, `uv.lock` and `.python-version` from that commit into
   `<git-common-dir>/molt/controllers/<key>` (the key is the digest of those
   trees, so a landing that changes none of them reuses the extraction), runs
   `uv sync --frozen --no-dev` from main's lock there, and execs the driver as
   `python -I` with `--pinned-controller <key>`. The candidate still supplies
   its gate policy and its tests; a change to the driver takes effect once it
   lands. `--candidate-controller` runs the checkout's own driver for a reviewed
   controller repair or controller-owned input-contract transition that main's
   controller cannot prepare. For example, a new native receiver may require
   linkage metadata that main's older SDK projection does not emit. Pin and
   review that coherent source and its controller projection first. Adoption
   runs the same complete selected gates and records `controller: candidate`;
   it grants no missing capability, skipped gate or failed-proof waiver. There
   is no automatic retry through candidate code. After the change lands,
   ordinary invocations again use main's pinned controller, whose tree identity
   now includes the new projection. `--dry-run` always runs in-process. Every
   progress event names its `controller`. The four most recently used
   controllers stay; an older one retires after a day without use.

   On Windows, the waiting Molt launcher and selected venv redirector start in
   the surviving controller cache. The pinned worker validates its physical
   controller identity, then consumes the captured original working directory
   before resolving command inputs. Repository arguments, relative environment
   paths and tool lookup keep their caller meaning. POSIX retains its exec
   handoff. The existing descendant guard binds its source before moving the
   launcher, passes the original environment without DX normalization, and
   admits managed state relative to the captured logical directory. Its
   Windows Job owns the redirector and worker through proven closure; killing
   or observing only the redirector cannot release their state. These are
   development process rules and add no work to emitted programs.
   A new Windows producer with an older pinned main that does not
   accept this private context refuses; it never retries through candidate code.
   Land the complete handoff through the unchanged POSIX controller before
   qualifying that coherent main on Windows.

   External launchers such as `uv run` can still hold their own Windows startup
   directory until the worker exits. Start worktree retirement from a surviving
   checkout and pass the target with `--repo`. Molt does not move or terminate
   external ancestors: if any still hold the target, failed Git removal keeps
   its state and reports the refusal.

1. **fetch** — require clean source, then `git fetch origin main`; pin the full
   upstream identity. `--expected-upstream <full-commit>` additionally requires
   that exact tip and prohibits rebase, preserving ownership-claim atomicity.
2. **rebase** — rebase HEAD onto `origin/main` *only if behind*. A conflicting
   rebase is **aborted** (restoring the pre-rebase HEAD) and the run fails
   loudly; it never leaves a half-applied rebase. Pin the resulting full commit
   once for markers, gate selection, source checks, push and confirmation.
3. **verify-commits (hazard 1)** — when no rebase occurs, the captured full HEAD
   must remain identical and contain the captured upstream tip. That immutable
   commit preserves its complete ancestry, including empty commits; no patch
   diffs or patch IDs are computed. Any HEAD drift fails before gates or push.
   An actual rebase computes each source commit's **patch-id** before mutation
   and verifies survival afterward: either replayed into the new branch range,
   or in the captured pre-rebase `HEAD..upstream` incoming span. The latter
   admits an independently cherry-picked patch even when replay leaves an empty
   candidate range; it never scans the full upstream history. Empty-diff source
   commits retain their explicit not-patch-tracked diagnostic. A missing
   patch-id is a **LOUD FAIL listing the dangling shas**. A dry-run requiring
   replay reports that survival remains unverified.
4. **cite** — the managed `commit-msg` hook gives every commit a Gerrit-style
   `Change-Id: I<40 hex>` trailer that survives rebase and amend. A Markdown
   document under `docs/` cites a commit of the same landing as `change:I`
   plus 8 to 40 hex digits of its Change-Id. After the patch-id check, the
   driver rewrites each citation to the rebased commit's 9-character id and
   amends the last commit, so the gates and the push see the final text. A
   citation that names no commit of the landing, names several, or names the
   last commit (whose id the amend changes) fails before any gate. Cite a
   landed commit by its commit id.
5. **markers (hazard 8)** — declared `--marker` content markers
   (`exists:<path>` / `contains:<path>::<needle>`) are checked against the
   post-rebase tree via python file ops. A mismatch fails before push.
6. **gates (hazard 10)** — the set of touched paths selects gate commands from
   the selected commit's `tools/proof_plan.toml` (change-class → gates). Always
   gates and every matching rule run. An edit to the plan file itself selects
   the rules that claim an entry it changed (`plan_entities`) and every rule it
   changed ([design](../design/landing_queue.md#selecting-by-plan-entry-not-by-the-plan-file-hf-457)).
   A non-zero gate or source/HEAD drift halts
   the run **before push**. `--extra-gate` selects an exact command already declared in this plan with
   prerequisite custody; it does not admit arbitrary commands. There is
   no gate-skipping or alternate-manifest landing route.
   Prerequisite failures are reported together. Content-resolver and version-query
   failures retain the probe command, directory, actual cause and bounded output; they do not
   fabricate an unavailable image path. Relative interpreter aliases preserve the
   selected entrypoint and canonicalize against the declared probe directory
   without changing the validated executable image.
   Before these gates, selected explicit pytest nodes are admitted with
   [pytest's own collection](https://docs.pytest.org/en/stable/how-to/usage.html#specifying-which-tests-to-run)
   under the cohort's interpreter, configuration and guard. Each unfiltered
   cohort collects only its explicit nodes; file-only targets add no
   collection process. Filtered cohorts retain their declared selection.
   Collection failure stops the candidate before compilation. Successful
   collection never replaces test execution or contributes a passing gate.
   Unfiltered requests first join overlapping directory/file/node selections,
   then group independent requests with the same prerequisites, taking the
   largest declared memory envelope in each cohort. Every contributing owner
   retains its requirements; a Cargo
   requirement on one request does not serialize unrelated Python-only work.
   Literal targets and parameter IDs retain their order within a cohort.
   Canonical repository-entry admission happens once when the policy loads;
   cohort selection uses tries without collection or filesystem probing.
   These remain ordinary gates in the existing executor: its session ceiling,
   delegated memory and single shared Cargo writer still apply. Extra process
   startup is an explicit cost to measure against worker utilization.
   This adds one guarded import pass for the selected modules, with workers
   disabled, separate collection logs and progress events. Emitted programs
   perform none of this development validation.
   Declared source, metadata and compilation gates run before the selected
   Python consumer cohorts. A stable linear partition preserves each phase's
   prior order, exact commands, prerequisite unions and de-duplication. This
   uses the existing gate/cohort declarations and replaces the Clippy-name
   special case. Already-required Cargo controls also move before consumers;
   no gate, build, scheduler or result cache is added. A stale methodology lock
   therefore refuses before a multi-minute Python cohort. Review measurement
   changes and update their existing lock before freezing a candidate.
   After node admission, the always gates run as one block through the proof executor
   (`tools/molt_dev_gates.py`). The plan's `executor_max_workers`, its
   `repository-policy` resource class and `molt.host_capacity` at one GiB per
   gate bound the block. Each gate reports in declared order, and the first
   failure in declared order is the one reported; gates declared
   after it are skipped or cancelled. A gate's nonzero exit raises
   `GateFailure` (a `CandidateFailure`); a source refusal or a stop of the
   executor itself stays a plain `DriverError`. The block leaves an executor receipt with
   each gate's duration, peak RSS and outcome at
   `<git common dir>/molt-integrate/receipts/<commit>.gates.json`. Remaining
   read-only rule checks run in declared order before the heavy gate block.
   Cargo and pytest gates then share the executor under the plan's resource
   limits and delegated memory; Cargo writers stay serialized. Every gate writes its stdout and stderr to
   `<receipt>.d/<run>/gate-<ordinal>.{stdout,stderr}.log` while it runs, and the guard's
   keepalive line shows progress. A passing gate prints one line, and its
   `gate_finished` progress event names both files; a failed gate prints the file
   paths and each stream's bounded end.
   Managed Python gates select the admitted image's read-only ordinary bytecode
   namespace before startup and disable bytecode writes. Verifiers use a
   separate fresh empty namespace; see [checkout provisioning](#checkout-provisioning).
   No gate result is
   reused: the receipt is evidence, not a cache.
7. **push** — `git push origin <captured-commit>:refs/heads/main`. Foreign Git
   hooks remain active and both rejection streams are retained. Git identity
   reads ignore local replacement refs. The push command's **exit code is
   NOT trusted** (hazard 2).
8. **confirm (hazard 2)** — `git fetch` then confirm `origin/main` **contains**
   the pushed tip via `git merge-base --is-ancestor`. Cleanup is gated on THIS,
   never on the push exit code. A non-landed tip is a loud fail; cleanup is
   refused.
9. **cleanup (hazard 3)** — with `--cleanup-worktree`, remove the worktree, but
   only when safe (see below).

Each failure raises a `DriverError` that `main()` translates to a non-zero exit
code, so `integrate` is CI-usable (exit `0` ok, `1` a check failed, `2` usage).

### Integration progress and cost attribution

`integrate` emits advisory JSON lines with schema
`molt.integration-progress.v1` into its existing stdout log, alongside human
output. Candidate selection, gate start, gate result and integration termination
retain the captured source/upstream, exact argv or shell command, declared
toolchains, gate ordinal, UTC and monotonic timings, and terminal return status.
The canonical detached runner already captures this stream; callers need no
private timing/status wrapper or additional state directory.

A missing terminal event leaves the result unknown. A successful gate event
cannot establish source stability, descendant closure, push confirmation,
release acceptance or reusable proof. Those remain owned by the integration
driver, execution custody and `tools/proof_receipts.py`. Dry-run observations
must retain their planning status; they cannot become executed-gate evidence.

This follows [Cargo's JSON message stream and build-finished event](https://doc.rust-lang.org/cargo/reference/external-tools.html#json-messages)
and [Bazel's build-event approach](https://bazel.build/remote/bep) while keeping
Molt's existing log and proof authorities. The incremental cost is two bounded
JSON records and stdout flushes per executed gate, plus candidate and terminal
records: O(gates + command bytes), with no extra process, source scan, image
hash, fsync, environment dump or retained per-gate state. No emitted program
executes this development telemetry. It establishes cost attribution, not an
integration speedup.

## The hazard → countermeasure table (as implemented)

| # | Hazard | Countermeasure in the driver |
|---|--------|------------------------------|
| 1 | **Rebase silently drops commits** (patch-id dedup vs moved-upstream) | Without replay, require unchanged captured HEAD and upstream ancestry, with zero patch-diff work. Actual replay computes source `git patch-id --stable` before mutation and verifies survival afterward. Missing ⇒ LOUD FAIL listing the dangling shas; never proceeds. |
| 2 | **Push exit codes lie** (rtk/sandbox swallow; 144-detached pushes can “succeed”) | `verify-push` / the push step **fetch** the remote and confirm `origin/<branch>` **contains** the tip via `merge-base --is-ancestor`. Cleanup is gated on that, never on the push command's exit. |
| 3 | **Worktree cleanup loses work** | cleanup **refuses** if a commit of `origin/main..HEAD` is not on `origin/main` by `Change-Id` trailer or patch-id (`landing_representation`, below) **or** tracked staged/unstaged changes exist (excluding the ignore set). `--force` requires **naming the sha** being abandoned (deliberate + auditable). The main worktree is never removed. |
| 4 | **Partial WIP salvage** (split staged vs unstaged) | `secure-wip` enumerates **all** tracked modifications from `git status --porcelain -z` (staged + unstaged + adds), honors partner-excludes, and commits them in **ONE** `WIP-RECOVERY`-marked commit scoped to an explicit pathspec (a pre-staged excluded file is left out). |
| 5 | **diff/ls/stat lie under rtk** | Every comparison verdict uses git **plumbing** (`rev-list`, `diff-tree`/`patch-id`, `rev-parse`, `merge-base --is-ancestor`, `status --porcelain -z`, `worktree list --porcelain`) captured as bytes, or python file ops (`pathlib` and owned content admission). Never a human-readable shell text tool whose output the proxy can rewrite. |
| 6 | **Stale-binary misattribution** | Each selected Cargo gate owns its declared package, feature, profile and target graph against the captured integration source. Product compiler reuse binds source/toolchain inputs and executable bytes through the existing compiler admission. A Git date, marker substring or byte difference cannot attest a different artifact; deterministic identical output is valid. |
| 7 | **.venv interpreter flips** (3.12 ↔ 3.14t across `uv` calls) | `python-oracle --python-version 3.12` resolves a candidate (uv → PATH → `sys.executable`), and **verifies `sys.version_info`** before accepting it. An interpreter that does not self-report the requested version is **refused**, never used. |
| 8 | **Content-marker verification** | `integrate --marker` checks declared `exists:`/`contains:` markers post-rebase, pre-push, via python file ops. |
| 9 | **Liveness/recovery probes** | `probe --file <p> --pid <n>` reports size+mtime (`os.stat`) and pid liveness (`os.kill(pid, 0)`) — replacing ad-hoc shell one-liners. Non-zero exit if a file is missing or a pid is dead. |
| 10 | **Gate selection by change-class** | `integrate` reads `tools/proof_plan.toml` (touched-path glob → required gates) and runs exactly the gates the change demands; `always` gates run for any change; `--extra-gate` adds more. Real `**` glob semantics. |
| 11 | **Backgrounded long-runs die silently** (the harness reaps `run_in_background` process groups at detach [exit 144]; sandboxed tool calls reap even `setsid` daemons at container teardown — both lose block-buffered output: empty log, no exit status) | `detached-run` starts a fresh `setsid` supervisor with **unbuffered IO** and an atomic state dir (`pid`/`sid`/`cmd.json`/`run.log`/`rc`); `detached-verify` is the **required second tool call** proving the daemon outlived the spawning call (`--min-age-s`), with a three-way verdict: `running` / `done(rc)` / **`died-silent`**. Spawn-and-verify in one call is structurally untrustable (teardown happens after the call returns), so the protocol is two-step *by design*. `detached-run` **never kills**: a live same-name daemon refuses; `--replace` only clears DEAD/finished state. |

## Subcommand surface

| Subcommand | Purpose |
|---|---|
| `integrate` | controller → fetch → rebase → verify-commits → cite → markers → gates → push → confirm → cleanup. `--dry-run`, `--no-push`, `--cleanup-worktree`, `--marker`, `--extra-gate`, `--expected-upstream`, `--candidate-controller`, `--ignore` (cleanup only). |
| `land` | Run the lane's static gates in the lane (the precheck), then queue this checkout's committed HEAD for the landing runner and return; start a runner unless one holds the runner lock. `--wait` exits 0 only when the commit landed. `--no-precheck` queues without the precheck. |
| `queue run` | The runner (started by `land`): land queued commits in batches through `integrate`, bisecting failures. `--candidate-controller` for a controller repair. |
| `queue status` | Pending requests, the bisection plan and recent results as JSON. `--recent N`. |
| `queue withdraw` | Close pending requests without testing them: `queue withdraw <request-id>... --reason TEXT`. Runs from main's pinned controller (`--candidate-controller` for a controller repair). Refuses while a batch runs, withdraws nothing if any id is not pending, and starts a runner when requests remain. |
| `secure-wip` | Commit **all** tracked modifications in one `WIP-RECOVERY` commit. `--dry-run`, `-m/--message`, `--ignore`, `--include-untracked`. |
| `verify-push` | Confirm `origin/<branch>` contains a tip sha by fetch + ancestor. `--tip`, `--json`. |
| `probe` | File size+mtime / pid liveness via python ops. `--file`, `--pid`, `--json`. |
| `python-oracle` | Resolve + PIN + verify a CPython version before use. `--python-version`, `--no-uv`, `--json`. |
| `detached-run` | Spawn a command as a `setsid` daemon with a state dir (hazard 11). `--name`, `--cwd`, `--env K=V`, `--replace`, `--state-dir`, `--json`, `-- cmd…`. The spawning tool call itself must run unsandboxed; verify in a *later* call using the published recovery command or JSON `verify_argv`. |
| `detached-verify` | Prove the daemon outlived its spawning call: `running` / `done(rc)` / `died-silent`. `--name`, `--min-age-s`, `--state-dir`, `--json`. Exit 0 only for `running` past min-age or `done` rc 0. |
| `cleanup` | Standalone hazard-3 worktree cleanup gate. `--ignore`, `--force SHA`. |
| `lane start <slug>` | Create `lane/<slug>` at the fetched base in an admissible slot, preferring the checkout selected by `--repo`; write its coordination record and provision its checkout inputs. Runs main's pinned controller. `--base`, `--root`, `--candidate-controller`. |
| `lane retire <slug>` | Retire a landed lane's local and remote branches and close its record. Detach a managed slot and retain its checkout state; remove a legacy non-slot lane's worktree and checkout state. Runs main's pinned controller. `--abandon` first writes a verified bundle. `--ignore`, `--candidate-controller`. |
| `lane list` | Each lane's tip, base, commits ahead and behind, landed, dirty and provisioned state. `--json`, `--no-fetch`. |

### Lanes (`lane start | retire | list`)

`lane start` and `lane retire` run main's pinned controller, as `land` does:
they fetch, then hand the command to the driver at the fetched main
(`molt_dev_controller.runs_from_pinned_controller`). Main's code provisions
every lane, including one started at `--base <ref>`, and judges every
retirement, landed or abandoned. A driver from another commit reads their
manifests with another schema (HF-468): on 2026-10-22 `lane start` run from a
lane at tool-release schema 3 failed to provision a new lane at schema 2, and
`lane retire` run from an older main judged a landed lane's Edgebox tree
stale. Where main's code cannot read a lane's state, retirement refuses
before it bundles or removes anything (`validate_lane_handoff`), so no signal
is lost. `--candidate-controller` runs the calling checkout's own driver. A
start and a retirement of one slug never run together: each holds the
slug's lock (`<git-common-dir>/molt/lane-start/<slug>.lock`), a start from
its checks through provisioning and a retirement for its whole run, and the
second refuses at once. The landing runner's own retirement of a landed lane
waits up to 60 s for the lock (`LANDED_RETIREMENT_LOCK_WAIT_S`), so a
start's checks or an operator's `lane retire` do not turn the landing into
`landed_retirement_failed`. A start that stops before its lane's record (a failed
`git worktree add`, a failed record, an interrupt) undoes what it created:
the worktree while Git finds it clean, then the branch once no worktree holds
it and while it still points at its base. A path that exists but is no
registered worktree was made by another process after the checks, so it is
kept while the branch this start made goes. When Git refuses a removal, it
keeps what Git refused and names the commands that finish by hand. The lock
covers only `lane start` and `lane retire`: a hand `git worktree add -b
lane/<slug>` at the lane's path in the same instant can still meet a start's
undo. The record names the controller that started
the lane. The record's environment wires the sccache the running driver pins
(main's), while provisioning reads the lane's own pins; the two differ only
for a `--base` lane whose sccache pin differs, and either sccache serves its
builds.

The driver owns each lane's lifecycle (design: `docs/design/landing_queue.md`,
"Orchestrator-owned lanes"). A lane is the branch `lane/<slug>` checked
out in a slot worktree. A slug is 1 to 64 lower-case letters, digits, `.`,
`_` or `-`.

- **`lane start <slug>`** fetches `origin/main` (or `--base <ref>`), then
  prefers the checkout selected by `--repo` when it is a registered, clean,
  detached, unlocked and non-prunable slot under the selected worktree root.
  Otherwise it takes the first such slot in path order. It runs `git switch --no-track -c
  lane/<slug>` in it, or adds the next slot with `git worktree add`
  (`molt.custody_layout`: twelve slots, the live worktree limit; another
  layout must pass `--root`). A slot outlives its lanes, so its path-keyed
  Cargo target, sccache keys and checkout inputs stay warm for the next lane
  (HF-491). A detached slot with changes, or a slot path that is no
  registered worktree, is skipped untouched. Locked and prunable registrations
  keep their slot numbers reserved. An active caller is not switched; ordinary
  slot selection applies. The default family is `<root>/worktrees` beside
  `<root>/molt-src`; a symlink spelling of `--repo` denotes its resolved checkout.
  This preference retains the checkout's artifact namespace, not a promise of
  cache validity: provisioning and each build still validate their inputs.
  Activating a released slot consumes one active WIP position even though it
  adds no worktree registration. Detach makes a slot available; the old lane's
  ref deletion and record completion can finish after a new lane claims it.
  It writes the record `logs/agents/lane-<slug>/coordination.json`
  in the main checkout through `tools/agent_coordination.py`, with the lane
  worktree as its source checkout. Then it provisions every checkout input
  (`molt.checkout_provisioning`): the lane's immutable development image, the Acorn
  tree, and the managed Git hooks. An inherited `UV_PROJECT_ENVIRONMENT`
  names the caller's environment, so provisioning drops it. It refuses an
  existing branch, an existing path and an active record. For a branch with
  no record (a start that stopped, or a branch made by hand) or a closed
  record, it names the Git commands that remove the branch and the
  worktree that holds it; `lane retire` of a branch with no record names
  the same commands. A provisioning failure keeps the lane and prints
  the command that finishes it.
- **`lane retire <slug>`** fetches, then accepts the lane only when it is
  landed. *Landed* is one predicate, `molt_dev_change_ids.landing_representation`:
  every ordinary commit of `origin/main..lane/<slug>` is on `origin/main`
  since the merge base, by its `Change-Id:` trailer or by its patch-id
  (`git cherry`). A merge commit requires actual ancestry on upstream: its
  resolution-only edits cannot be proved by matching a trailer or parent patches. A tip that is an ancestor of `origin/main` is the trivial
  case; a lane replayed by the queue, with its last commit amended, is
  landed too. `--abandon` accepts an unlanded lane after it writes
  `origin/main..lane/<slug>` to `<root>/drift-bundles/lane-<slug>-<tip>.bundle`
  (where `tools/drift_harvest.py` keeps bundles), verifies it and prints its
  path. A slot lane's retirement then detaches the clean slot at
  `origin/main` and keeps its checkout state; a lane recorded before slots
  existed has its own worktree, which retirement removes with its checkout
  state. Retirement binds the canonical managed record to its checkout and branch,
  refuses locked or dirty worktrees (tracked changes or untracked files, outside
  an explicitly supplied ignore set), and names each path. Automatic retirement
  additionally requires the enrolled tip and path, so later work is retained.
  Multiple registrations sharing the branch are held. Ignored payloads are
  removable inside current checkout inputs verified by
  `molt.checkout_provisioning` (lock stamp and sentinel); `land` first makes the
  trees a lane owns current, and a stale input is held with the command that
  provisions it, whatever its writer tagged. Outside those trees, paths that
  hold no signal also leave: PEP 3147 bytecode in `__pycache__`, a directory
  tagged by the Cache Directory Tagging Specification, a setuptools
  `*.egg-info` holding only `egg_info`'s files, an empty checkout-state lock,
  and a plain directory that holds only these (docs/design/landing_queue.md).
  The artifact-publication
  owner also recognizes its direct, empty persistent lock metadata, which leaves
  only with the whole checkout under retirement custody; locks are never unlinked
  to make a lane look clean. Malformed reserved leaves and other ignored notes,
  logs, or source are held for harvest and indexed preservation outside the lane. It refuses to remove
  the checkout that runs the driver or holds the current directory, and a
  remote lane branch that holds commits in neither the lane nor main. Then it
  removes the worktree through the hazard-3 cleanup (which retires the
  checkout's backend daemons and reclaims its checkout state, HF-313),
  deletes `origin/lane/<slug>` with a lease and confirms it with
  `ls-remote`, deletes the local branch with `update-ref -d <ref> <tip>`, and
  closes the record as `landed` or `abandoned`. Exact-tip/path retirement intent
  is written to the existing record before deletion, without a terminal status.
  If the final record write fails, an explicit retry verifies the worktree,
  registration, local and remote branch are all absent before recording completion.
  Existing checkout-state locks and
  process custody protect owned proofs and daemons; enrollment is not a global
  process lease and cannot protect writers that ignore the handoff.
- **`lane list`** shows each `lane/*` branch: tip, merge base, commits ahead
  and behind, landed, dirty paths and per-input provisioning state, plus the
  repository's hook state. Read-only Git plumbing (`merge-base`, `rev-list`,
  `cherry`, `for-each-ref`, `worktree list`) runs as a bounded metadata
  probe without a memory guard: on 2026-10-20, 10 lanes took 5.8 s instead
  of 26.8 s.

Cost: through the pinned controller, `lane start` pays the handoff (one
fetch, a controller extraction only when main's controller tree changed, one
no-op `uv sync`, one exec), then its own fetch, one `git worktree add` and the
provisioners. Each fetch of an unchanged GitHub origin took 0.26-0.34 s on
2026-10-22. The second fetch stays: a command can wait for a lock between the
handoff and its own fetch, so only its own fetch is current. `lane retire`
took 3.2 s through the controller against 2.3 s with the candidate's driver
(review of HF-468, 2026-10-22). `landing_representation` pays one `git log` when the tip is on
main, and at most one `merge-base`, one more `log` and one `cherry` otherwise.

### Checkout provisioning

`molt.checkout_provisioning` owns a checkout's lock-declared inputs.
`python -m molt.checkout_provisioning ensure|status [INPUT ...]` runs it by
hand from an existing bootstrap interpreter. Managed Python uses the existing
source-build publisher and schema 7 recipe at
`<artifact_root>/build-environments/development/<checkout_component>/<recipe_sha>`.
The recipe binds the selected interpreter, lock closure, dependency groups and
editable checkout metadata. `[tool.molt.dx].dependency_groups` in `pyproject.toml`
currently selects `dev` and `bench`, together with root dependencies.

Selection metadata lives in the checkout's Git control directory, outside the
image, keyed by interpreter/group intent and the canonical producer contract.
A producer contract upgrade selects a new record and image without opening or
repairing a retired generation. An explicit image remains an exact selection
and must satisfy the current contract. Reading that bounded projection does
not admit executable bytes. `ensure uv-environment` publishes a missing
generation under the existing address lock and records the selection only after
success. When an implicit selection outlives its retired physical generation,
`status` reports it absent and `ensure` republishes through that same owner.
An explicit missing image, indirect path, or present invalid image remains a
refusal. Changed declared inputs select a new recipe; published images are never
synced or repaired in place. Explicit `UV_PROJECT_ENVIRONMENT` must identify a
matching published image at managed operation boundaries. `MOLT_VENV` (or
`venv_exec --venv`) explicitly borrows an external environment without converting
it to managed ownership. There is no implicit checkout `.venv`, active-venv or
main-worktree fallback. Ordinary source bootstrap `uv sync` remains separate.
The `tools/dev.py` maintenance commands (`doctor`, `setup`, `update`, and
`validate`) honor an explicit borrowed environment without selecting or syncing
a managed image. Installation, the Python version matrix, and managed repository
gates continue to require managed ownership.

Publication derives ordinary checked-hash caches at optimization levels 0, 1 and
2 into the image's `bytecode` directory. Admission verifies those bytes against
captured source and runtime inputs; pytest rewriting remains in memory. The
verifier starts with `-B -I -X pycache_prefix=<fresh-empty-owned-prefix>` through
the shared `python_identity_verifier_arguments` authority and retains that
scratch owner through child closure. It validates the image namespace by
explicit path without changing `sys.pycache_prefix`. After admission, payloads
select `-B -X pycache_prefix=<image>/bytecode` before interpreter startup;
environment variables alone cannot establish this policy under `-I` or `-E`.
RunContext's serialized environments retain `PYTHONDONTWRITEBYTECODE` alongside
the selected cache prefix: this includes CLI and developer exports, setup
defaults, and coordination JSON, shell and PowerShell records. Ordinary Python
children consume that no-write policy at startup; isolated children still require
the explicit argv policy above.
Existing process-state leases retain the selected generation until observed
process closure, even when a newer generation is selected. Owner/use metadata
stays outside the image; [checkout state lifecycle](../OPERATIONS.md) owns
retirement. Selection or publication alone is not release qualification.
The selected capture child performs published-manifest admission before final
input verification; the guardian remains isolated from site packages. Guardian
startup and prefix lifetime follow the same fresh-namespace rule through
[existing command and run custody](../agent/PROOF_QUEUE.md).
Standalone capture probes emit UTF-8 diagnostics before package admission,
including through redirected Windows streams. Importing the facade leaves the
caller's streams unchanged.

The core `molt.source_build_environment` authority serves DX, proof capture and
CLI producers; its schema and requirement owners also live under `molt`.
Image publication uses the existing durable exact-JSON writer with literal
UTF-8. Recipe identities and admission compare decoded canonical objects, and
existing published manifests retain their original bytes.

When a controller provisions another editable checkout, publication selects
that checkout's reader and binds its source files before launch. The
existing input custody verifies their generations through publication, and the
selected process proves that its loaded package belongs to the same checkout.
The additional cost is one hash pass over the reader's source files, followed
by metadata checks.

Other checkout inputs keep their existing protocols: current stamps read the
declared locks; stale inputs run a guarded provisioner under the checkout lock
and publish a stamp only after success. Toolchain admission ensures a selected
tool's `checkout_input` (the Acorn tree), so `tools/generators.py`, the proof
executor and `integrate` provision it on demand.

### The landing queue (`land`, `queue run`)

`land` checks that the checkout is clean, then runs the precheck: the
selected gates whose toolchains only read the candidate's files (linters,
formatters, type checks, generators and policy audits; no pytest session and
no Cargo build) run in the lane against its merge base with main, through the
same selection, environment and executor as `integrate`. A failing gate stops
`land` before it queues anything. It then writes one request under
`<git-common-dir>/molt/landing-queue/requests/` and returns; the request pins
the commit. Enrollment of a managed `lane/<slug>` is a cooperative handoff:
stop editing it until the request closes. Its canonical lane record, exact
worktree and tip must match. The runner automatically retires the managed lane
through `lane retire` after publication. Ordinary branches and detached
checkouts remain intact; a lane-prefixed branch without managed custody is refused. A runner starts through
the detached-run protocol below, under a unique name, from the main worktree,
which outlives every lane it retires; the runner's OS file lock, not the
process table, decides which runner works.

The runner always runs main's pinned controller. Each batch takes the
bisection plan's next group, else the oldest `MAX_BATCH` (8) requests:

1. Materialize the candidate in the queue's own sandbox worktree
   (`<git-common-dir>/molt/landing-sandbox`). If the first requested tip already
   contains captured main, check it out directly and create a dedicated candidate
   commit with the same tree and that tip as its parent. This preserves unchanged
   source files and ignored build products between retries, while citation amendment
   changes only the candidate commit. Ordinary commit hooks run; an unexpected tree
   or parent stops the runner. Otherwise start at main. Merge the remaining
   requested tips in queue order with `--no-ff`. This preserves source history and
   manual merge resolutions, including through the final citation amendment.
   A conflicting lane is aborted back to the preceding candidate and leaves the
   batch with result `conflict`; inability to restore that candidate stops the runner.
2. Run `integrate` on the sandbox, waiting for the repository integration lock
   held by any other integrator. Its selected toolchain closures prepare only
   the inputs they require from the candidate's locks. A source-Python
   environment role selects the sandbox's development image before tool
   fingerprinting; selected non-Python checkout inputs use the same canonical
   provisioning authority. Gates without that role and without an explicit
   environment selector do not create an image. Explicit selectors still
   validate before any gate. Each launch revalidates its prepared image; the
   plan is not permission to reuse stale physical admission. The union of the batch's selected gates runs
   once; change citations resolve across the batch.
3. On publication the runner refreshes shared hooks from published main and
   retires each managed lane. Only completed retirement records `landed` with
   the published commit. A hold or cleanup failure records
   `landed_retirement_failed`, retaining that commit and an actionable follow-up;
   `land --wait` exits nonzero. Requests already represented on main take the same
   retirement path without repeating gates. A candidate failure (a gate, the committed gate policy or binding,
   a change citation) splits a batch of two or more into halves, first half
   next; a lone lane records `failed` with the reason. A moved main
   (`RebaseConflict`, `PublicationRace`) rebuilds the batch. Any other driver
   failure stops the runner with every request still queued.

The runner lands one batch per process, then re-runs through the controller
handoff, so each batch runs under the driver of the main it lands on. Before
it works, it drops inherited `MOLT_VENV`, `UV_PROJECT_ENVIRONMENT` and
`VIRTUAL_ENV`: the sandbox's gates use the sandbox's environment. Results
older than 30 days and finished runner logs beyond the newest 20 retire when a
runner starts; retirement-failure results remain as evidence. The integration
owner follows a failed retirement through the existing `lane retire` command
from a surviving checkout, after reconciling the named custody or signal. Do not
repeat successful landing gates to repair cleanup. The original failure result
remains historical evidence; the lane coordination record records completion.

New requests use `molt.landing-request.v2` and results use `molt.landing-result.v2`; the retired opt-in is not a compatibility
flag. Deployment drains the old runner and pending requests before publishing the
controller update. Any late v1 request is refused and preserved for explicit
reconciliation under the existing runner lock, never silently reinterpreted.

Each result records one status from a closed set (`LandingStatus` in
`tools/molt_dev_queue.py`): `landed`, `landed_retirement_failed`, `failed`, `conflict` or `withdrawn`.
`queue withdraw <request-id>... --reason TEXT` closes pending requests without
testing them and records the reason as the result's detail. It takes the
runner lock, which a runner holds for each batch, so it never edits a batch
under test; while a batch runs it refuses, and it withdraws nothing when any
named id is not pending. A bisection plan drops a withdrawn id. It runs from
main's pinned controller, as `land` does, because it writes queue state and
starts the runner: a `land` that probed the lock while withdraw held it
started none, so withdraw starts one when requests remain.

Who is a runner: a runner (or a withdrawal) holds the runner lock exclusively.
`land`, `land --wait` and `withdraw` probe it in shared mode
(`_runner_lock_free`), so two probes never read each other as a runner.

`land --wait` polls for the request's result, at 0.05 s doubling to 1 s. It
fails, naming the newest runner's log, when the runner lock is free, the
request is still queued and no runner may still land it. Every runner counts,
since an older one can be waiting for the lock while a newer one has exited.
A runner has finished when its supervisor wrote `rc`, or when the process its
`pid` file names is gone or is not that supervisor (its argv names the
runner's own `worker.json`, so a reused pid never counts); a supervisor that
died before it wrote `rc` stays an unreaped zombie of `land`, which counts as
gone. Only the newest runner can still be starting without a `pid` file.

### Git network transfers

Local Git plumbing has no wall-clock deadline: a deadline on a 5 ms read
measures the host, and on 2026-10-21 one failed a 47-minute batch during a
38-minute stall. A transfer (`fetch`, `push`, `pull`, `clone`, `ls-remote`,
`remote update`, `submodule update`) is bounded by its progress instead, in
one place (`tools/git_transfer.py`) that the memory guard applies to every
guarded command, so it covers Git run through `CommandExecutor`, the harness
or `HarnessExecutionContext`. A raw subprocess bypasses it, as it bypasses the
guard.

- HTTP aborts below 1 byte/s for 120 s. Settings are read as Git reads them
  (the last value of each key, with the transfer's own `-C` and `-c`); a
  tighter `GIT_HTTP_LOW_SPEED_*` or `http.lowSpeed*` stays, and a URL-scoped
  `http.<url>.lowSpeed*` keeps its effect for its URL.
- ssh runs through `tools/git_ssh_bound.sh`, which adds `ConnectTimeout`
  (the connect and the banner exchange) and server-alive probes only where
  the connection's own setting (`ssh -G`) is unset or looser. The user's ssh
  command stays as written, in Git's precedence: `GIT_SSH_COMMAND`,
  `core.sshCommand`, then `GIT_SSH`. Another client (plink, a wrapper) is left
  as configured and unbounded.
- With no terminal to reach a person (the landing runner, an agent's shell),
  a transfer never prompts: askpass programs are blanked, Git's terminal
  prompt and Git Credential Manager's interaction are off, ssh runs in
  BatchMode, and the command ends after 120 s without output (fetch, push,
  pull and clone report progress). The output bound counts only time the
  guard watched, so a host freeze that stops guard and transfer alike does
  not end the transfer. An interactive `lane start` or `integrate` keeps its
  prompts.

### The detached-run protocol (hazard 11)

```bash
# Tool call 1 (UNSANDBOXED — the sandbox is precisely what kills daemons):
python3 tools/molt_dev.py detached-run --name flip_sweep --cwd /tmp/wt_flip --json \
  -- python3 tests/molt_diff.py tests/differential/basic --jobs 4 \
     --json-output /tmp/sweep.json

# Tool call 2+ (any later call; --min-age-s proves it survived teardown):
# Run the published POSIX command, or pass the returned verify_argv directly
# to the process launcher without a shell. Do not reconstruct its state root.
# → running        keep waiting (poll again later)
# → done, rc=N     read the artifacts; rc≠0 exits 1
# → died-silent    the hazard-11 group-kill class, reported LOUDLY
```

`detached-run --json` returns `name`, `pid`, the per-run `state_dir`, and
`verify_argv`: the argument array for the required later observation. It pins
this interpreter, its isolation flag, the absolute driver, and the selected
state root through `--state-dir`, including when launch used the default.
A later call can therefore use a different temporary environment or working
directory. The landing queue's runners still live under
`.git/molt/landing-queue/runners`; state placement and lookup defaults are
unchanged. Observation does not restart the payload.

The human hint displays the same argv as a POSIX shell command on POSIX hosts
and a labeled JSON argument array on Windows. Pass the array directly to a
process launcher; POSIX quoting is not a Windows shell command. The JSON
`state_dir` names the run's directory, while the argv's `--state-dir` names its
parent, as required by the verifier. Preserve the returned handoff instead of
reconstructing either path in a new environment.

Discovered 2026-06-06 (twice in one session): a backgrounded 807-test sweep
died at harness detach with an **empty log** — block buffering ate every line —
and the `setsid` respawn died at sandbox teardown the same way. The state-dir
`rc` file is the only trustworthy completion signal; its absence with a dead
pid is a *diagnosis*, not a mystery.

Detached custody cannot protect a process from an independent host service
that sends signals. Process cancellation belongs to the supervisor that
created the child, with verified ownership and identity. Global reapers
must not infer abandonment from command-line names or checkout paths,
elapsed time, a missing TTY, pipes, or `/dev/null` stdin: these are normal
properties of healthy supervised builds. When a proof is interrupted,
correlate its recorded PIDs and signal timestamps with host service logs
before changing the payload, increasing timeouts, or repeating the campaign.

## The proof-plan manifest (`tools/proof_plan.toml`)

`integrate` selects gates by matching the touched paths of the integrated
commits against each rule's globs. `always` gates run for every non-empty
change; a rule matches when **any** touched path matches **any** of its globs;
gates are de-duplicated preserving first-seen order. Rules ship for: native &
LLVM backend Rust, TIR midend Rust, WASM host Rust, runtime/object-model Rust,
frontend & bootstrap stdlib Python, the op-kind registry, the suite-honesty and
ecosystem-compat ratchets, docs architecture, the dev tooling itself (a
self-gate that runs `tests/test_molt_dev.py`), and CI/pyproject wiring. Local
rules have one closed field set; unknown or retired policy fields fail
admission rather than becoming unused promises.

Unfiltered pytest targets from selected rules share one invocation, so
interpreter, guard and collection startup are paid once. The largest declared
owning target envelope sizes the host-derived worker count, and the invocation
receives the union of its contributing rules' declared toolchains. Literal
targets retain first-occurrence order and parameter IDs; file/directory requests
and explicitly requested children remain in one collection. Each `-k`/`-m`
selection stays separate.
The repository pytest configuration reports only the ten slowest durations
above 0.25 seconds, including setup and teardown, to expose stragglers.

After aggregate prerequisite admission, the driver checks current capacity on
the output volumes of gate owners explicitly declaring Cargo before executing
any gate. Transitive Cargo availability for read-only metadata is not a build
requirement. Python-only
and dry-run plans acquire no new disk requirement. This is readiness against
the existing floor: remaining campaign growth is unknown and no bytes are
reserved. Canonical probes before and after Cargo-capable gates retain signed
raw free-byte changes; these are interval observations, not exclusive gate
allocations. Diagnostic probe failure cannot replace the original gate result.
Actual child output admission remains mandatory. This development telemetry
provides the measurements needed for operation budgets without guessing them.

Source-extension production and proof policy have separate dependency edges.
Producer, registry and source-build environment changes select production tests
and their proof bridge and receipt consumers. Proof-plan, queue admission and
toolchain-custody changes select the bridge and receipts; they do not require
unrelated package-production tests. Mixed consumers, including source-extension
link inputs and scientific-stack Pact oracles, stay on the proof side. The three
cohorts retain the same complete test/node union when production inputs change.
Selection checks enforce the shared producer edges and reject both excessive
production selection and missing proof receivers.

This applies the declared-dependency principle from
[Bazel Skyframe](https://bazel.build/reference/skyframe) and
[Ninja's build dependencies](https://ninja-build.org/manual.html#ref_dependencies)
within the existing local rule model. Shared patterns declare edges to both
consumers; they do not add an executor, cache or inheritance scheme. Parsing
still reads one manifest, and selection uses the existing path/pattern scan.
The additional rule costs a small amount of selection work to avoid running
unrelated cohorts. CI family selection and release acceptance retain their
existing authorities. All of this work runs during development.

Scan-record consumers follow the same boundary: a leaf scanner or its test
selects its semantic owner, while shared record, gate-runner and generator
sources select the consumer union. Structural-audit sources also select their
record-replay and generator-manifest parser consumers. Editing a consumer test
does not make that test a producer for unrelated scanner or CI controls; real
CI and shared generator inputs retain their complete consumer closure. These
are explicit data edges in the current local rules. Deriving local test
selection from canonical test ownership and family inputs remains the HF-334
obligation; this partition adds no second ownership parser or receipt cache.

`tools/proof_plan.toml` owns gate commands and their prerequisites. Update the
owning dependency rule there; the driver's tests assert that the committed
manifest remains valid and selects the required consumers.

Compiler identity uses the existing owners: `backend_binary.admitted_backend_binary`
for a developer checkout, `compiler_identity.backend_build_admission` for the
selected Cargo/toolchain/feature/profile/CPU inputs, and
`compiler_identity.installed_compiler_admission` for shipped installations.
Actual program semantics use the differential and release witness authorities.
No separately supplied binary can prove the different test images built by the
selected Cargo gates, so integration has no post-gate timestamp or marker lane.

This follows [Cargo's target/profile and dep-info model](https://doc.rust-lang.org/cargo/reference/build-cache.html)
and [rustc's dependency fingerprints and unchanged query results](https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation-in-detail.html).
A commit after a successful build does not change compiler inputs; identical
rebuild output is desirable determinism. The existing content receipt must still
reject changed source/toolchain inputs and changed executable bytes even when
file sizes or timestamps are preserved. Cargo's own build scheduling remains
separate from Molt's product content admission.

Removing the obsolete diagnostic lane avoids its Git query, reference reads
and optional guarded marker process. It adds no source scan, toolchain discovery
or compiler build after the gates. This is development-side work; emitted
program behavior and runtime cost are unchanged. No latency improvement is
claimed without a matched measurement.

Shell gates project the shared Cargo environment policy before launch, including
compound commands. Selected sccache wrappers disable incremental compilation and
use a durable server temporary directory and private cache endpoint; an explicit
endpoint is preserved. Direct-Cargo detection cannot see through opaque shell
arguments, so this projection belongs at the shell boundary. It adds no work to
emitted programs.

CI proof families execute through the same manifest-owned DAG. Commands become
eligible only after their declared parents succeed; eligible commands are
admitted in canonical manifest order under both the global worker ceiling and
the per-resource limits in `[[resource_policy]]`. The compiler-build resource
remains serialized because its Cargo/link state is mutable, while independent
repository, formal, runtime, and audit proofs use bounded parallel capacity.
An ordinary failed command blocks its dependents while independent work
continues. A deadline is local only when the guard proves process closure and
Cargo recovery is complete or unnecessary under its native interruption
inventory. Source mutation, unsafe resource pressure, unknown custody, signals,
or operator interruption stop all work through `guarded_exec` custody.
The exact failure rules are owned by
[the testing contract](../spec/areas/testing/0007-testing.md#proof-executor-failure-and-candidate-binding).
Receipts are always written
in manifest order and include the observed peak fanout, resource peaks, wall
time, cancellations, and the existing per-command peak-RSS/toolchain evidence.
For every `github-job` family, the manifest validator simulates the deterministic
DAG at declared command timeouts and requires its resource-aware projected
makespan to fit inside the workflow job timeout. This preserves fail-fast outer
budgets without pretending mutually concurrent partitions consume sequential
wall time.

## Agent-bootstrap conventions

These are the per-session conventions every agent follows; the driver assumes
them and the doc records them so they are canonical, not tribal.

- **One lane per change, from `origin/main`.** Each parallel agent works in
  its own lane:
  ```bash
  uv run --python 3.12 python tools/molt_dev.py lane start <slug>
  ```
  It prints the worktree path. Retire the lane with `lane retire <slug>`
  once it lands.
- **DX env before any build.** Export the RunContext facts at the start of a
  shell before build/test/bench work so artifact roots, cache roots, temp roots,
  daemon socket paths, and Python path all come from the same authority:
  ```bash
  eval "$(python3 tools/run_context_env.py --prefer-external-artifacts --dx --format posix)"
  ```
  Do not pin `MOLT_SESSION_ID` for ordinary builds; the generated custody id
  still uses the persistent Cargo target. Use `--session-id <unique-name>` only
  for perf/bench/test-shard/proof lanes that intentionally need
  `target/sessions/<id>/` isolation.
- **`PYTHONPATH=<repo>/src`** when running the in-repo `molt` package or tools
  that import it.
- **`safe_run.py` for any raw binary.** Never run a compiled molt binary (or
  anything that could infinite-loop / allocate unboundedly) unguarded:
  ```bash
  python3 tools/safe_run.py --rss-mb 2048 --timeout 15 -- ./binary [args]
  # exit 124 = TIMEOUT, 137 = OOM, else the child's own code.
  ```
  `difftest` runs generated programs through this guard.
- **Canonical WASM runner.** WASM artifacts run through the canonical node host
  shim `wasm/run_wasm.js` (the same shim the wasm differential calibration
  uses); do not hand-roll a host.
- **Foreground builds under contention.** Max 2 build-triggering agents at once
  (5 concurrent builds OOM the machine); prefer foreground builds when the host
  is contended so the resource guard can see them. Drain stale workers with
  `molt clean --apply --kill-processes` between waves.
- **`python-oracle` for version-sensitive tooling.** Resolve the interpreter via
  `python-oracle --python-version 3.12` rather than trusting whatever `.venv`
  symlink is currently active.

## rtk hazard applies to *your* shell too

The same proxy that makes `git diff`/`ls`/`stat`/`cmp` unreliable inside the
driver also rewrites *your* interactive shell. Inside the tool, all verdicts use
python ops / git plumbing. Outside, prefer absolute paths, verify pushes with
`verify-push` (not the push command's printed result), and confirm file content
with `probe`/python rather than `ls`/`stat`.

## Examples

```bash
# Plan an integration without mutating (dry-run the whole pipeline):
python3 tools/molt_dev.py integrate --dry-run

# Integrate with a content marker and an extra gate already declared by the plan:
python3 tools/molt_dev.py integrate \
    --marker "contains:runtime/molt-runtime/src/object/ops.rs::SETATTR" \
    --extra-gate "python tools/perf_methodology.py --check"

# Salvage a split staged/unstaged WIP into one recovery commit:
python3 tools/molt_dev.py secure-wip -m "in-progress alias-root drop fix"

# Confirm a push actually landed (do not trust the push's exit code):
python3 tools/molt_dev.py verify-push --tip HEAD

# Pin and verify CPython 3.12 before a calibration run:
PY=$(python3 tools/molt_dev.py python-oracle --python-version 3.12) && "$PY" -V

# Probe a daemon pid + its log:
python3 tools/molt_dev.py probe --pid 12345 --file logs/calib.log
```

## Where the contract is tested

`tests/test_molt_dev.py` exercises the driver's Git and process countermeasures
on real temporary repositories with a bare origin: commit preservation, push
confirmation, cleanup refusal, WIP capture, source markers, selected-gate failure
and interpreter admission. `tests/test_molt_dev_lanes.py` exercises the lane
commands and the landed predicate on a real checkout family, including a lane
replayed onto a moved main with its last commit amended.
`tests/test_checkout_provisioning.py` exercises stamps, failures, owner
installation, admission and a guarded `integrate` child whose inherited
`UV_PROJECT_ENVIRONMENT` names an environment nobody built. The `dev-tooling`
rule runs these owners whenever the driver, its config or tests change. `tests/test_proof_plan.py` exercises local
policy admission and projection. Compiler identity remains qualified by the
existing CLI artifact-custody owners: `test_compiler_identity.py`,
`test_cli_backend_binary_materialization.py`, `test_native_toolchain_transfer.py`
and `test_installed_compiler.py` under `tests/cli/`. They distinguish wrong-source,
changed executable and metadata-only changes, preserving independent oracles.
