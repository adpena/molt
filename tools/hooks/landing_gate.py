#!/usr/bin/env python3
"""Land-or-blocker Stop gate (APPARATUS Wave 1, A2) -- M12 mechanized.

M12 is the operator's #1 fury: *reporting/planning/handing-off without LANDING
is POISON. Every work turn lands a commit / proof / passing test, or names a
real external blocker.* This gate turns that into a Stop-hook mechanism.

Per-worktree window = **session-start HEAD .. current HEAD** (a marker in
``.molt/state/landing_gate_marker.json``, written by ``session_digest`` at
SessionStart and re-baselined defensively here). If a turn produced substantive
tool activity but the window shows:
  * NO landed commit, AND
  * NO proof_queue row in flight, AND
  * NO named blocker in ``.molt/state/blockers.jsonl``,
then it BLOCKS with a firm re-engage nudge (M12 + M05: a PASS is a hypothesis
until reproduced -- the gate asks for the artifact).

Escape valves (both logged): a ``[report-only]`` commit-subject token in the
window, or a blocker row (``landing_gate.py --record-blocker "reason"``).

LOOP-SAFE: this is a re-engage NUDGE, never a wedge. It composes with (does not
replace) the autonomous ``/goal`` Stop loop: it blocks at most ONCE per HEAD
state (``last_block_head`` marker), honors ``stop_hook_active``, and is silent
when nothing substantive happened. ``decide()`` is pure + unit-tested; the
wrapper is fail-open.
"""

from __future__ import annotations

import json
import subprocess
import sys
import time
from pathlib import Path

try:
    from tools.hooks import _common
    from tools.hooks.waivers import record_waiver
except Exception:  # pragma: no cover - path-invocation fallback
    import os as _os

    sys.path.insert(0, _os.path.dirname(_os.path.dirname(_os.path.dirname(__file__))))
    from tools.hooks import _common
    from tools.hooks.waivers import record_waiver


MARKER_NAME = "landing_gate_marker.json"
BLOCKERS_NAME = "blockers.jsonl"
SUBSTANTIVE_TOOL_THRESHOLD = 2

BLOCK_MESSAGE = (
    "This turn reported without landing. Land a commit / proof / passing test via "
    "the queue (tools/proof_queue.py, tools/ff_land.py), or record the real external "
    'blocker (python tools/hooks/landing_gate.py --record-blocker "<reason>"), then '
    "stop. Reporting/planning without landing is the standing M12 violation; a PASS "
    "is a hypothesis until reproduced (M05)."
)


# --- pure decision surface -------------------------------------------------


def decide(
    *,
    substantive_activity: bool,
    window_has_commit: bool,
    queue_row_in_flight: bool,
    blocker_recorded: bool,
    report_only: bool,
) -> str | None:
    """Return the block reason, or None to allow. PURE + unit-tested.

    A turn is allowed when it was not substantive, OR it landed a commit, OR it
    has a queue row in flight, OR it named a blocker, OR it is marked
    ``[report-only]``. Otherwise it reported without landing -> block.
    """
    if not substantive_activity:
        return None
    if window_has_commit or queue_row_in_flight or blocker_recorded or report_only:
        return None
    return BLOCK_MESSAGE


# --- fact gathering (best-effort; uncertainty fails OPEN = allow) -----------


def _marker_path(root: Path) -> Path:
    return _common.state_dir(root) / MARKER_NAME


def _read_marker(root: Path) -> dict:
    try:
        p = _marker_path(root)
        if p.exists():
            return json.loads(p.read_text(encoding="utf-8"))
    except Exception:
        pass
    return {}


def _write_marker(root: Path, marker: dict) -> None:
    try:
        _marker_path(root).write_text(
            json.dumps(marker, ensure_ascii=True), encoding="utf-8"
        )
    except Exception:
        pass


def write_session_baseline(root: Path, session_id: str) -> None:
    """Called by session_digest at SessionStart: record the window base HEAD."""
    _write_marker(
        root,
        {
            "session_id": session_id,
            "start_head": _common.git_head(root),
            "start_ts": time.time(),
            "last_block_head": None,
        },
    )


def _window_has_commit(root: Path, start_head: str | None) -> bool:
    if not start_head:
        return False
    head = _common.git_head(root)
    return bool(head) and head != start_head


