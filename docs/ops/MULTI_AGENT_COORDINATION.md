# Multi-Agent Verification Coordination

Status: active
Owner: tooling + tests + release
Last updated: 2026-06-22

## Purpose

Molt uses many agents because the proof surface is too large for one serial
loop, but proof work must be coordinated. This protocol makes agents discover
the same collaboration contract before they run differential, conformance,
benchmark, or broad validation lanes.

The goal is highest expected signal per machine hour:

- one tiny, named aperture before any broad work;
- one full structural rip through the authority exposed by that aperture;
- one authority per proof lane;
- one supervising broad differential or regrtest run per shared target root;
- targeted verification before broad sweeps;
- durable evidence in canonical artifact roots;
- no silent blocking, duplicate rebuild storms, or spammy full-suite reruns.

## Startup Discovery

Every agent that may run builds, differential tests, conformance, regrtest,
benchmarks, or long validation must do this before the first heavy command:

1. Read `AGENTS.md`, this file, and the relevant routing doc for the touched
   subsystem.
2. Name the tiny aperture and the structural authority it exposes. Do not start
   a broad proof lane, decomposition arc, or multi-agent migration until the
   entry invariant, command family, file cluster, authority surface, or failing
   execution path is explicit.
3. Determine the local environment and choose commands from facts, not
   assumptions:

   ```bash
   uv run --python 3.12 python tools/agent_coordination.py env
   ```

   The command reports OS, architecture, Python executable, available Python
   launchers, `uv`, shell, WSL/Codex/CI hints, and the recommended Python
   command for this host. Use the wrapper `tools/new-agent-task.sh` only when a
   POSIX shell is available; otherwise run
   `uv run --python 3.12 python tools/agent_coordination.py init <task>`.
4. Inspect local ownership without reverting partner work:

   ```bash
   git status --short
   ```

5. Create or update a task log:

   ```bash
   tools/new-agent-task.sh <task-slug>
   ```

   The wrapper delegates to `tools/agent_coordination.py init` and writes both
   a markdown report and `logs/agents/<task>/coordination.json`.

6. Use the cross-platform DX authority before any build or harness command:

   ```bash
   uv run --python 3.12 python -m molt.cli dx env
   uv run --python 3.12 python -m molt.cli dx run -- <command>
   ```

   The command syntax is identical on Windows, macOS, and Linux. The tool
   resolves host OS/architecture facts, canonical artifact roots, session id,
   backend-daemon socket directory, shared `SCCACHE_DIR`, and cache-retention
   defaults. Use `molt dx env --format powershell|posix|cmd|json` only when a
   parent shell must import the values.

7. Bootstrap shared throughput defaults when a long proof lane still needs
   shell activation:

   ```bash
   uv run --python 3.12 python -m molt.cli dx env --format posix
   ```

   `tools/throughput_env.sh --apply` remains a POSIX compatibility wrapper over
   the same Python DX resolver, not a second environment authority.

8. Record in `logs/agents/<task>/`:
   - named aperture and exposed authority;
   - owned files/directories;
   - intended proof lane;
   - whether this agent is the broad-sweep coordinator;
   - exact commands and artifact paths;
   - current blocker or next command.

If another active log already owns the same files or broad proof lane, switch to
a non-colliding task, coordinate explicitly in the log, or wait only when the
existing owner has a bounded finish condition.

## Machine-Readable Discovery

Agents must use the JSON coordination records as the machine-readable discovery
surface before long proof work.

```bash
uv run --python 3.12 python tools/agent_coordination.py scan
uv run --python 3.12 python tools/agent_coordination.py check
uv run --python 3.12 python tools/agent_coordination.py proof-plan
```

- `scan` lists current task records and any broad-lane collisions.
- `check` returns nonzero when two active broad-sweep coordinators claim the
  same proof lane on the same shared target root.
- `proof-plan [paths...]` recommends focused proof lanes from explicit paths or
  from current `git status` when paths are omitted. Run it before long proof
  work so agents pick targeted differential, conformance, backend, or custody
  checks before broad sweeps.
