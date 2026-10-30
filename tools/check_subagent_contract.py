#!/usr/bin/env python3
"""Anti-rot integrity gate for the subagent-dispatch contract (APPARATUS A7).

The contract only works if every named block still EXISTS and still carries its
load-bearing instruction. This gate asserts exactly that, and it does so with a
HARDCODED list of required names + key phrases that is INDEPENDENT of the
contract module's own registry -- so deleting, renaming, or blanking a constant
(or emptying ``CONTRACT_CONSTANT_NAMES`` to hide the deletion) fails the gate
rather than self-waiving it. A removed constant cannot certify its own absence.

Runs as an ``always`` landing gate and the ``repository.subagent-contract``
proof-plan command;
its teeth are proven by ``tests/tools/test_subagent_contract.py``.

Run:
    python tools/check_subagent_contract.py           # PASS -> exit 0, FAIL -> exit 2
    python tools/check_subagent_contract.py --json     # machine-readable result
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)


# --- The gate's OWN authority (independent of the module under audit) ----------------------
#
# These are hardcoded here on purpose. If subagent_contract.py drops or renames a constant,
# this list still demands it -> the gate fails. Keep in sync deliberately when the contract
# genuinely gains/loses a block (that is a reviewed change, not silent drift).

REQUIRED_CONSTANTS: tuple[str, ...] = (
    "GROUNDED_PROGRESS",
    "NO_ENDING_ON_PROMISES",
    "VERIFY_CHANGED_CONTRACT",
    "EVIDENCE_REVIEW",
    "WEB_AUTHORITY",
    "LANDING_CONTRACT",
    "NO_FAKES",
    "GIT_CUSTODY",
    "BUILD_ENV",
    "DISTILL",
    "MODEL_TIER",
    "TRIALITY_WIRING",
    "PREMISE_VERIFICATION",
    "HANDOFF_PRECHECK",
)

#: The verbatim-grade fragment each required constant must still contain.
REQUIRED_KEY_PHRASES: dict[str, str] = {
    "GROUNDED_PROGRESS": "audit each claim against a tool result",
    "NO_ENDING_ON_PROMISES": "check your last paragraph",
    "VERIFY_CHANGED_CONTRACT": "Reuse matching evidence",
    "EVIDENCE_REVIEW": "against the written specification",
    "WEB_AUTHORITY": "primary sources",
    "LANDING_CONTRACT": "integration owner",
    "NO_FAKES": "zero stubs, zero theater",
    "GIT_CUSTODY": "NEVER `git add -A`",
    "BUILD_ENV": "never change source, seal, worktree, or toolchain custody",
    "DISTILL": "advances the assigned outcome",
    "MODEL_TIER": "Respect operator concurrency limits",
    "TRIALITY_WIRING": "all three legs",
    "PREMISE_VERIFICATION": "before your first Write",
    "HANDOFF_PRECHECK": "--precheck-only",
}


@dataclass
class AuditResult:
    ok: bool
    failures: list[str] = field(default_factory=list)

    def as_dict(self) -> dict[str, Any]:
        return {"ok": self.ok, "failures": self.failures}


def _import_contract():
    """Import the real contract module; the bootstrap makes `tools` resolvable."""
    from tools import subagent_contract

    return subagent_contract


def audit(module: Any | None = None) -> AuditResult:
    """Check the contract module against this gate's hardcoded authority.

    Args:
        module: the module (or any namespace) to audit. ``None`` imports the real
            ``tools.subagent_contract``. Passing a synthetic namespace with a constant
            deleted/blanked is how the teeth are proven (see the tests).

    Returns:
        An :class:`AuditResult`; ``ok`` is True only if every required constant exists,
        is a non-empty string, keeps its key phrase, and documents its M## in ``MECHANIZES``.
    """
    sc = module if module is not None else _import_contract()
    failures: list[str] = []

    for name in REQUIRED_CONSTANTS:
        value = getattr(sc, name, None)
        if value is None:
            failures.append(f"{name}: MISSING (constant removed or renamed)")
            continue
        if not isinstance(value, str) or not value.strip():
            failures.append(f"{name}: not a non-empty string (blanked?)")
            continue
        phrase = REQUIRED_KEY_PHRASES[name]
        if phrase not in value:
            failures.append(f"{name}: lost its key phrase {phrase!r}")

    # The module's own registry must still advertise every required constant -- emptying it
    # to hide a deletion is itself a failure.
    registry = getattr(sc, "CONTRACT_CONSTANT_NAMES", ())
    for name in REQUIRED_CONSTANTS:
        if name not in registry:
            failures.append(f"{name}: absent from CONTRACT_CONSTANT_NAMES registry")

    # Every required constant must document the M## directive it mechanizes.
    mechanizes = getattr(sc, "MECHANIZES", {})
    for name in REQUIRED_CONSTANTS:
        anchor = mechanizes.get(name) if isinstance(mechanizes, dict) else None
        if not anchor or not str(anchor).strip():
            failures.append(f"{name}: no MECHANIZES M## anchor")

    return AuditResult(ok=not failures, failures=failures)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--json", action="store_true", help="emit the result as JSON")
    args = ap.parse_args(argv)

    result = audit()
    if args.json:
        print(json.dumps(result.as_dict(), indent=2))
    elif result.ok:
        print(
            f"PASS: subagent contract integrity ({len(REQUIRED_CONSTANTS)} constants present)"
        )
    else:
        print("FAIL: subagent contract integrity")
        for f in result.failures:
            print(f"  - {f}")
    return 0 if result.ok else 2


if __name__ == "__main__":
    sys.exit(main())
