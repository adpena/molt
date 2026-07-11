#!/usr/bin/env python3
"""Gate-liveness canary for the apparatus itself (APPARATUS Wave 1; A8's P4 rule).

molt has repeatedly hit the "gate green atop a broken thing" class (M42), and
the configured-!=-effective class (M34). A gate that can no longer FIRE certifies
nothing. This tool is the binding-vs-inert audit pointed at the hook spine's OWN
gates: it feeds each pure decision surface a KNOWN-BAD input and asserts the gate
still fires. If any canary goes silent, the apparatus has silently rotted and
this tool fails (exit 1 under ``--check``).

Every canary calls a PURE ``decide()`` -- no subprocess, no environment -- so the
liveness check is deterministic and fast.
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.hooks import bash_guard, landing_gate  # noqa: E402
from tools.hooks import waivers  # noqa: E402
from tools import triality_gate, magnitude_dismissal_gate  # noqa: E402


@dataclass
class Canary:
    gate: str
    name: str
    fires: Callable[[], bool]  # True == the gate correctly fired on known-bad input


def _canaries() -> list[Canary]:
    return [
        Canary(
            "bash_guard",
            "destructive-git-on-shared-checkout",
            lambda: (
                bash_guard.decide(
                    "git reset --hard HEAD",
                    in_shared_checkout=True,
                    queue_live=False,
                    origin_is_https=False,
                ).block
            ),
        ),
        Canary(
            "bash_guard",
            "git-add-sweep",
            lambda: (
                bash_guard.decide(
                    "git add -A",
                    in_shared_checkout=False,
                    queue_live=False,
                    origin_is_https=False,
                ).block
            ),
        ),
        Canary(
            "bash_guard",
            "add-then-commit-sweep",
            lambda: (
                bash_guard.decide(
                    'git add foo.rs && git commit -m "x"',
                    in_shared_checkout=False,
                    queue_live=False,
                    origin_is_https=False,
                ).block
            ),
        ),
        Canary(
            "bash_guard",
            "build-bypasses-live-queue",
            lambda: (
                bash_guard.decide(
                    "cargo build --release",
                    in_shared_checkout=False,
                    queue_live=True,
                    origin_is_https=False,
                ).block
            ),
        ),
        Canary(
            "bash_guard",
            "https-push-to-origin",
            lambda: (
                bash_guard.decide(
                    "git push origin main",
                    in_shared_checkout=False,
                    queue_live=False,
                    origin_is_https=True,
                ).block
            ),
        ),
        Canary(
            "bash_guard",
            "override-neutralizes-block",
            # A known-GOOD counter-fixture: the override must turn a block OFF.
            lambda: (
                not bash_guard.decide(
                    "MOLT_GUARD_OK=1 git reset --hard HEAD",
                    in_shared_checkout=True,
                    queue_live=False,
                    origin_is_https=False,
                ).block
            ),
        ),
        Canary(
            "landing_gate",
            "report-without-landing",
            lambda: (
                landing_gate.decide(
                    substantive_activity=True,
                    window_has_commit=False,
                    queue_row_in_flight=False,
                    blocker_recorded=False,
                    report_only=False,
                )
                is not None
            ),
        ),
        Canary(
            "landing_gate",
            "landed-commit-allows",
            lambda: (
                landing_gate.decide(
                    substantive_activity=True,
                    window_has_commit=True,
                    queue_row_in_flight=False,
                    blocker_recorded=False,
                    report_only=False,
                )
                is None
            ),
        ),
        Canary(
            "waivers",
            "placeholder-rationale-rejected",
            lambda: not waivers.is_valid_rationale("todo"),
        ),
        Canary(
            "waivers",
            "real-rationale-accepted",
            lambda: waivers.is_valid_rationale(
                "origin remote intentionally https for CI mirror"
            ),
        ),
        # --- Wave 2, A3: triality drift gate -------------------------------
        Canary(
            "triality_gate",
            "bug-fix-without-net-fires",
            lambda: bool(
                triality_gate.window_drift(
                    ["fix(runtime): PyLong_AsLong silent truncation on >2**46"],
                    ["runtime/molt-lang-cpython-abi/src/numbers.rs"],
                )
            ),
        ),
        Canary(
            "triality_gate",
            "bug-fix-with-test-allows",
            # known-GOOD counter-fixture: a test in the window neutralizes the drift.
            lambda: (
                not triality_gate.window_drift(
                    ["fix(runtime): silent truncation"],
                    [
                        "runtime/src/numbers.rs",
                        "runtime/tests/test_long_conversions.rs",
                    ],
                )
            ),
        ),
        Canary(
            "triality_gate",
            "perf-claim-without-bench-fires",
            lambda: bool(
                triality_gate.window_drift(
                    ["perf(runtime): int-mul peel -- 1.65x CPython"],
                    ["runtime/src/arith.rs"],
                )
            ),
        ),
        Canary(
            "triality_gate",
            "bare-opt-out-token-not-honored",
            # a [no-triality] with NO rationale must NOT opt out (fires as drift).
            lambda: triality_gate.opt_out_rationale(["fix: x [no-triality]"]) is None,
        ),
        # --- Wave 2, A6: magnitude-dismissal + verdict-scope gate ----------
        Canary(
            "magnitude_dismissal_gate",
            "absolute-dismissal-fires",
            lambda: bool(
                magnitude_dismissal_gate.classify_window(
                    ["weak delta on the erasure lane; not worth chasing, deferring"],
                    {},
                )
            ),
        ),
        Canary(
            "magnitude_dismissal_gate",
            "relative-significance-allows",
            lambda: (
                not magnitude_dismissal_gate.classify_window(
                    [
                        "erasure delta 0.012 is 18% of the remaining gap to green, keep it "
                        "(relative significance)"
                    ],
                    {},
                )
            ),
        ),
        Canary(
            "magnitude_dismissal_gate",
            "unscoped-kill-fires",
            lambda: bool(
                magnitude_dismissal_gate.classify_window(
                    [],
                    {
                        "docs/agent/POISON_ORPHAN_LEDGER.md": [
                            "Lever-D is FALSIFIED, dropping it"
                        ]
                    },
                )
            ),
        ),
        Canary(
            "magnitude_dismissal_gate",
            "scoped-verdict-allows",
            lambda: (
                not magnitude_dismissal_gate.classify_window(
                    [],
                    {
                        "docs/agent/POISON_ORPHAN_LEDGER.md": [
                            "Lever-D FALSIFIED  verdict_scope: instance -- only the toy formulation"
                        ]
                    },
                )
            ),
        ),
    ]


def run() -> list[tuple[Canary, bool]]:
    results: list[tuple[Canary, bool]] = []
    for c in _canaries():
        try:
            ok = bool(c.fires())
        except Exception:
            ok = False
        results.append((c, ok))
    return results


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="check_gate_liveness", description=__doc__)
    ap.add_argument(
        "--check", action="store_true", help="exit 1 if any canary failed to fire"
    )
    args = ap.parse_args(argv)

    results = run()
    dead = [c for c, ok in results if not ok]
    for c, ok in results:
        print(f"  [{'LIVE' if ok else 'DEAD'}] {c.gate}:{c.name}")
    if dead:
        print(
            f"\n{len(dead)} gate canary/canaries FAILED to fire -- the apparatus has "
            f"silently rotted (M34/M42). Fix the gate before trusting it."
        )
        if args.check:
            return 1
    else:
        print(f"\nAll {len(results)} gate canaries live.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
