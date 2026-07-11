#!/usr/bin/env python3
"""Stop-hook dispatcher (APPARATUS Wave 1, A1).

Wired to the Claude Code ``Stop`` event. Runs the ordered list of Stop-leg
gates; the FIRST that returns a block reason is emitted as
``{"decision":"block","reason":...}`` on stdout with exit 0 -- the verified Stop
block contract (code.claude.com/docs/en/hooks): a Stop block re-engages the
agent with an actionable nudge and NEVER wedges the session.

Wave 1 wires exactly one leg: the A2 landing gate. The ``GATES`` list is the
clean seam for Wave 2's triality drift detector (A3) and magnitude-dismissal
detector (A6) -- each a ``(name, evaluate)`` pair with the same
``evaluate(data, root) -> str | None`` shape.

LOOP-SAFETY / COMPOSITION:
  * honors ``stop_hook_active`` (defensive ``.get``): if the harness is already
    continuing because of a stop hook, we stay silent (exit 0).
  * each leg is individually block-once (via its own marker), so re-engagement
    is bounded and composes with the autonomous ``/goal`` loop rather than
    fighting it.
  * fail-open: any exception -> exit 0 (the wrapper in ``_common``).
"""

from __future__ import annotations

import json
import sys

try:
    from tools.hooks import _common, landing_gate
    from tools import triality_gate, magnitude_dismissal_gate
except Exception:  # pragma: no cover - path-invocation fallback
    import os as _os

    sys.path.insert(0, _os.path.dirname(_os.path.dirname(_os.path.dirname(__file__))))
    from tools.hooks import _common, landing_gate
    from tools import triality_gate, magnitude_dismissal_gate


# Ordered Stop-leg gates. Each is a ``(name, evaluate(data, root) -> str|None)``
# pair; the FIRST to return a reason is emitted and the rest are skipped this
# turn. A leg raising is caught per-leg below (independent fail-open), so one
# leg's bug can never sink the others.
#   * landing_gate (A2)              -- land-or-name-a-blocker (M12)
#   * triality_gate (A3)             -- keep the DAG/DSL/equations legs in sync (M63)
#   * magnitude_dismissal_gate (A6)  -- no absolute-magnitude dismissal / unscoped
#                                       verdict (M46/M47, M11, M16)
GATES = [
    ("landing_gate", landing_gate.evaluate),
    ("triality_gate", triality_gate.evaluate),
    ("magnitude_dismissal_gate", magnitude_dismissal_gate.evaluate),
]


def run() -> int:
    data = _common.read_hook_input()

    # Loop guard: if we are already inside a stop-hook-induced continuation, do
    # not stack another block (prevents an infinite Stop loop / wedge).
    if data.get("stop_hook_active"):
        return 0

    cwd = data.get("cwd")
    root = _common.repo_root(cwd)

    for name, evaluate in GATES:
        try:
            reason = evaluate(data, root)
        except Exception as exc:  # one leg failing must not sink the others
            _common.log_error(f"stop_gates.{name}", exc, root)
            reason = None
        if reason:
            sys.stdout.write(
                json.dumps({"decision": "block", "reason": f"[{name}] {reason}"}) + "\n"
            )
            return 0
    return 0


def main() -> None:
    _common.run_fail_open("stop_gates", run)


if __name__ == "__main__":
    main()
