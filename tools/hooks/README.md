# molt Claude Code hooks

One hook runs: a digest of live facts at session start. No hook blocks,
delays or re-engages work (owner, 2026-10-21: the hooks exist to speed work
up, not to stop it).

## The hook

`wiring.py` declares each hook (event, matcher, script, timeout);
`tools/gen_claude_hooks.py` projects it into `.claude/settings.json`, which
Claude Code reads, and `generators.py check` keeps the two equal. Edit the
declaration, never the settings file.

| Event | Script | Job |
|---|---|---|
| `SessionStart` | `session_digest.py` | digest of live facts injected as context: pointers to AGENTS.md, the coordination context and the v1.0 ledger; the landing queue; the proof queue; live worktrees against `drift_harvest`'s SPRAWL threshold |

The 2026-10-21 owner decision retired the Bash guard, the Stop legs
(anti-recurrence, triality, magnitude-dismissal), the landing gate, session
learning and the waiver grammar. Git history keeps them.

`_common.py` holds the fail-open wrapper, the UTF-8 backstop and the proof
queue reader.

## Invariants

- **Fail open.** Any exception in a hook exits 0. The error goes to
  `.molt/state/<hook>_errors.log`; past an escalation threshold the hook also
  prints it on stderr, so a failing hook is never silent. Proven by
  `tests/tools/test_hooks_fail_open.py`.
- **Writes.** The digest writes nothing into the tracked tree or the user's
  Claude memory directory. Its own output is the error log. Its Git reads run
  through the project's guarded command path, which keeps that path's
  standard custody records (memory-guard locks and custody files under the
  artifact root). `tools/apparatus_agent_safety.py --check` scans the hook
  package for termination capability: bounded read-only `subprocess.run`
  calls are allowed; `Popen`, kill/terminate calls, signals and
  taskkill-style actuation are forbidden.
- **Windows.** ASCII-safe, UTF-8-explicit output (reuses
  `tools/_io_utf8.force_utf8_stdio`).
- **Interpreter.** Every settings command runs `sh tools/hooks/run.sh <hook>`,
  which starts Python the way the Git hooks do: it sources
  `.githooks/molt-hook-launch.sh`, which selects the common checkout's
  platform-native uv environment (`.venv/bin/python` or
  `.venv/Scripts/python.exe`) and binds imports to the invoking worktree. A
  linked worktree therefore needs no environment of its own. Claude Code's
  PATH need not carry `python`; the hook path runs only `sh`, `git` and
  `dirname`. A missing `run.sh`, launch script, environment or hook exits 1
  and names the missing piece. Sourcing a missing file in POSIX `sh` exits 2,
  so `run.sh` checks the launch script first. The POSIX cells are tested
  (`tests/tools/test_hooks_fail_open.py`); the Windows cell (Git for Windows
  `sh`) is unverified.

## Runtime state (`.molt/state/`, gitignored)

`<hook>_errors.log` only. Per-machine and per-worktree; never committed.

## Verified against the primary docs

The settings.json shape, the `SessionStart` stdout-as-context behavior and
the exit-code semantics were verified against
`https://code.claude.com/docs/en/hooks` (2026-07-10; the 2026-10-21 review
rechecked the exit codes and `CLAUDE_PROJECT_DIR`).
