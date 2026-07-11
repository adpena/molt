#!/usr/bin/env python3
"""PreToolUse adapter for the canonical forbidden-checkout path policy."""

from __future__ import annotations

import sys

try:
    from tools import forbidden_checkout_guard
    from tools.hooks import _common
except Exception:
    import os as _os

    sys.path.insert(0, _os.path.dirname(_os.path.dirname(_os.path.dirname(__file__))))
    from tools import forbidden_checkout_guard
    from tools.hooks import _common


def run() -> int:
    data = _common.read_hook_input()
    decision = forbidden_checkout_guard.decide(
        str(data.get("tool_name", "")),
        data.get("tool_input") if isinstance(data.get("tool_input"), dict) else {},
        str(data.get("cwd", "")),
    )
    if decision.block:
        print(f"BLOCKED [{decision.rule}]: {decision.reason}", file=sys.stderr)
        return 2
    return 0


def main() -> None:
    _common.run_fail_open("path_guard", run)


if __name__ == "__main__":
    main()
