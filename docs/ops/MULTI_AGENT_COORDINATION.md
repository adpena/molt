# Multi-Agent Verification Coordination

Status: active
Owner: tooling + tests + release
Last updated: 2026-10-28

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

## Proportionate development feedback

The orchestrator selects a small set of release-critical outcomes by dependency,
user impact and measured cost. Finish, land and retire a coherent outcome before
opening adjacent apparatus work. A bounded review is complete when it returns its
requested evidence; implementation ownership includes integration and retirement.

Choose the changed claim and a plausible defect, then the smallest independent
check that distinguishes them. Stop focused development testing when that claim is
proved. A new failure, changed input or named unresolved obligation can justify
widening. Required selected integration gates and release qualification still run;
development feedback and release acceptance are distinct claims. Preserve matching
receipts rather than repeating successful work. Scratch experiments do not require
permanent tests, helpers or new validators for low-impact edits.

Reuse compatible compiled producers and batch affected selectors. Different
profiles, feature sets and targets need explicit equivalence or subsumption before
their gates can be consolidated. Measure the critical path before increasing
workers; additional pytest sessions do not shorten a serialized Rust build. Keep
one shared Cargo writer and existing resource admission. Delegate substantial
independent work when saved time or independent correctness insight justifies
startup and context cost; do not create small agents to recheck passing evidence.

This policy was reviewed against primary guidance on 2026-10-28:

