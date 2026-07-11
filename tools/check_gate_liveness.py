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
import datetime as _dt
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.hooks import bash_guard, landing_gate, session_learning  # noqa: E402
from tools.hooks import waivers  # noqa: E402
from tools import triality, triality_gate, magnitude_dismissal_gate  # noqa: E402
from tools import claims_status, check_sister_landed, commit_serializer  # noqa: E402
from tools import advisory_classifier, disk_guard  # noqa: E402
from tools import anti_recurrence_gate, apparatus_agent_safety  # noqa: E402
from tools import encoding_gate, forbidden_checkout_guard  # noqa: E402

_GB = 1024**3

# Fixed clock + timestamp helper for the A11 claims-status canaries (the pure
# classifier takes an explicit ``now`` so the liveness check is deterministic).
_A11_NOW = _dt.datetime(2026, 7, 11, 12, 0, 0, tzinfo=_dt.timezone.utc)


def _a11_ts(hours_ago: float) -> str:
    return (_A11_NOW - _dt.timedelta(hours=hours_ago)).strftime(claims_status._TS_FMT)


@dataclass
class Canary:
    gate: str
    name: str
    fires: Callable[[], bool]  # True == the gate correctly fired on known-bad input


def _canaries() -> list[Canary]:
    return [
        Canary(
            "advisory_classifier",
            "closed-enum-rejects-explanation",
            lambda: (
                advisory_classifier.decide_output(
                    "poison because maybe", text="x", schema=("poison", "benign")
                ).verdict
                is None
            ),
        ),
        Canary(
            "advisory_classifier",
            "prompt-echo-rejected",
            lambda: (
                advisory_classifier.decide_output(
                    "source text", text="source text", schema=("benign",)
                ).reason
                == "prompt-echo"
            ),
        ),
        Canary(
            "triality",
            "missing-equations-leg-fires",
            lambda: (
                not triality.decide(
                    {
                        "finding_id": "bad",
                        "invariant": "one fact",
                        "legs": {
                            "dag": {
                                "authority": "x",
                                "fingerprint": triality.invariant_fingerprint(
                                    "one fact"
                                ),
                            },
                            "dsl": {
                                "authority": "x",
                                "fingerprint": triality.invariant_fingerprint(
                                    "one fact"
                                ),
                            },
                        },
                    }
                ).known
            ),
        ),
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
        # --- Wave 4, A11: claims terminal vocabulary + classifier ----------
        Canary(
            "claims_status",
            "falsified-classifies-retired",
            lambda: (
                claims_status.classify_status(
                    claims_status.Row("L", "a", _a11_ts(0.1), "FALSIFIED", ""), _A11_NOW
                )[0]
                == claims_status.RETIRED
            ),
        ),
        Canary(
            "claims_status",
            "stale-claimed-classifies-stale",
            lambda: (
                claims_status.classify_status(
                    claims_status.Row("L", "a", _a11_ts(9.0), "CLAIMED", ""), _A11_NOW
                )[0]
                == claims_status.STALE
            ),
        ),
        Canary(
            "claims_status",
            "fresh-claimed-classifies-live",
            # known-GOOD counter-fixture: a fresh claim must NOT read as retired/stale.
            lambda: (
                claims_status.classify_status(
                    claims_status.Row("L", "a", _a11_ts(0.5), "CLAIMED", ""), _A11_NOW
                )[0]
                == claims_status.LIVE
            ),
        ),
        # --- Wave 4, A11: premise-verification preflight -------------------
        Canary(
            "check_sister_landed",
            "landed-stands-down",
            lambda: (
                check_sister_landed.classify(["done"], []).code
                == check_sister_landed.STAND_DOWN_DUPLICATE
            ),
        ),
        Canary(
            "check_sister_landed",
            "inflight-waits",
            lambda: (
                check_sister_landed.classify([], ["mid"]).code
                == check_sister_landed.WAIT_AND_REASSESS
            ),
        ),
        Canary(
            "check_sister_landed",
            "clean-proceeds",
            # known-GOOD counter-fixture: no matches must PROCEED, not falsely block.
            lambda: (
                check_sister_landed.classify([], []).code == check_sister_landed.PROCEED
            ),
        ),
        # --- Wave 4, A11: serialized commit primitive ----------------------
        Canary(
            "commit_serializer",
            "add-dash-A-refused",
            lambda: commit_serializer.validate_pathspec(["-A"]) is not None,
        ),
        Canary(
            "commit_serializer",
            "dot-sweep-refused",
            lambda: commit_serializer.validate_pathspec(["."]) is not None,
        ),
        Canary(
            "commit_serializer",
            "named-pathspec-allowed",
            # known-GOOD counter-fixture: a clean named file must NOT be refused.
            lambda: commit_serializer.validate_pathspec(["runtime/src/x.rs"]) is None,
        ),
        # --- APPARATUS disk guard: deterministic preemptive reclaim ---------
        Canary(
            "disk_guard",
            "below-high-water-reclaims-stale",
            lambda: (
                disk_guard.plan_reclaim(
                    [
                        disk_guard.Candidate(
                            Path("/x/target/sessions/old"),
                            "sessions",
                            mtime=0.0,
                            size=5 * _GB,
                        )
                    ],
                    free_bytes=10 * _GB,
                    high_water_bytes=25 * _GB,
                    target_bytes=40 * _GB,
                    now=1_000_000.0,
                    min_idle_s=900.0,
                ).triggered
            ),
        ),
        Canary(
            "disk_guard",
            "above-high-water-stands-down",
            # known-GOOD counter-fixture: above the threshold, NOTHING is selected.
            lambda: (
                not disk_guard.plan_reclaim(
                    [
                        disk_guard.Candidate(
                            Path("/x/target/sessions/old"),
                            "sessions",
                            mtime=0.0,
                            size=5 * _GB,
                        )
                    ],
                    free_bytes=30 * _GB,
                    high_water_bytes=25 * _GB,
                    target_bytes=40 * _GB,
                    now=1_000_000.0,
                    min_idle_s=900.0,
                ).selected
            ),
        ),
        Canary(
            "disk_guard",
            "active-dir-never-reclaimed",
            lambda: (
                disk_guard.Candidate(
                    Path("/x/target/codex-live"),
                    "lane",
                    mtime=999_999.0,  # touched ~1s before now -> ACTIVE
                    size=9 * _GB,
                )
                not in disk_guard.plan_reclaim(
                    [
                        disk_guard.Candidate(
                            Path("/x/target/codex-live"),
                            "lane",
                            mtime=999_999.0,
                            size=9 * _GB,
                        )
                    ],
                    free_bytes=1 * _GB,
                    high_water_bytes=25 * _GB,
                    target_bytes=40 * _GB,
                    now=1_000_000.0,
                    min_idle_s=900.0,
                ).selected
            ),
        ),
        Canary(
            "forbidden_checkout_guard",
            "synthetic-onedrive-mutation-refused",
            forbidden_checkout_guard.self_test,
        ),
        Canary(
            "apparatus_agent_safety",
            "synthetic-process-actuation-refused",
            apparatus_agent_safety.self_test,
        ),
        Canary(
            "encoding_gate",
            "synthetic-default-codec-refused",
            lambda: bool(
                encoding_gate.scan_source("open('x.txt', 'w')\n", "synthetic.py")
            ),
        ),
        Canary(
            "anti_recurrence_gate",
            "synthetic-uncaptured-bug-class-warned",
            anti_recurrence_gate.self_test,
        ),
        Canary(
            "session_learning",
            "digest-carries-crux-and-frontier",
            session_learning.self_test,
        ),
    ]


def _registered_canaries() -> set[str]:
    try:
        payload = tomllib.loads(
            (ROOT / "tools" / "recurring_harms.toml").read_text(encoding="utf-8")
        )
        return {str(item["canary"]) for item in payload.get("harm", [])}
    except Exception:
        return set()


def _live_canary_keys(canaries: list[Canary]) -> set[str]:
    return {f"{canary.gate}.{canary.name}" for canary in canaries}


def run() -> list[tuple[Canary, bool]]:
    results: list[tuple[Canary, bool]] = []
    canaries = _canaries()
    missing = _registered_canaries() - _live_canary_keys(canaries)
    for name in sorted(missing):
        canaries.append(Canary("recurring_harms", f"missing:{name}", lambda: False))
    for c in canaries:
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