- `env` prints local OS/Python/shell facts so agents choose macOS, Linux, WSL,
  or Windows-safe commands before running long proof lanes.
- `codex-stall -- <command>` runs a proof command through
  `tools/memory_guard.py` by default and writes privacy-preserving output
  liveness evidence under `logs/agents/codex_stall/`. Use it when Codex appears
  stalled or crash-adjacent during Windows proof work; it records first-output
  gaps, stream-idle spans, byte/chunk counts, elapsed time, and return code,
  but not child stdout/stderr text or Codex state.
- `tools/new-agent-task.sh <task>` is the POSIX short wrapper for
  `tools/agent_coordination.py init <task>`. The init command writes
  `logs/agents/<task>/env.sh` and `env.ps1` from the same DX facts; the
  primary cross-platform command path remains `molt dx run -- <command>`.
- `coordination.json` is a discovery index, not a lock. The real serialization
  authority for differential work remains the harness lock under
  `<CARGO_TARGET_DIR>/.molt_state/diff_run.lock`.

### Windows Host Traps

On Windows, `python3` may resolve to a Microsoft Store `WindowsApps` execution
alias and `bash` may resolve to the WSL shim at `C:\Windows\System32\bash.exe`.
Treat both as unsuitable for long proof lanes unless the environment snapshot
marks them usable. Prefer:

```powershell
uv run --python 3.12 python tools/agent_coordination.py <command>
```

When a POSIX wrapper is useful on Windows, use the `usable_bash` reported by
`tools/agent_coordination.py env` rather than the first `bash` on `PATH`.

If Codex restarts with Windows status `0xC000013A`, treat it as an interrupted
or torn-down process first. Preserve the active task log, re-run `env`, and
inspect application logs before assuming the most recent `state_db`,
plugin-manifest, or MCP warning is causal. Do not edit Codex runtime plugin
manifests, plugin caches, or state databases as a first response while a Molt
proof lane is active.

## Coordination Roles

Use these roles instead of letting every agent run the heaviest command it can
think of.

| Role | Owns | Runs | Must not do |
| --- | --- | --- | --- |
| Implementer | A bounded structural code/doc arc | Targeted unit, cargo, or diff proof for touched behavior | Start full sweeps while code is still moving |
| Reducer | One failure family or failure queue | Minimal repros, focused differential reruns, debug traces | Re-run the whole corpus to rediscover known failures |
| Broad-sweep coordinator | One shared target root and one broad lane | Full differential, CPython regrtest, conformance, or release validation | Compete with another broad sweep on the same target root |
| Perf custodian | One benchmark family and baseline artifact | Targeted bench, bench diff, regression triage | Mix perf claims with unrelated correctness sweeps |
| Integrator | Cross-agent merge/proof synthesis | Final targeted and broad gates after owned changes land | Rewrite partner work or weaken coverage |

One agent can hold more than one role only when the roles do not collide. For
example, an implementer may also run targeted diff tests for its own patch, but
the full differential sweep should have a single broad-sweep coordinator.

## Proof Selection Ladder

Choose the cheapest proof that can falsify the current change. Escalate only
after lower rungs pass or when the change directly requires the wider lane.

1. Static/local contract checks:
   - format/lint/type checks for touched language;
   - generated-table freshness checks when generated authority moved;
   - focused Rust/Python unit tests for the edited module.
2. Targeted behavioral proof:
   - direct regression test;
   - focused `tests/molt_diff.py <specific files>`;
   - focused backend, WASM, LLVM, Luau, or intrinsic lane.
3. Lane proof:
   - one differential lane such as `tests/differential/basic` or
     `tests/differential/stdlib`;
   - one CPython regrtest selection;
   - one conformance suite shard.
4. Broad proof:
   - full differential basic + stdlib;
   - full `molt validate`;
   - CPython regrtest pre-release lane;
   - benchmark matrix or release benchmark bundle.

Do not run a broader rung just to produce activity. State the hypothesis, run
the smallest falsifying command, and preserve the artifact.

## Differential And Conformance Protocol

Differential and conformance lanes are shared resources.

