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
fetch  →  rebase  →  verify-commits  →  markers  →  gates  →  push  →  confirm  →  cleanup
```

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
4. **markers (hazard 8)** — declared `--marker` content markers
   (`exists:<path>` / `contains:<path>::<needle>`) are checked against the
   post-rebase tree via python file ops. A mismatch fails before push.
5. **gates (hazard 10)** — the set of touched paths selects gate commands from
   the selected commit's `tools/proof_plan.toml` (change-class → gates). Always
   gates and every matching rule run. A non-zero gate or source/HEAD drift halts
   the run **before push**. `--extra-gate` only adds arc-specific gates. There is
   no gate-skipping or alternate-manifest landing route.
   Prerequisite failures are reported together. Content-resolver and version-query
   failures retain the probe command, directory, actual cause and bounded output; they do not
   fabricate an unavailable image path. Relative interpreter aliases preserve the
   selected entrypoint and canonicalize against the declared probe directory
   without changing the validated executable image.
6. **push** — `git push origin <captured-commit>:refs/heads/main`. Foreign Git
   hooks remain active and both rejection streams are retained. Git identity
   reads ignore local replacement refs. The push command's **exit code is
   NOT trusted** (hazard 2).
7. **confirm (hazard 2)** — `git fetch` then confirm `origin/main` **contains**
   the pushed tip via `git merge-base --is-ancestor`. Cleanup is gated on THIS,
   never on the push exit code. A non-landed tip is a loud fail; cleanup is
   refused.
8. **cleanup (hazard 3)** — with `--cleanup-worktree`, remove the worktree, but
   only when safe (see below).

Each failure raises a `DriverError` that `main()` translates to a non-zero exit
code, so `integrate` is CI-usable (exit `0` ok, `1` a check failed, `2` usage).

## The hazard → countermeasure table (as implemented)

| # | Hazard | Countermeasure in the driver |
|---|--------|------------------------------|
| 1 | **Rebase silently drops commits** (patch-id dedup vs moved-upstream) | Without replay, require unchanged captured HEAD and upstream ancestry, with zero patch-diff work. Actual replay computes source `git patch-id --stable` before mutation and verifies survival afterward. Missing ⇒ LOUD FAIL listing the dangling shas; never proceeds. |
| 2 | **Push exit codes lie** (rtk/sandbox swallow; 144-detached pushes can “succeed”) | `verify-push` / the push step **fetch** the remote and confirm `origin/<branch>` **contains** the tip via `merge-base --is-ancestor`. Cleanup is gated on that, never on the push command's exit. |
| 3 | **Worktree cleanup loses work** | cleanup **refuses** if unpushed commits exist (`origin/main..HEAD` non-empty) **or** tracked staged/unstaged changes exist (excluding the ignore set). `--force` requires **naming the sha** being abandoned (deliberate + auditable). The main worktree is never removed. |
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
| `integrate` | fetch → rebase → verify-commits → markers → gates → push → confirm → cleanup. `--dry-run`, `--no-push`, `--cleanup-worktree`, `--marker`, `--extra-gate`, `--expected-upstream`, `--ignore` (cleanup only). |
| `secure-wip` | Commit **all** tracked modifications in one `WIP-RECOVERY` commit. `--dry-run`, `-m/--message`, `--ignore`, `--include-untracked`. |
| `verify-push` | Confirm `origin/<branch>` contains a tip sha by fetch + ancestor. `--tip`, `--json`. |
| `probe` | File size+mtime / pid liveness via python ops. `--file`, `--pid`, `--json`. |
| `python-oracle` | Resolve + PIN + verify a CPython version before use. `--python-version`, `--no-uv`, `--json`. |
| `detached-run` | Spawn a command as a `setsid` daemon with a state dir (hazard 11). `--name`, `--cwd`, `--env K=V`, `--replace`, `--state-dir`, `--json`, `-- cmd…`. The spawning tool call itself must run unsandboxed; verify in a *later* call. |
| `detached-verify` | Prove the daemon outlived its spawning call: `running` / `done(rc)` / `died-silent`. `--name`, `--min-age-s`, `--state-dir`, `--json`. Exit 0 only for `running` past min-age or `done` rc 0. |
| `cleanup` | Standalone hazard-3 worktree cleanup gate. `--ignore`, `--force SHA`. |

### The detached-run protocol (hazard 11)

```bash
# Tool call 1 (UNSANDBOXED — the sandbox is precisely what kills daemons):
python3 tools/molt_dev.py detached-run --name flip_sweep --cwd /tmp/wt_flip \
  -- python3 tests/molt_diff.py tests/differential/basic --jobs 4 \
     --json-output /tmp/sweep.json

# Tool call 2+ (any later call; --min-age-s proves it survived teardown):
python3 tools/molt_dev.py detached-verify --name flip_sweep --min-age-s 30 --json
# → running        keep waiting (poll again later)
# → done, rc=N     read the artifacts; rc≠0 exits 1
# → died-silent    the hazard-11 group-kill class, reported LOUDLY
```

Discovered 2026-06-06 (twice in one session): a backgrounded 807-test sweep
died at harness detach with an **empty log** — block buffering ate every line —
and the `setsid` respawn died at sandbox teardown the same way. The state-dir
`rc` file is the only trustworthy completion signal; its absence with a dead
pid is a *diagnosis*, not a mystery.

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

Gate command strings mirror the canonical invocations in
`pyproject.toml [tool.molt.dx.commands]`. Edit the manifest to add a
change-class; the driver's own tests assert the committed manifest stays valid.

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

- **Worktree off `origin/main`.** Each parallel agent works in a fresh detached
  worktree:
  ```bash
  git -C /Users/adpena/Projects/molt worktree add --detach /tmp/wt_<name> origin/main
  ```
  (Agent worktree isolation branches from the session-start commit; foundation
  work touching just-landed files should branch from current `main` instead.)
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

# Integrate with a declared content marker and an arc-specific extra gate:
python3 tools/molt_dev.py integrate \
    --marker "contains:runtime/molt-runtime/src/object/ops.rs::SETATTR" \
    --extra-gate "python3 -m pytest -q tests/test_some_arc.py"

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
and interpreter admission. The `dev-tooling` rule runs that owner whenever the
driver, its config or tests change. `tests/test_proof_plan.py` exercises local
policy admission and projection. Compiler identity remains qualified by the
existing CLI artifact-custody owners: `test_compiler_identity.py`,
`test_cli_backend_binary_materialization.py`, `test_native_toolchain_transfer.py`
and `test_installed_compiler.py` under `tests/cli/`. They distinguish wrong-source,
changed executable and metadata-only changes, preserving independent oracles.