- [OpenAI's current model guide](https://developers.openai.com/api/docs/guides/latest-model)
  and [GPT-6 Astra workflow guidance](https://developers.openai.com/blog/rethinking-skills-and-prompts-for-gpt-6-astra)
  recommend calibrated testing and delegation, concise task-specific instructions
  and removal of accumulated scaffolding. Small edits need no mandatory full-map
  discovery or blanket extra verification.
- [Anthropic's Opus prompting guide](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-opus-5)
  and [Fable prompting guide](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-fable-5-1)
  discourage reflexive verification and tiny delegated tasks; scope permanent
  tests to the requested behavior and batch independent tool work. The newer
  [Opus 5.5 guide](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-opus-5-5)
  retains Opus 5's prompting patterns as a starting point and recommends measuring
  effort against quality and latency rather than carrying maximum settings forward.
- [Claude Code memory guidance](https://code.claude.com/docs/en/memory)
  favors concise, consistent instructions. Imports still consume context:
  `CLAUDE.md` imports `AGENTS.md` as one authority, not a second policy.
- [LLVM's developer policy](https://llvm.org/docs/DeveloperPolicy.html) favors
  focused regressions using the closest tool and incremental self-contained
  changes. [rustc's test workflow](https://rustc-dev-guide.rust-lang.org/tests/running.html)
  uses targeted local tests and broader integration qualification. Neither model
  treats maximum local test volume as progress.

## Serialized integration

### V1 burndown authority

`docs/agent/V1_HANDOFF_FINDINGS.md` alone owns current V1.0 burndown state,
priorities, blockers and next closure outcomes. Consult its published release
decision, active integration and dependency order before selecting or resuming
work, then read the owning finding rows. Use `context` to distinguish published
and selected source; dirty primary prose and remembered handoffs are leads.

Workers return evidence against those finding IDs. The integrator updates their
rows and the relevant frontier/dependency sections as material state changes,
in the same landing as the implementation or qualification. Source integration,
representative execution and full qualification remain distinct; close only the
qualified family. Record remaining cells and the next result needed. Reconcile
contradictions into the ledger instead of adding another backlog. Coordination
records retain live ownership and private command/recovery state; roadmaps and
reports link to or derive their V1 status from the ledger.

### Release outcome graph

The canonical `context` command joins the committed findings ledger and proof
catalog with explicit `release_outcomes` in active coordination records. It reports
published and selected commit identities separately; dirty working-tree prose
cannot change either count. Every finding remains visible. An open finding without
a binding is `UNPLANNED`; a binding is `PLANNED`, which does not attest review,
semantic proof, landing eligibility or completion. Historical fixed rows keep their
recorded status without invented phase evidence.

An outcome names a complete invariant or migration family and may own several
finding IDs. Declare only real prerequisite edges. Independent outcomes need no
edge merely because they share a release milestone. One active record owns each
binding; duplicate finding/outcome ownership, unknown findings or proof families,
escaping input paths and dependency cycles make the projection unavailable with a
structured diagnostic.

For example, an active record can contain:

```json
"release_outcomes": [{
  "id": "execution-evidence",
  "findings": ["HF-334", "HF-190", "HF-35"],
  "depends_on": [],
  "proof_families": ["repository_policy"],
  "inputs": ["tools/proof_*.py", "tools/molt_dev*.py"]
}]
```

These declarations live in existing coordination custody, outside compiler source
and the proof-policy digest. Updating an assignment or dependency therefore does
not invalidate unchanged proof receipts. Proof commands, dependencies and resource
admission remain owned by the proof plan/executor; publication and retirement remain
owned by the landing queue. This graph projection does not yet connect phase
evidence to either queue or direct-integration admission. Keep that limitation
explicit until one shared, source-bound predicate serves both paths.

A live `tools/molt_dev.py integrate` owns one OS file lock in Git's resolved
common directory, shared by the primary checkout and every linked worktree.
The handle covers fetch, source selection, gates, push confirmation and cleanup.
A competing integration refuses before fetch or proof execution; it does not
poll, sleep or spend resources qualifying a second candidate. Dry-run creates
no lock. Independent source implementation and unrelated builds remain free to
proceed in their own trees.

This follows [Cargo's filesystem-lock ownership](https://doc.rust-lang.org/stable/nightly-rustc/cargo/util/flock/index.html)
using Molt's existing cross-platform file-lock authority. Git common-directory
resolution preserves byte-exact Git path output and is also shared with
main-worktree environment discovery. Admission
adds one Git metadata query and one lock acquisition per live integration, with
one small persistent lock file. The OS handle owns exclusion: file presence,
PID age and timestamps do not. Never delete the lock file to release it; normal
completion, exceptions and process exit release the handle. This is development
coordination and adds no work to emitted programs.

The lock is advisory and repository-local. Current cooperating drivers share
it; older drivers, independent clones and external writers do not. Upstream
identity checks, ordinary non-fast-forward rejection and fetch/ancestor push
confirmation therefore remain mandatory. A publication race is a refusal,
never permission to force-push or reuse qualification for changed inputs.

Retirement uses [Git's worktree removal](https://git-scm.com/docs/git-worktree#Documentation/git-worktree.txt-remove-fworktree)
to retire the directory and its registration together. Launch the removal
subprocess from the surviving main checkout as well as selecting it with `-C`:
the Windows executable shim can retain the initial cwd while Git removes the
target. Drift harvesting protects its executing source checkout by resolved
containment, so its default Git cwd survives every prune. These are development
tooling rules and add no work to emitted programs.

## Startup Discovery

Every dispatch uses `tools.dispatch_prompt.DispatchBounds` (or the CLI's required
`--bounds` JSON file). Specify `outcome`, `deliverable`, `integration_owner`,
`proof_owner`, `owned_paths`, `proof_commands`, `budget_minutes` and `stop_when`.
There are no inferred defaults: empty path and command arrays explicitly mean
read-only work and no proof launches. Workers return when their deliverable or
budget is reached, closing owned commands through their existing custody owner.
Only the parent can assign expanded scope, research, proofs or delegation after
reassessing the critical path. These are validated dispatch inputs and supervisory
rules, not a sandbox or a mechanism that terminates arbitrary agent processes.

### Brief review

The parent reviews every brief before dispatch. A brief passes when:

1. Every premise is checked. Paths, flags, fields, lines and commands are checked
   against the checkout (`--help`, `ls`, `rg`), and every rule against the
   repository's own infrastructure (hooks, generators, launchers), so a brief never
   forbids what the repository requires. External facts cite a primary source.
2. Each decision names the case it must refuse, its soundness condition and the
   evidence that would overturn it. The worker stops and reports that evidence.
3. Acceptance is an oracle: the invariant, plus tests that tell it from a plausible
   defect. It is not a list of implementation steps.
4. It names the cost bar and how to measure it.
5. It names the lanes in flight, the shared state it touches and the merge points.
6. It changes no earlier decision silently. A changed decision says so, and why.
7. It states the stop conditions and the report format.

The brief applies the engineering rules in `AGENTS.md` (typed lifecycles, minimum
apparatus, whole-family migration); it does not restate them. Dispatch it as a file
(`dispatch_prompt.py --out`), so a correction appended under a dated "Correction"
heading reaches a running worker.

Use one proof owner and inspect matching retained evidence before requesting a
run. An independent reviewer challenges the changed contract and oracle; it does
not automatically rerun a passing proof. Read-only findings are valid deliverables.
The integration owner lands and retires source work; workers have no per-turn
commit, proof-run or landing quota. `AGENTS.md` is the standing policy and
`CLAUDE.md` imports that same authority.

Classify facts and identities with typed structures, declared schemas and
canonical deterministic parsers first. If genuine ambiguity remains, a local model
may suggest a label. The backend is a typed capability chosen up front and recorded
on every label, never try-then-fall-back: an OpenAI-compatible local server
(llama.cpp `llama-server`, LM Studio) on every OS and architecture, or `fmtools`
(Apple Foundation Models, with per-row candidate sets) on macOS 26. Measured on the
ledger's 20 families (2026-10-31): Qwen3.8-27B 80%, embedding nearest neighbours
50% top-1 and 75% top-3, Apple FM 18% (42% choosing among a top-3 shortlist). A
label is advice, never a gate or an authority over typed identities and execution
evidence. Keep models unloaded when idle; they hold memory that gates need.

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

### Interpreter environments are mutable proof inputs

Versioned CPython oracles use the existing interpreter and pinned-source
authorities. Resolve the installed command and verify its complete version with
`molt.python_interpreter.probe_python_command`; retain the interpreter identity
separately from the guest target. Do not run project-scoped `uv run --python` for
another minor in an active checkout: uv correctly replaces that checkout's
environment, including packages another proof may still be importing.

When dependencies require another environment, provision an explicitly owned
`UV_PROJECT_ENVIRONMENT` and bind the consuming command to that environment.
Reconcile live readers before a sync or replacement; a checkout being clean does
not make its environment idle. If mutation occurs, preserve the command and
identity evidence, classify the exact overlapping intervals, and requalify the
affected claims. A completed proof before the mutation remains useful; a later
proof on another verified interpreter must name that interpreter. Do not discard
unchanged evidence or relabel interpreter versions. This is development custody
and adds no work to compiled programs.

## Machine-Readable Discovery

Agents must use the JSON coordination records as the machine-readable discovery
surface before long proof work.

```bash
uv run --python 3.12 python tools/agent_coordination.py context
uv run --python 3.12 python tools/agent_coordination.py scan
uv run --python 3.12 python tools/agent_coordination.py check
uv run --python 3.12 python tools/agent_coordination.py proof-plan
```

- `context` is the canonical read-only whole-project entrypoint. Its concise
  renderer separates Git facts observed at query time from file-backed claims,
  coordination, and proof records. `context --json` emits the versioned
  `molt.agent-context.v1` model with canonical root, HEAD/origin drift, every
  registered worktree and its dirty-path count, claims health, proof-audit and
  dead-custody summaries, instruction-authority health, and canonical document
  pointers. It writes no registry or status file. Missing authorities, invalid
  records, subprocess failures, and unhealthy proof audit results appear in
  `errors` and make the command exit nonzero; unavailable data is never rendered
  as a healthy zero. The summarized proof section includes the full-detail
  audit command when deeper evidence is needed. Queue inspection opens existing
  state read-only, including a live WAL, and performs no initialization,
  migration or scheduling reconciliation. Unavailable or unsupported queue state
  remains an explicit context error until an authorized writer handles it.
  Pending ownership requests show
  their requester, owner task, exact requested paths, task/request status, record
  path, and timestamps in both text and JSON. Active JSON records also include
  granted `owned_paths` and resolved request state.
- `scan` shows pending ownership requests before listing all task records,
  including historical canonical records, and any broad-lane collisions. Its JSON
  shares the same `pending_ownership_requests` projection as `context`.
- `check` returns nonzero for any invalid canonical record, or when two active
  broad-sweep coordinators claim the same proof lane on the same shared target
  root. A rejected record cannot make an unsafe coordination check green.
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
- `codex-crash --crash-text "<dialog text>"` classifies a Codex crash dialog
  and writes a bounded recovery capsule under `logs/agents/codex_crash/`. It
  treats `3221225786` as Windows `STATUS_CONTROL_C_EXIT`, recognizes
  response-stream retry pressure, plugin `interface.defaultPrompt` warnings,
  and rollout-state repair symptoms, counts plugin manifest pressure, and
  records next actions without storing raw crash text or mutating Codex state.
- `tools/new-agent-task.sh <task>` is the POSIX short wrapper for
  `tools/agent_coordination.py init <task>`. The init command writes
  `logs/agents/<task>/env.sh` and `env.ps1` from the same DX facts; the
  primary cross-platform command path remains `molt dx run -- <command>`.
- `coordination.json` is a discovery index, not a lock. The real serialization
  authority for differential work remains the harness lock under
  the canonical `MOLT_EXT_ROOT/tmp/build-control/<target-address>/diff_run.lock`
  when an artifact root is selected (otherwise the target-local state root).

### Source ownership and handoff

The parent registers every active worker before dispatch, including read-only
reviewers. Use `init --agent --role --lane --owned` and update the resulting
record when scope changes or the worker finishes. A reviewer has no owned write
paths. Keep the agreed registry root and actual source checkout explicit in the
parent record; `scan` reads the selected repository's records, not agent chat.

`tools/molt_dev.py lane start <slug>` registers each lane it creates as task
`lane-<slug>` in the main checkout's registry, with the lane worktree as the
record's source checkout and a `lane` object (branch, base, worktree).
`lane retire` closes the record as `landed` or `abandoned` and records the
retired tip and any bundle.

The registry consists only of `logs/agents/<task>/coordination.json`, where
`<task>` is one directory name and the record's `task` field matches it exactly.
`init` rejects nested task names and nonportable or aliased directory names
using the shared portable-path grammar; it never strips or rewrites task identity.
Files below that directory, including copied
`coordination.json` files in evidence, archives, base trees, or donor trees, are
preserved material and never register another worker. Discovery does not depend
on archive directory names or renaming evidence files. Malformed JSON, missing
task IDs, and mismatched IDs at canonical locations remain visible as invalid
records and make context health fail. Only admitted canonical records contribute
active workers, pending requests, and proof-lane collisions.

The integrator reserves finding IDs in the current canonical
`docs/agent/V1_HANDOFF_FINDINGS.md` before workers use them in source or handoffs.
Workers use descriptive artifact names until that reservation is published;
private copies of the ledger do not allocate IDs. Reconcile current `origin/main`
before assigning or merging findings, and use the existing `finding_status`
projection to reject duplicate or resurrected rows. If a private label collides
with an independently published finding, retain both obligations under distinct
canonical IDs and record the private-to-canonical mapping in coordination
evidence. Keep frozen receipts intact. `(was HF-N)` means the same finding was
renamed and closes the old row; it must never rename a different public finding
merely because a private label reused its number.

If a worker is explicitly waiting for an ownership decision, put the request in
that worker's existing `coordination.json`. For a new task:

```bash
uv run --python 3.12 python tools/agent_coordination.py init llvm-worker \
  --agent llvm-delegate --role implementer --status blocked \
  --request-owner integration --requested src/molt/verified_subset.py \
  --requested tools/verified_subset.py
```

`--request-owner` names the coordination task responsible for assigning scope;
each `--requested` path is repository-relative. These options write one
`ownership_request` object with `owner`, `requested_paths`, `status: "pending"`,
and `requested_at_utc`, a UTC timestamp with `Z` or an explicit zero offset.
They do not add paths to `owned_paths`. For an existing
task, update that object and `updated_at_utc` in the existing record instead of
reinitializing its scaffold. Markdown can explain the request, but the JSON
fields are the discovery authority for its owner, scope, and current state.

The owner resolves the request by changing its status to `acknowledged`,
`declined`, or `withdrawn`, recording the decision in the task's evidence, and
updating the task status, timestamp, and granted `owned_paths` as appropriate.
Acknowledgment does not automatically grant paths or start a proof lane. A new
scope request replaces the resolved request with a new pending timestamp; retain
the prior decision in the existing progress log or report.

Only explicit pending requests on `running`, `paused`, or `blocked` tasks appear
in the pending section. Historical blocked records with no request, resolved
requests, and terminal task records do not become pending actions. An old
explicit pending request remains visible until resolved; its recorded timestamps
show its age without treating age as cancellation. Malformed request fields make
the record visibly invalid rather than silently dropping its request.

Each query observes the selected registry at query time. A context captured
before registration cannot include the later request; query again at handoff or
before assigning the next scope. These tools do not poll, notify another chat,
guarantee delivery, or establish a source lock.

`check` detects broad proof-lane collisions; it does not lock source files or
merge dirty worktrees. The integrator must enforce disjoint write scopes and
freeze affected writers before reconciliation. Record base revision, changed
paths, deletions, evidence, unresolved findings, and a working-content identity
at handoff. Recheck that identity before applying the reviewed changes; if it
changed, reconcile the new delta rather than overwrite it or repeat stale proof.

Before design, implementation or deletion, record the worker's proactive search
of the canonical implementation, relevant history/handoffs and preserved donor
work. Identify the authority reused and any overlapping implementation; do not
infer absence from a task summary or one checkout. External reviewers follow the
same retrieval discipline and remain read-only unless assigned disjoint write
ownership. Model choice never changes custody or the active worker/proof caps.

Compare both working trees against their common base. Preserve unique changes
and reconcile semantic conflicts before advancing a baseline or pruning a tree.
Regenerate projections from the merged authority. Only the integrator stages,
proves the integrated result, and lands; a completed worker is not a landed arc.

Assign each worker a complete contract family and its completion obligations in
that same coordination record. The worker owns producer and consumer migration,
retirement of replaced paths, independent regression controls, failure and
resource ownership, documentation, self-review and cleanup. Its handoff lists
exact evidence and unverified coordinates; source readiness is not completion.
The integrator returns exposed failures to that owner and keeps the worker
responsible through accepted qualification and landing. Shared expensive builds
have one named proof owner; workers supply their selectors and oracles to that
build instead of starting competing copies. Close ownership only after checking
the landed source, accepted evidence and remaining obligations against the
canonical findings ledger.

### Windows Host Traps

On Windows, `python3` may resolve to a Microsoft Store `WindowsApps` execution
alias and `bash` may resolve to the WSL shim at `C:\Windows\System32\bash.exe`.
Treat both as unsuitable for long proof lanes unless the environment snapshot
marks them usable. Prefer:

```powershell
uv run --python 3.12 python tools/agent_coordination.py <command>
```

Environment reporting uses the canonical executable search over its captured
PATH, PATHEXT and current-directory policy. An absent or empty PATH disables
implicit search; empty components of a nonempty PATH name the current directory.
Windows environment names and executable suffix ordering follow the same rules
as toolchain selection, including `NoDefaultCurrentDirectoryInExePath`. An
untraversable candidate (permission denial or Windows error 448) is unavailable;
other I/O errors still propagate. Fixed Git Bash locations use the same candidate
predicate without extending those exact paths with further suffixes. Discovery
does not verify interpreter identity or authorize a launch: selected commands
still pass their normal executable and process-custody admission. An explicit
`PYTHON` remains a user recommendation, not verified capability evidence.

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

Group compatible repairs before Rust compilation. A build group must retain its
source and lockfile, Rust/SDK inputs, target, profile, feature set, flags and
build-script inputs; test count and finding count do not define build groups.
For compatible development features, compile the required test targets together
with `cargo test --no-run --message-format=json-render-diagnostics`, then run
focused selectors on the exact Cargo-reported executables under existing guards
and libtest accounting. Do not repeat `cargo check` as a warming ritual. A
development feature union does not qualify mutually exclusive features or the
shipped feature/profile matrix.

Use source checks and generator oracles before compilation. A generator-only
repair whose Rust outputs remain byte-identical needs no new Rust build unless
another compiled input changed. The unconditional `tools/generators.py check`
owns freshness for the manifest's reproducible generators; local rules add their
semantic checks and owning tests without repeating named freshness commands.
Generators that require external or host inputs retain their selected checks.
Reuse completed evidence for unchanged inputs; rebuild affected artifacts after
actual input changes. Target-Python versions
may share a Rust image only where version selection is dynamic; syntax oracles,
version-specific ABI layouts and actual matrix execution still need their own
evidence. Release/performance builds prove their distinct profile contracts and
must not be repeated for each fixture. These are development-side policies.

## Differential And Conformance Protocol

Differential and conformance lanes are shared resources.

`BackendExecutionContext` owns each test's target Python, build profile,
stdlib profile and capability/environment policy across native, WASM, LLVM and
Luau. CLI, metadata and environment selectors are resolved before dispatch;
native subprocess/batch and cross-backend argv consume the same stdlib-profile
parser. The context also carries one immutable `ItemBudget`, pinned before
serial or process-pool dispatch. Its target-specific `TimeoutPolicy` supplies
both admission reservation and actual phase caps. CPython, interpreter metadata,
LLVM provenance probes, startup ping, strict batch retries, native recoveries and
subprocess fallback consume its remaining time; worker startup and queue delay
never renew it. A standalone case creates that budget once, and warm-cache work
uses the same policy's native build-only bound. Suite preflight consumes the
absolute suite deadline and retains a normal not-run summary on exhaustion.
Successful probe facts remain cacheable independently of a deadline. These are
process-dispatch and guarded child execution bounds: mandatory child/lease
cleanup, filesystem calls and summary publication still require the outer guard's
headroom. Completed outcomes retain their typed failure precedence; exhaustion
during backend evaluation retains the selected backend rows. Suite evidence consumes
the actual execution environment’s explicitly selected marker through the shared
reader; an ambient artifact location cannot override an explicit context.
Per-backend result rows retain the selected profiles. Requested JSONL
receipt writes fail loudly; an unwritable evidence sink is not a passing run.

`BackendResult` preserves build/run timeout facts through every adapter. A guard
deadline takes precedence over its termination code; timeout and unavailable
targets cannot become expected semantic failures. Per-backend rows survive a
later backend failure, retaining return code, phase, timeout and diagnostic hash;
the aggregate log retains the diagnostic text. `--fail-fast` stops new test
admission on failure, OOM or an unavailable target and disables OOM retries.
Parallel execution admits at most `--jobs` tests, cancels only work that has not
started, and drains already-running results before closing the run.

- Keep one supervising broad diff/regrtest/conformance run per shared
  `CARGO_TARGET_DIR`.
- Let `tests/molt_diff.py` own its run lock at
  the target-addressed canonical build-state root's `diff_run.lock`
  (or target-local state without `MOLT_EXT_ROOT`); do not bypass it with raw
  loops.
- If a broad run is active, other agents should prefer:
  - implementation work that does not need the broad lane;
  - failure-queue reduction;
  - targeted reruns with a different target root only when explicitly useful;
  - docs/spec/matrix updates based on already captured evidence.
- If the diff lock would make an agent wait without adding signal, record the
  wait in the task log and move to non-colliding work.
- Resolve canonical roots through the DX authority and the current root policy
  in `docs/agent/ORCHESTRATION.md` before heavy maintainer, agent, benchmark,
  differential, conformance, or CI-style lanes. Do not infer another drive as a
  canonical capacity fallback. Public users may compile in place, use Molt/Cargo defaults,
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