- Keep one supervising broad diff/regrtest/conformance run per shared
  `CARGO_TARGET_DIR`.
- Let `tests/molt_diff.py` own its run lock at
  `<CARGO_TARGET_DIR>/.molt_state/diff_run.lock`; do not bypass it with raw
  loops.
- If a broad run is active, other agents should prefer:
  - implementation work that does not need the broad lane;
  - failure-queue reduction;
  - targeted reruns with a different target root only when explicitly useful;
  - docs/spec/matrix updates based on already captured evidence.
- If the diff lock would make an agent wait without adding signal, record the
  wait in the task log and move to non-colliding work.
- Resolve canonical roots through the DX authority before heavy maintainer,
  agent, benchmark, differential, conformance, or CI-style lanes. On Windows
  checkouts on `C:`, use a healthy non-`C:` root unless an explicit emergency
  override is set. Public users may compile in place, use Molt/Cargo defaults,
  or choose outputs with explicit flags/environment variables.

  ```bash
  export MOLT_SESSION_ID="<unique-agent-session>"
  eval "$(python3 tools/run_context_env.py --prefer-external-artifacts --dx --format posix)"
  ```

  Repo-local roots such as `$PWD/target`, `$PWD/.molt_cache`, and `$PWD/tmp`
  are local fallback/user-default examples, not heavy proof-lane guidance on
  constrained internal disks.

- Keep RSS and harness custody enabled. Memory blowups are failures, not a
  reason to disable the guard.
- Write broad-run summaries to durable locations:
  - `tmp/diff/summary.json` or an explicit `MOLT_DIFF_SUMMARY`;
  - `logs/cpython_regrtest/<run>/summary.md`;
  - `logs/validate-*.json`;
  - `bench/results/*.json`.

## High-EV Evidence Rules

Evidence is high signal when it changes a decision.

- Prefer failure queues over rediscovery.
- Prefer targeted reruns after an implementation change.
- Prefer one broad coordinator over many agents queued behind the same lock.
- Prefer structured summaries over stdout walls.
- Prefer cross-version or cross-target proof only when the touched semantics can
  plausibly diverge by Python version, OS, architecture, backend, or capability.
- Do not treat a broad pass as a security or performance proof unless the
  relevant security or benchmark gate also ran.
- When a command fails, preserve the command, return code, artifact path, and
  next falsifying step before launching another heavy command.

## Collision Rules

Agents must be respectful of partner work.

- Never revert, restage, reformat, or clean up files owned by another active
  task unless the owner explicitly asks.
- If two agents must touch the same source of truth, appoint one integrator and
  make the other agents produce patches, repros, or evidence against that
  integration lane.
- Do not add new broad-sweep artifacts under ad hoc top-level directories.
- Do not use raw PID kills, `pkill`, or socket/PID guesses. Use the custody
  sentinel and backend-daemon identity authority described in `docs/OPERATIONS.md`.
- Do not split a structural fix across agents in a way that leaves parallel
  sources of truth. Split by complete structural primitive, not by convenience.

## Task Log Contract

Every active multi-agent task log under `logs/agents/<task>/` should answer:

- What invariant, command family, file cluster, or product surface does this agent own?
- Which files/directories are in scope?
- Which proof rung is planned next?
- Which broad proof lanes are already owned elsewhere?
- Where are the command artifacts?
- What should the next agent do if this one stops?

The same facts must be mirrored in `coordination.json` when they affect
machine scheduling or proof-lane ownership.

Use bounded status reports in `docs/OPERATIONS.md` for coordination, not as a
substitute for changing code. A log that has not been updated and has no running
command should be treated as stale context, not an active lock.

## Completion Checklist

- [ ] Owned files are staged in the intended ownership arc.
- [ ] Task log names files touched, commands run, artifacts, and residual risk.
- [ ] Targeted proof passed or the failure is documented with a closure plan.
- [ ] Broad proof was run by one coordinator when required.
- [ ] Spec/status/roadmap/matrix docs moved with any changed claim.
- [ ] No partner work was reverted or silently overwritten.
