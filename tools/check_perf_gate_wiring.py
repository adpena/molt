#!/usr/bin/env python3
"""Fail-closed audit: the canonical perf gate MUST be wired to fire on main.

The meta-bug this kills (the TIER-0 / sharpest finding of the verification-machinery
audit): .github/workflows/perf-gate.yml is the ONE release-blocking CPython-floor
scoreboard, but its triggers were workflow_dispatch + a weekly cron only -- it never
ran on a PR or a merge to main. So every perf-green was vacuous and the
perf-authority drift-gate certified nothing. ci.yml running tests/tools/
test_perf_authority.py is a *unit test of the authority module* -- a PROXY for
"the gate ran", which is the master meta-bug class: PROXY-MEASUREMENT SUBSTITUTION
(a verifier measures a cheap proxy correlated with the real invariant on the happy
path and decorrelated exactly where the bug lives).

This checker replaces the proxy with a check on the real thing: the canonical gate
must (a) invoke the real scoreboard and (b) actually fire on main. It is wired into
ci_gate tier-1 (via tests/tools/test_check_perf_gate_wiring.py) so an un-wiring
cannot silently regress.
"""

from __future__ import annotations

import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
PERF_GATE = REPO / ".github" / "workflows" / "perf-gate.yml"
SCOREBOARD_CMD = "perf_scoreboard.py"


def _load_yaml(path: Path):
    import yaml  # pyyaml is a dev dependency

    return yaml.safe_load(path.read_text(encoding="utf-8"))


def _triggers(doc: object) -> dict:
    # YAML 1.1 coerces the bare key `on` to the boolean True; handle both spellings
    # so the audit is not itself fooled by the parse (a meta-meta-bug).
    if isinstance(doc, dict):
        if "on" in doc:
            return doc["on"] or {}
        if True in doc:
            return doc[True] or {}
    return {}


def check() -> list[str]:
    """Return a list of wiring problems; empty list == correctly wired."""
    if not PERF_GATE.exists():
        return [f"{PERF_GATE} is missing -- the canonical perf gate does not exist"]
    text = PERF_GATE.read_text(encoding="utf-8")
    try:
        doc = _load_yaml(PERF_GATE)
    except Exception as exc:  # noqa: BLE001 - any parse failure is a wiring failure
        return [f"perf-gate.yml does not parse as YAML: {exc}"]

    problems: list[str] = []
    triggers = _triggers(doc)

    # (1) It must invoke the REAL scoreboard, not a stand-in.
    if SCOREBOARD_CMD not in text:
        problems.append(
            f"perf-gate.yml never invokes {SCOREBOARD_CMD} -- it is not the canonical gate"
        )

    # (2) It must actually FIRE on main: a push to main (release gate) and/or a PR.
    push = triggers.get("push") or {}
    push_branches = push.get("branches") if isinstance(push, dict) else None
    fires_on_main_push = bool(push_branches) and "main" in push_branches
    fires_on_pr = "pull_request" in triggers
    if not (fires_on_main_push or fires_on_pr):
        problems.append(
            "perf-gate.yml does not fire on a pull_request or a push to main -- its "
            f"only triggers are {sorted(map(str, triggers))!r}. The canonical perf gate "
            "certifies NOTHING on any merge; every perf-green is vacuous. Add "
            "`push: {branches: [main]}` (and/or pull_request) to its `on:` block."
        )

    return problems


def main() -> int:
    problems = check()
    if problems:
        print("perf-gate-wiring: FAIL -- the canonical perf gate is not wired to main:")
        for p in problems:
            print(f"  - {p}")
        print(
            "  (a gate that never fires certifies nothing -- proxy-measurement substitution.)"
        )
        return 1
    print(
        "perf-gate-wiring: OK -- canonical perf gate fires on main and runs the scoreboard."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
