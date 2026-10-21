#!/usr/bin/env python3
"""Stop-hook dispatcher (APPARATUS Wave 1, A1).

Wired to the Claude Code ``Stop`` event. Runs every ``StopLeg`` in ``LEGS``.
Each leg's ``[[gate_flip]]`` decides what its reason does: under ``strict``
the FIRST reason is emitted as ``{"decision":"block","reason":...}`` on stdout
with exit 0 -- the verified Stop block contract
(code.claude.com/docs/en/hooks): a Stop block re-engages the agent with an
actionable nudge and NEVER wedges the session. Under ``warn`` (every leg today,
owner 2026-10-21: observe first) the reason becomes an observe-only record in
``.molt/state/stop_gates_shadow.jsonl`` and nothing reaches the session.

LOOP-SAFETY / COMPOSITION:
  * honors ``stop_hook_active`` (defensive ``.get``): if the harness is already
    continuing because of a stop hook, we stay silent (exit 0).
  * the triality and magnitude legs are block-once (via their own marker), so
    re-engagement is bounded and composes with the autonomous ``/goal`` loop.
    The anti-recurrence leg has no marker: it must gain one before its flip may
    go strict.
  * fail-open: any exception -> exit 0 (the wrapper in ``_common``).
"""

from __future__ import annotations

import json
import sys
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[2]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.hooks import _common  # noqa: E402
from tools import anti_recurrence_gate, triality_gate, magnitude_dismissal_gate  # noqa: E402
from tools.check_gate_flips import FlipState, flip_state  # noqa: E402


@dataclass(frozen=True, slots=True)
class StopLeg:
    """One Stop check. ``name`` is its ``[[gate_flip]]`` in tools/proof_plan.toml:
    under ``strict`` its reason re-engages the turn, under ``warn`` (or with
    no entry) the reason becomes an observe-only record and nothing reaches
    the session."""

    name: str
    evaluate: Callable[[dict, Path], str | None]


# Every Stop leg, in order; under ``strict`` the first reason is emitted. A leg
# raising is caught per-leg below (independent fail-open), so one leg's bug can
# never sink the others.
#   * anti_recurrence_gate (A7)      -- a COMPLETE bug-class claim with no teeth
#                                       or durable lesson (M05)
#   * triality_gate (A3)             -- keep the DAG/DSL/equations legs in sync (M63)
#   * magnitude_dismissal_gate (A6)  -- no absolute-magnitude dismissal / unscoped
#                                       verdict (M46/M47, M11, M16)
# All three read one session window per checkout, so they cannot tell this
# session's commits from another's (HF-424); each stays ``warn`` until it can.
# The landing gate (A2) and the session-learning writer are not wired (owner,
# 2026-10-21): the landing gate's [[gate_flip]] states what it needs (HF-424),
# and HF-427 tracks the session-learning redesign.
LEGS: tuple[StopLeg, ...] = (
    StopLeg("anti_recurrence_gate", anti_recurrence_gate.evaluate_stop),
    StopLeg("triality_gate", triality_gate.evaluate),
    StopLeg("magnitude_dismissal_gate", magnitude_dismissal_gate.evaluate),
)
SHADOW_NAME = "stop_gates_shadow.jsonl"


def enforcement_state(leg: StopLeg) -> FlipState:
    """The leg's ``[[gate_flip]]`` state; no entry or an unreadable plan is
    ``warn``, so a Stop never re-engages a turn by default."""
    try:
        return flip_state(leg.name) or "warn"
    except (OSError, ValueError):  # tomllib.TOMLDecodeError is a ValueError
        return "warn"


def run() -> int:
    data = _common.read_hook_input()

    # Loop guard: if we are already inside a stop-hook-induced continuation, do
    # not stack another block (prevents an infinite Stop loop / wedge).
    if data.get("stop_hook_active"):
        return 0

    cwd = data.get("cwd")
    root = _common.repo_root(cwd)

    for leg in LEGS:
        try:
            reason = leg.evaluate(data, root)
        except Exception as exc:  # one leg failing must not sink the others
            _common.log_error(f"stop_gates.{leg.name}", exc, root)
            continue
        if not reason:
            continue
        if enforcement_state(leg) == "strict":
            sys.stdout.write(
                json.dumps({"decision": "block", "reason": f"[{leg.name}] {reason}"})
                + "\n"
            )
            return 0
        _common.record_shadow(
            root,
            SHADOW_NAME,
            {
                "session_id": str(data.get("session_id", "")),
                "leg": leg.name,
                "reason": reason,
            },
        )
    return 0


def main() -> None:
    _common.run_fail_open("stop_gates", run)


if __name__ == "__main__":
    main()
