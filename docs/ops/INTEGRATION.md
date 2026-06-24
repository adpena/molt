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
the whole pipeline without mutating anything.

```
fetch  →  rebase  →  verify-commits  →  markers  →  gates  →  push  →  confirm  →  cleanup
```

1. **fetch** — `git fetch origin main`; record the upstream tip.
2. **rebase** — rebase HEAD onto `origin/main` *only if behind*. A conflicting
   rebase is **aborted** (restoring the pre-rebase HEAD) and the run fails
   loudly; it never leaves a half-applied rebase.
3. **verify-commits (hazard 1)** — every source commit's **patch-id** computed
   *before* the rebase must be present *after*: either replayed into the new
   branch range, or already-upstream (the legitimate patch-id dedup). A missing
   patch-id is a **LOUD FAIL listing the dangling shas**; integration stops.
4. **markers (hazard 8)** — declared `--marker` content markers
   (`exists:<path>` / `contains:<path>::<needle>`) are checked against the
   post-rebase tree via python file ops. A mismatch fails before push.
5. **gates (hazard 10)** — the set of touched paths selects gate commands from
   `tools/molt_dev_gates.toml` (change-class → gates). A non-zero gate halts the
   run **before push**. `--extra-gate` appends arc-specific gates;
   `--no-gates` skips gating (use only when gating out-of-band).
6. **push** — `git push origin HEAD:main`. The push command's **exit code is
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
| 1 | **Rebase silently drops commits** (patch-id dedup vs moved-upstream) | `integrate` computes each source commit's `git patch-id --stable` *before* the rebase and verifies it survives *after* — present in the new range OR provably already-upstream by patch-id. Missing ⇒ LOUD FAIL listing the dangling shas; never proceeds. |
| 2 | **Push exit codes lie** (rtk/sandbox swallow; 144-detached pushes can “succeed”) | `verify-push` / the push step **fetch** the remote and confirm `origin/<branch>` **contains** the tip via `merge-base --is-ancestor`. Cleanup is gated on that, never on the push command's exit. |
| 3 | **Worktree cleanup loses work** | cleanup **refuses** if unpushed commits exist (`origin/main..HEAD` non-empty) **or** tracked staged/unstaged changes exist (excluding the ignore set). `--force` requires **naming the sha** being abandoned (deliberate + auditable). The main worktree is never removed. |
| 4 | **Partial WIP salvage** (split staged vs unstaged) | `secure-wip` enumerates **all** tracked modifications from `git status --porcelain -z` (staged + unstaged + adds), honors partner-excludes, and commits them in **ONE** `WIP-RECOVERY`-marked commit scoped to an explicit pathspec (a pre-staged excluded file is left out). |
| 5 | **diff/ls/stat lie under rtk** | Every comparison verdict uses git **plumbing** (`rev-list`, `diff-tree`/`patch-id`, `rev-parse`, `merge-base --is-ancestor`, `status --porcelain -z`, `worktree list --porcelain`) captured as bytes, or python file ops (`pathlib`, `filecmp.cmp(shallow=False)`). Never a human-readable shell text tool whose output the proxy can rewrite. |
| 6 | **Stale-binary misattribution** | `verify-toolchain` reports binary mtime vs the **newest commit touching Rust sources** (`runtime/**`, `Cargo.*`) and runs a configurable **behavior-marker probe** against the binary (always under `tools/safe_run.py`). A green marker on a stale binary is still flagged stale. `--reference` byte-compares against a prior binary to catch a no-op rebuild. |
| 7 | **.venv interpreter flips** (3.12 ↔ 3.14t across `uv` calls) | `python-oracle --python-version 3.12` resolves a candidate (uv → PATH → `sys.executable`), and **verifies `sys.version_info`** before accepting it. An interpreter that does not self-report the requested version is **refused**, never used. |
| 8 | **Content-marker verification** | `integrate --marker` checks declared `exists:`/`contains:` markers post-rebase, pre-push, via python file ops. |
| 9 | **Liveness/recovery probes** | `probe --file <p> --pid <n>` reports size+mtime (`os.stat`) and pid liveness (`os.kill(pid, 0)`) — replacing ad-hoc shell one-liners. Non-zero exit if a file is missing or a pid is dead. |
| 10 | **Gate selection by change-class** | `integrate` reads `tools/molt_dev_gates.toml` (touched-path glob → required gates) and runs exactly the gates the change demands; `always` gates run for any change; `--extra-gate` adds more. Real `**` glob semantics. |
| 11 | **Backgrounded long-runs die silently** (the harness reaps `run_in_background` process groups at detach [exit 144]; sandboxed tool calls reap even `setsid` daemons at container teardown — both lose block-buffered output: empty log, no exit status) | `detached-run` double-forks + `setsid` with **unbuffered IO** and an atomic state dir (`pid`/`sid`/`cmd.json`/`run.log`/`rc`); `detached-verify` is the **required second tool call** proving the daemon outlived the spawning call (`--min-age-s`), with a three-way verdict: `running` / `done(rc)` / **`died-silent`**. Spawn-and-verify in one call is structurally untrustable (teardown happens after the call returns), so the protocol is two-step *by design*. `detached-run` **never kills**: a live same-name daemon refuses; `--replace` only clears DEAD/finished state. |

