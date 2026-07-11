# molt hook spine (APPARATUS Wave 1)

Mechanical enforcement of the standing directives, so drift is caught by tooling
instead of operator reminders. Wired from `.claude/settings.json` to three
Claude Code lifecycle points. Full design: `docs/agent/APPARATUS_FROM_COMMA_LAB.md`
(sections A1, A2, A9); the standing directives are `memory/MEMORY.md` (M##).

## The hooks

| Event | Script | Job | Mechanizes |
|---|---|---|---|
| `SessionStart` | `session_digest.py` | <5s read-only digest (goal pointer, custody, drift debt, build-wall, standing directives) injected as context; writes the landing-gate window baseline | M01, M09, M67 |
| `PreToolUse` (Bash) | `bash_guard.py` | refuses destructive-git-on-shared-checkout, `git add` sweeps, build-bypasses-live-queue, https-push | M17, M18, M20, M27, M19 |
| `Stop` | `stop_gates.py` -> `landing_gate.py` | land-or-blocker nudge: a substantive turn that landed no commit / queue row / blocker is re-engaged | M12, M05 |

Supporting: `_common.py` (fail-open wrapper, UTF-8 backstop, locked jsonl,
git/queue helpers), `waivers.py` (A9 waiver grammar), `../check_gate_flips.py`
(A9 warn->strict auditor), `../check_gate_liveness.py` (canary: each gate still
fires on a known-bad fixture).

## Non-negotiable invariants

- **FAIL-OPEN.** Any exception in a hook -> ALLOW / `exit 0`. The session is
  never bricked. Errors are logged to `.molt/state/<hook>_errors.log` with a
  loud-escalation threshold so fail-open is never silent. Proven by
  `tests/tools/test_hooks_fail_open.py` (a raising `decide()` -> the wrapper
  allows).
- **Pure decision surface.** Every hook's `decide()` / `evaluate()` is a pure,
  separately unit-tested function; the fail-open wrapper is thin.
- **Loop-safe + event-triggered.** The `Stop` block is a re-engage NUDGE
  (exit 0), never a wedge: guarded by `stop_hook_active` + a persisted
  `last_block_head` marker (blocks at most once per HEAD state), silent when no
  substantive activity. It **composes with** (does not replace) the autonomous
  `/goal` Stop loop.
- **Windows.** `msvcrt` locks (not `fcntl`); ASCII-safe, UTF-8-explicit
  (reuses `tools/_io_utf8.force_utf8_stdio`, M43 cp1252 class). Stdlib-only, so
  the hooks run under any Python 3.9+ (the `command` cannot depend on the venv).

## Escape valves (never binary)

- **bash_guard override:** prefix the command with `MOLT_GUARD_OK=1` once you
  have verified it is safe. The override is audited to `.molt/state/waivers.jsonl`.
- **landing_gate:** land a commit/proof, or record the real blocker:
  `python tools/hooks/landing_gate.py --record-blocker "<reason>"`. A
  `[report-only]` commit-subject token also satisfies the window.
- **waiver grammar (A9):** inline `# <GATE>_OK:<rationale>` (>=4 real,
  non-placeholder chars) or a `[skip-<gate>]` commit-subject token. Honored
  waivers are appended to `.molt/state/waivers.jsonl`.

## Runtime state (`.molt/state/`, gitignored)

`landing_gate_marker.json` (per-session window base + `last_block_head`),
`blockers.jsonl`, `waivers.jsonl`, `<hook>_errors.log`. Per-machine /
per-worktree; never committed.

## Verified against the primary docs

The settings.json shape, the `Stop` block contract
(`{"decision":"block","reason":...}` on exit 0), the `PreToolUse` block via
exit-2-with-stderr, the `SessionStart` stdout-as-context behavior, and the
exit-code semantics were verified against `https://code.claude.com/docs/en/hooks`
(2026-07-10).
