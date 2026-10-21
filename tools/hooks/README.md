# molt hook spine (APPARATUS Wave 1)

Mechanical enforcement of the standing directives, so drift is caught by tooling
instead of operator reminders. Wired from `.claude/settings.json` to three
Claude Code lifecycle points. Full design: `docs/agent/APPARATUS_FROM_COMMA_LAB.md`
(sections A1, A2, A9); the standing directives are `memory/MEMORY.md` (M##).

## The hooks

| Event | Script | Job | Mechanizes |
|---|---|---|---|
| `SessionStart` | `session_digest.py` | read-only digest of live facts (pointers to AGENTS.md, the coordination context and the v1.0 ledger; landing queue; proof queue; live worktrees against `drift_harvest`'s SPRAWL threshold; what the hooks enforce, read from their wiring) injected as context; anchors the session window | M67 |
| `PreToolUse` (Bash) | `bash_guard.py` | judges destructive-git-on-shared-checkout, `git add` sweeps, build-bypasses-live-queue and https-push; observe-only until its gate_flip is strict (below) | M17, M18, M20, M27, M19 |
| `Stop` | `stop_gates.py` | warns on uncaptured bug-class learning (advisory), then applies the triality and verdict-scope gates | M05, M22 |

`landing_gate.py` (A2) and `session_learning.py` are not wired (owner,
2026-10-21). The landing gate could not attribute landing activity to a
session, and session learning wrote into the user's memory directory on most
Stops. The landing gate's `[[gate_flip]]` in `tools/proof_plan.toml` states
what it needs before it returns, and HF-424 tracks the per-session window it
needs; HF-427 tracks the session-learning redesign. `tools/recurring_harms.toml`
names those rows as the `gap` of each harm whose hook guard does not enforce,
and `tools/check_gate_liveness.py` checks that claim against the live wiring.

Supporting: `_common.py` (fail-open wrapper, UTF-8 backstop, locked jsonl,
git/queue helpers), `waivers.py` (A9 waiver grammar), `../check_gate_flips.py`
(A9 warn->strict auditor), `../check_gate_liveness.py` (canary: each gate still
fires on a known-bad fixture), and `../recurring_harms.toml` (one canonical
harm -> guard -> live-canary registry).

## Non-negotiable invariants

- **FAIL-OPEN.** Any exception in a hook -> ALLOW / `exit 0`. The session is
  never bricked. Errors are logged to `.molt/state/<hook>_errors.log` with a
  loud-escalation threshold so fail-open is never silent. Proven by
  `tests/tools/test_hooks_fail_open.py` (a raising `decide()` -> the wrapper
  allows).
- **Pure decision surface.** Every hook's `decide()` / `evaluate()` is a pure,
  separately unit-tested function; the fail-open wrapper is thin.
- **No process actuation.** `tools/apparatus_agent_safety.py --check` scans the
  hook spine and apparatus guards for termination capability; read-only bounded
  observations may use `subprocess.run`, but `Popen`, kill/terminate calls,
  signals, and taskkill-style actuation are forbidden.
- **Loop-safe + event-triggered.** The `Stop` block is a re-engage NUDGE
  (exit 0), never a wedge: guarded by `stop_hook_active` and by each leg's own
  `EventWindow` marker (blocks at most once per HEAD state), silent when no
  commit in the window touches its subject. It **composes with** (does not
  replace) the autonomous `/goal` Stop loop.
- **Observe before enforce.** `bash_guard` reads its `[[gate_flip]]` state in
  `tools/proof_plan.toml` (`bash_guard.enforcement_state`). Under `warn` it
  refuses nothing: it appends each would-be block to
  `.molt/state/bash_guard_shadow.jsonl` and exits 0. Under `strict` it exits 2
  with the reason. A missing or unreadable entry means `warn`. Its strict
  condition is a replay of recorded session commands with zero false blocks.
- **Prefilter.** `run.sh` and `bash_guard.run` skip the guard when the payload
  or command holds none of `bash_guard.PREFILTER_TOKENS` (git, cargo, build);
  this saves about 145 ms on each other Bash call. The filters read text and
  the guard reads shell words, so an escaped spelling (`c\argo b`) passes
  unseen (HF-423, pinned by a strict xfail test). The guard may not go strict
  until one shell-word model owns both.
- **Windows.** `msvcrt` locks (not `fcntl`); ASCII-safe, UTF-8-explicit
  (reuses `tools/_io_utf8.force_utf8_stdio`, M43 cp1252 class).
- **Interpreter.** Every settings command runs `sh tools/hooks/run.sh <hook>`,
  which starts Python the way the Git hooks do: it sources
  `.githooks/molt-hook-launch.sh`, which selects the common checkout's
  platform-native uv environment (`.venv/bin/python` or
  `.venv/Scripts/python.exe`) and binds imports to the invoking worktree. A
  linked worktree therefore needs no environment of its own. Claude Code's
  PATH need not carry `python`; the hook path runs only `sh`, `git`, `cat` and
  `dirname`. A missing launcher, environment or hook exits 1 with the remedy:
  fail-open and visible, never 2 (which would block every Bash call). The
  POSIX cells are tested (`tests/tools/test_hooks_fail_open.py`); the Windows
  cell (Git for Windows `sh`) is unverified.

## Escape valves (never binary)

- **bash_guard override:** under `strict`, prefix the command with
  `MOLT_GUARD_OK=1` once you have verified it is safe. The override is audited
  to `.molt/state/waivers.jsonl`.
- **waiver grammar (A9):** inline `# <GATE>_OK:<rationale>` (>=4 real,
  non-placeholder chars) or a `[skip-<gate>]` commit-subject token. Honored
  waivers are appended to `.molt/state/waivers.jsonl`.

## Runtime state (`.molt/state/`, gitignored)

`landing_gate_marker.json` (the session window: the HEAD at session start,
which the anti-recurrence advisory diffs against; one file per checkout, so
concurrent sessions share it: HF-424), `bash_guard_shadow.jsonl`,
`waivers.jsonl`, each Stop leg's `*_marker.json`, `<hook>_errors.log`.
Per-machine / per-worktree; never committed.

## Verified against the primary docs

The settings.json shape, the `Stop` block contract
(`{"decision":"block","reason":...}` on exit 0), the `PreToolUse` block via
exit-2-with-stderr, the `SessionStart` stdout-as-context behavior, and the
exit-code semantics were verified against `https://code.claude.com/docs/en/hooks`
(2026-07-10).