def _window_subjects(root: Path, start_head: str | None) -> str:
    if not start_head:
        return ""
    try:
        r = subprocess.run(
            ["git", "log", f"{start_head}..HEAD", "--format=%s"],
            cwd=str(root),
            capture_output=True,
            text=True,
            timeout=5,
        )
        return r.stdout if r.returncode == 0 else ""
    except Exception:
        return ""


def _queue_row_in_flight(root: Path) -> bool:
    """True if a proof_queue row is active. Uncertain -> True (fail-open allow)."""
    cnt = _common.proof_queue_active_count(root, timeout=4.0)
    if cnt is None:
        return True  # cannot determine -> do not block on this axis
    return cnt > 0


def _blocker_recorded(root: Path, session_id: str, start_ts: float) -> bool:
    rows = _common.read_jsonl(_common.state_dir(root) / BLOCKERS_NAME)
    for row in rows:
        if session_id and row.get("session_id") == session_id:
            return True
        ets = row.get("epoch_ts")
        if isinstance(ets, (int, float)) and ets >= start_ts:
            return True
    return False


def _count_tool_uses(transcript_path: str | None) -> int:
    if not transcript_path:
        return 0
    try:
        p = Path(transcript_path)
        if not p.exists():
            return 0
        # Read at most the last ~4MB to stay fast on long sessions.
        size = p.stat().st_size
        with open(p, "rb") as fh:
            if size > 4_000_000:
                fh.seek(size - 4_000_000)
            blob = fh.read().decode("utf-8", errors="replace")
        return blob.count('"tool_use"')
    except Exception:
        return 0


# --- evaluate (called by stop_gates) ---------------------------------------


def evaluate(data: dict, root: Path) -> str | None:
    """Compute facts, apply block-once, return a block reason or None.

    Handles its own marker state so ``stop_gates`` only prints the block JSON.
    """
    session_id = str(data.get("session_id", ""))
    transcript_path = data.get("transcript_path")

    marker = _read_marker(root)
    # Establish / re-baseline the window for a new (or unknown) session.
    if not marker or marker.get("session_id") != session_id:
        write_session_baseline(root, session_id)
        return None  # first Stop of the session sets the baseline, never blocks

    start_head = marker.get("start_head")
    start_ts = marker.get("start_ts") or 0.0

    substantive = _count_tool_uses(transcript_path) >= SUBSTANTIVE_TOOL_THRESHOLD
    window_commit = _window_has_commit(root, start_head)
    subjects = _window_subjects(root, start_head) if window_commit else ""
    report_only = "[report-only]" in subjects
    queue_flight = _queue_row_in_flight(root)
    blocker = _blocker_recorded(root, session_id, float(start_ts))

    reason = decide(
        substantive_activity=substantive,
        window_has_commit=window_commit,
        queue_row_in_flight=queue_flight,
        blocker_recorded=blocker,
        report_only=report_only,
    )
    if reason is None:
        return None

    # block-once per HEAD state: never re-block the identical position (no wedge).
    head = _common.git_head(root)
    if marker.get("last_block_head") == head:
        return None
    marker["last_block_head"] = head
    _write_marker(root, marker)
    if report_only:
        record_waiver(
            "landing_gate",
            "report-only window token",
            source="commit-token",
            context=subjects[:400],
            root=root,
        )
    return reason


# --- blocker recording CLI (the M12 escape valve) --------------------------


def record_blocker(root: Path, text: str, session_id: str = "") -> None:
    _common.append_jsonl(
        _common.state_dir(root) / BLOCKERS_NAME,
        {
            "session_id": session_id,
            "epoch_ts": time.time(),
            "blocker": text,
        },
    )


def _cli(argv: list[str]) -> int:
    import argparse

    ap = argparse.ArgumentParser(prog="landing_gate")
    ap.add_argument(
        "--record-blocker",
        metavar="REASON",
        help="append a named external blocker to .molt/state/blockers.jsonl "
        "(the M12 escape valve: a real blocker satisfies the landing gate)",
    )
    ap.add_argument(
        "--session", default="", help="optional session id to tag the blocker"
    )
    args = ap.parse_args(argv)
    root = _common.repo_root()
    if args.record_blocker:
        if len(args.record_blocker.strip()) < 8:
            sys.stderr.write(
                "landing_gate: --record-blocker needs a real reason (>=8 chars)\n"
            )
            return 1
        record_blocker(root, args.record_blocker.strip(), args.session)
        print(f"blocker recorded: {args.record_blocker.strip()}")
        return 0
    ap.print_help()
    return 0


if __name__ == "__main__":
    sys.exit(_cli(sys.argv[1:]))
