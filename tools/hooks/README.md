# molt hook spine (APPARATUS Wave 1)

Mechanical enforcement of the standing directives, so drift is caught by tooling
instead of operator reminders. Wired to three Claude Code lifecycle points.
Full design: `docs/agent/APPARATUS_FROM_COMMA_LAB.md` (sections A1, A2, A9);
the standing directives are `memory/MEMORY.md` (M##).

## The hooks

`wiring.py` declares each hook (event, matcher, script, timeout);
`tools/gen_claude_hooks.py` projects it into `.claude/settings.json`, which
Claude Code reads, and `generators.py check` keeps the two equal. Edit the
declaration, never the settings file.

| Event | Script | Job | Mechanizes |
|---|---|---|---|
| `SessionStart` | `session_digest.py` | read-only digest of live facts (pointers to AGENTS.md, the coordination context and the v1.0 ledger; landing queue; proof queue; live worktrees against `drift_harvest`'s SPRAWL threshold; each hook's live enforcement state, its would-be blocks of the last 7 days, and the waiver grammar) injected as context; anchors the session window when a conversation starts or clears | M67 |
| `PreToolUse` (Bash) | `bash_guard.py` | judges destructive-git-on-shared-checkout, `git add` sweeps, build-bypasses-live-queue and https-push | M17, M18, M20, M27, M19 |
| `Stop` | `stop_gates.py` | runs each `StopLeg` in `LEGS`: anti-recurrence (A7), triality (A3), magnitude-dismissal (A6) | M05, M22, M63, M46 |

`landing_gate.py` (A2) and `session_learning.py` are not wired (owner,
2026-10-21). The landing gate could not attribute landing activity to a
session, and session learning wrote into the user's memory directory on most
Stops. The landing gate's `[[gate_flip]]` in `tools/proof_plan.toml` states
what it needs before it returns, and HF-424 tracks the per-session window it
needs; HF-427 tracks the session-learning redesign. `tools/recurring_harms.toml`
names those rows as the `gap` of each harm whose hook-run guard does not
enforce, and `tools/check_gate_liveness.py` checks that claim against the
typed wiring.

Supporting: `_common.py` (fail-open wrapper, UTF-8 backstop, locked jsonl,
the observe-only record log, git/queue helpers), `waivers.py` (the one A9
waiver grammar), `../check_gate_flips.py` (the one `[[gate_flip]]` reader,
`flip_state`, and the warn->strict auditor), `../check_gate_liveness.py`
(canary: each gate still fires on a known-bad fixture, and each harm's
declared gap matches the wiring), and `../recurring_harms.toml` (one canonical
harm -> guard -> live-canary registry).

## Non-negotiable invariants

- **FAIL-OPEN.** Any exception in a hook -> ALLOW / `exit 0`. The session is
  never bricked. Errors are logged to `.molt/state/<hook>_errors.log` with a
  loud-escalation threshold. Proven by `tests/tools/test_hooks_fail_open.py`
  (a raising `decide()` -> the wrapper allows).
- **Pure decision surface.** Every hook's `decide()` / `evaluate()` is a pure,
  separately unit-tested function; the fail-open wrapper is thin. The guard's
  host facts (shared checkout, live queue, HTTPS origin) are a `GuardFacts`
  of lazy readers that a rule calls only when it reaches a command that needs
  one, so no text predicate outside the rules can skip a rule.
- **No process actuation.** `tools/apparatus_agent_safety.py --check` scans the
  hook spine and apparatus guards for termination capability; read-only bounded
  observations may use `subprocess.run`, but `Popen`, kill/terminate calls,
  signals, and taskkill-style actuation are forbidden.
- **Observe before enforce.** Every guard and Stop leg reads its
  `[[gate_flip]]` through `check_gate_flips.flip_state`, the one reader, which
  refuses a misspelled state. Under `strict` the guard exits 2 and a Stop leg
  re-engages the turn (the first strict reason, at most once per HEAD for the
  marker-backed legs). Under `warn`, with no entry, or with an unreadable plan,
  nothing reaches the session: the would-be block becomes a record in
  `.molt/state/bash_guard_shadow.jsonl` or `stop_gates_shadow.jsonl` (with the
  session and the time; each log rotates past 4 MiB), and the digest reports
  the last 7 days. Every guard and leg is `warn` today (owner, 2026-10-21:
  observe first). Each flip's `strict_when` says what must hold first; for
  the Stop legs that includes the per-session window (HF-424), because today
  another session's commits can trip them.
- **Prefilter.** `run.sh` skips Python when the raw payload holds none of
  `bash_guard.PREFILTER_TOKENS` (git, cargo, build) in any case: about 15 ms
  instead of 160 ms. Its test is `prefilter.generated.sh`, generated from that
  tuple, so the two cannot drift; the payload includes the description and
  paths, so a call whose description names a token pays the full cost. The
  guard matches executable names without case (`GIT` runs git on a
  case-insensitive filesystem). The filters read text and the guard reads
  shell words, so an escaped or quote-split spelling (`c\argo b`,
  `g'i't reset --hard`) passes unseen (HF-423, pinned by a strict xfail test).
  The guard may not go strict until one shell-word model owns both.
- **Windows.** `msvcrt` locks (not `fcntl`); ASCII-safe, UTF-8-explicit
  (reuses `tools/_io_utf8.force_utf8_stdio`, M43 cp1252 class).
- **Interpreter.** Every settings command runs `sh tools/hooks/run.sh <hook>`,
  which starts Python the way the Git hooks do: it sources
  `.githooks/molt-hook-launch.sh`, which selects the common checkout's
  platform-native uv environment (`.venv/bin/python` or
  `.venv/Scripts/python.exe`) and binds imports to the invoking worktree. A
  linked worktree therefore needs no environment of its own. Claude Code's
  PATH need not carry `python`; the hook path runs only `sh`, `git`, `cat` and
  `dirname`. A missing launcher, prefilter, environment or hook exits 1 with
  the remedy: fail-open and visible, never 2 (which would block every Bash
  call; sourcing a missing file in POSIX `sh` exits 2, so each sourced file is
  checked first). The POSIX cells are tested
  (`tests/tools/test_hooks_fail_open.py`); the Windows cell (Git for Windows
  `sh`) is unverified.

## Escape valves (never binary)

- **bash_guard override:** under `strict`, prefix the command with
  `MOLT_GUARD_OK=1` once you have verified it is safe. The override is audited
  to `.molt/state/waivers.jsonl`.
- **waiver grammar (A9), one classifier in `waivers.py`:** inline
  `# <GATE>_OK: <rationale>` on the flagged line, or `[skip-<gate>] <rationale>`
  in a commit subject (`commit_opt_out`; `[skip-triality]`,
  `[skip-magnitude-dismissal]`). The rationale must follow the token and carry
  at least 4 real characters; a bare token is refused. Honored waivers are
  appended to `.molt/state/waivers.jsonl`.

## Runtime state (`.molt/state/`, gitignored)

`landing_gate_marker.json` (the session window: the HEAD when a conversation
started, which the Stop legs diff against; one file per checkout, so
concurrent sessions share it: HF-424), `bash_guard_shadow.jsonl` and
`stop_gates_shadow.jsonl` (observe-only records, rotated to `.1`),
`waivers.jsonl`, each Stop leg's `*_marker.json`, `<hook>_errors.log`.
Per-machine / per-worktree; never committed.

## Verified against the primary docs

The settings.json shape, the `Stop` block contract
(`{"decision":"block","reason":...}` on exit 0), the `PreToolUse` block via
exit-2-with-stderr, the `SessionStart` stdout-as-context behavior and its
`source` field, and the exit-code semantics were verified against
`https://code.claude.com/docs/en/hooks` (2026-07-10; the 2026-10-21 review
rechecked the exit codes and `CLAUDE_PROJECT_DIR`).
