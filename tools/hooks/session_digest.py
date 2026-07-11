#!/usr/bin/env python3
"""SessionStart digest (APPARATUS Wave 1, A1) -- molt's costate "sense organ".

A <5s, read-only, fail-open digest printed to stdout (which Claude Code injects
as session context). The agent starts every session already knowing the state
it would otherwise be reminded of:

  * the standing goal aperture (M01 witness-closure toward pact-collab green),
  * CLAIMS / proof_queue custody snapshot,
  * worktree / drift debt (count vs the M67 threshold),
  * a build-wall sample if one is recorded (M09),
  * the top standing directives the hook spine now enforces,
  * the apparatus footer (how to override / escape / waive).

NEVER fails the session: every section is independently guarded and the whole
thing runs under the fail-open wrapper. Also writes the landing-gate window
baseline (session-start HEAD) so the A2 Stop gate has a per-session anchor.
"""

from __future__ import annotations

import io
import json
import sys
from pathlib import Path

try:
    from tools.hooks import _common, landing_gate
except Exception:  # pragma: no cover - path-invocation fallback
    import os as _os

    sys.path.insert(0, _os.path.dirname(_os.path.dirname(_os.path.dirname(__file__))))
    from tools.hooks import _common, landing_gate


WORKTREE_DEBT_THRESHOLD = 24  # M67: 130+ worktrees = poison; keep the count low.

STANDING_DIRECTIVES = [
    "M12  LAND every work turn (commit/proof/passing test) or NAME a real blocker; reporting without landing is poison.",
    "M05  Zero fakes; independently verify every deliverable incl. subagents'; a PASS is a hypothesis until reproduced.",
    "M17  NEVER destructive git (reset --hard/checkout --/clean -fd/stash drop) on the shared checkout.",
    "M20  Commit with `git commit -- <pathspec>`; `git add X && git commit` sweeps other agents' WIP.",
    "M19  origin push MUST use SSH (git@github.com); HTTPS lacks `workflow` scope.",
    "M27  <=1-2 builds; route heavy builds through the proof_queue (OOM/contention class).",
    "M67  Harvest+prune worktree/branch drift EVERY session (tools/drift_harvest.py).",
]


def _section_goal(out: io.StringIO, root: Path) -> None:
    out.write("GOAL (M01, aperture=witness-closure):\n")
    out.write(
        "  witness -> WASM -> candidate_outputs.npz -> check_parity PASS "
        "(pact-collab 100-yr green).\n"
    )
    # Best-effort: surface the E1 witness lane state from CLAIMS if present.
    try:
        claims = (root / "docs" / "agent" / "CLAIMS.md").read_text(
            encoding="utf-8", errors="replace"
        )
        for line in claims.splitlines():
            if "E1-WITNESS-TO-GREEN" in line and (
                "CLAIMED" in line or "PROGRESS" in line
            ):
                out.write(f"  claim: {line.strip()[:120]}\n")
                break
    except Exception:
        pass


def _section_custody(out: io.StringIO, root: Path) -> None:
    out.write("CUSTODY:\n")
    # Active CLAIMS rows (cheap file scan).
    try:
        claims = (root / "docs" / "agent" / "CLAIMS.md").read_text(
            encoding="utf-8", errors="replace"
        )
        active = [
            ln.strip()
            for ln in claims.splitlines()
            if "CLAIMED" in ln and "RELEASED" not in ln
        ]
        out.write(f"  CLAIMS: {len(active)} active claim line(s).\n")
    except Exception:
        out.write("  CLAIMS: (unavailable)\n")
    # proof_queue active-row count (short cap; None = undeterminable).
    cnt = _common.proof_queue_active_count(root, timeout=3.5)
    if cnt is None:
        out.write("  proof_queue: status undeterminable.\n")
    else:
        out.write(f"  proof_queue: {cnt} active row(s).\n")


def _section_drift(out: io.StringIO, root: Path) -> None:
    count = _common.worktree_count(root)
    if count is None:
        out.write("DRIFT: worktree count unavailable.\n")
        return
    flag = (
        "  <-- OVER THRESHOLD, run tools/drift_harvest.py --prune"
        if count > WORKTREE_DEBT_THRESHOLD
        else ""
    )
    out.write(
        f"DRIFT: {count} worktree(s) (M67 threshold {WORKTREE_DEBT_THRESHOLD}).{flag}\n"
    )


def _section_build_wall(out: io.StringIO, root: Path) -> None:
    # Only report a real recorded sample; never fabricate (M10).
    for rel in (".molt/state/build_wall.json", "docs/agent/PERF_HOTSPOTS_20260710.md"):
        p = root / rel
        try:
            if p.exists() and p.suffix == ".json":
                data = json.loads(p.read_text(encoding="utf-8"))
                out.write(f"BUILD-WALL (M09): {data}\n")
                return
        except Exception:
            continue
    out.write(
        "BUILD-WALL (M09): no recorded sample; profile the hot path before claiming.\n"
    )


def _section_directives(out: io.StringIO) -> None:
    out.write("STANDING DIRECTIVES (now enforced by the hook spine):\n")
    for d in STANDING_DIRECTIVES:
        out.write(f"  - {d}\n")
    out.write("  (full index: memory/MEMORY.md + POINTERS.md)\n")


def _section_footer(out: io.StringIO) -> None:
    out.write("APPARATUS (Wave 1 hooks active):\n")
    out.write(
        "  bash_guard blocks destructive-git-on-shared / add-sweep / queue-bypass / https-push; "
        "override once-verified with MOLT_GUARD_OK=1 <cmd> (audited).\n"
    )
    out.write(
        "  Stop landing-gate: land a commit/proof, or "
        '`python tools/hooks/landing_gate.py --record-blocker "<reason>"`.\n'
    )
    out.write(
        "  Waiver grammar: `# <GATE>_OK:<rationale>` (>=4 real chars) or [skip-<gate>].\n"
    )


def run() -> int:
    data = _common.read_hook_input()
    cwd = data.get("cwd")
    root = _common.repo_root(cwd)
    session_id = str(data.get("session_id", ""))

    # Anchor the A2 landing-gate window for this session.
    try:
        landing_gate.write_session_baseline(root, session_id)
    except Exception:
        pass

    out = io.StringIO()
    out.write("== molt session digest (apparatus wave 1) ==\n")
    for section in (
        lambda: _section_goal(out, root),
        lambda: _section_custody(out, root),
        lambda: _section_drift(out, root),
        lambda: _section_build_wall(out, root),
        lambda: _section_directives(out),
        lambda: _section_footer(out),
    ):
        try:
            section()
        except Exception as exc:  # a bad section must not sink the digest
            out.write(f"  (section error: {type(exc).__name__})\n")
            _common.log_error("session_digest", exc, root)
    sys.stdout.write(out.getvalue())
    return 0


def main() -> None:
    _common.run_fail_open("session_digest", run)


if __name__ == "__main__":
    main()