## Subcommand surface

| Subcommand | Purpose |
|---|---|
| `integrate` | fetch → rebase → verify-commits → markers → gates → push → confirm → cleanup. `--dry-run`, `--no-gates`, `--no-push`, `--cleanup-worktree`, `--marker`, `--extra-gate`, `--gates-config`, `--ignore`, toolchain-freshness flags. |
| `secure-wip` | Commit **all** tracked modifications in one `WIP-RECOVERY` commit. `--dry-run`, `-m/--message`, `--ignore`, `--include-untracked`. |
| `verify-push` | Confirm `origin/<branch>` contains a tip sha by fetch + ancestor. `--tip`, `--json`. |
| `verify-toolchain` | Behavior-marker probe + binary-freshness report. `--marker`, `--probe-arg`, `--require-fresh`, `--reference`, `--require-differs`, `--json`. |
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

## The gate manifest (`tools/molt_dev_gates.toml`)

`integrate` selects gates by matching the touched paths of the integrated
commits against each rule's globs. `always` gates run for every non-empty
change; a rule matches when **any** touched path matches **any** of its globs;
gates are de-duplicated preserving first-seen order. Rules ship for: native &
LLVM backend Rust, TIR midend Rust, WASM host Rust, runtime/object-model Rust,
frontend & bootstrap stdlib Python, the op-kind registry, the suite-honesty and
ecosystem-compat ratchets, docs architecture, the dev tooling itself (a
self-gate that runs `tests/test_molt_dev.py`), and CI/pyproject wiring. A rule may set
`require_fresh_toolchain = true` so a Rust change additionally demands a fresh
built binary (hazard 6) when `--toolchain-binary` is supplied.

Gate command strings mirror the canonical invocations in
`pyproject.toml [tool.molt.dx.commands]`. Edit the manifest to add a
change-class; the driver's own tests assert the committed manifest stays valid.

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
- **`MOLT_SESSION_ID` before any build.** Export it at the start of *every*
  shell command so each session gets its own `target-<id>/`, daemon socket, and
  build caches:
  ```bash
  export MOLT_SESSION_ID="<unique-name>"   # MUST precede any molt/cargo command
  ```
- **`PYTHONPATH=<repo>/src`** when running the in-repo `molt` package or tools
  that import it.
- **`safe_run.py` for any raw binary.** Never run a compiled molt binary (or
  anything that could infinite-loop / allocate unboundedly) unguarded:
  ```bash
  python3 tools/safe_run.py --rss-mb 2048 --timeout 15 -- ./binary [args]
  # exit 124 = TIMEOUT, 137 = OOM, else the child's own code.
  ```
  `verify-toolchain`'s probe always routes the binary through `safe_run.py`.
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

# Verify a freshly built binary is not stale and carries the expected marker:
python3 tools/molt_dev.py verify-toolchain ./target-devtool/release-fast/molt-app \
    --marker "native=148" --require-fresh

# Pin and verify CPython 3.12 before a calibration run:
PY=$(python3 tools/molt_dev.py python-oracle --python-version 3.12) && "$PY" -V

# Probe a daemon pid + its log:
python3 tools/molt_dev.py probe --pid 12345 --file logs/calib.log
```

## Where the contract is tested

`tests/test_molt_dev.py` proves **every** hazard countermeasure with a
failure-mode test on real temp git repositories (bare-origin clones), mirroring
the `tests/test_check_suite_honesty.py` guard-test style: dropped-commit
detection fires; push-confirm catches a non-landed tip; cleanup refuses unpushed
and dirty trees; secure-wip captures staged + unstaged; marker mismatch fails;
gate-manifest selection is correct; the stale-binary and interpreter-pin
countermeasures fire. The `dev-tooling` gate rule runs this test file whenever
the driver, its config, or its tests change — so the driver gates **itself**.
