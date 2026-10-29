"""Finding tests: the executable form of the V1 findings ledger.

A finding test states the behavior an open ledger row violates, with an
independent oracle, and fails today because of that row's defect. It carries
the row's identity as a closed type (`finding_ids.OpenFindingId`, generated
from `docs/agent/V1_HANDOFF_FINDINGS.md`), so `ty` rejects a test that cites a
finding the ledger does not have or has closed.

`finding()` marks a pytest test; `tests/conftest.py` turns the mark into a
strict expected failure. The test must fail with an `AssertionError` about the
finding's contract: any other exception is an unexpected failure, and a pass is
an unexpected pass (strict XPASS) that fails the run until the marker is
removed and the row closes in the same change. Differential programs and
CPython regrtest cases name the same identity in their own expectation records
(`finding=` in `MOLT_META`, `finding` in
`tools/suite_honesty/differential_expectations.json`); `tools/finding_coverage.py`
projects all three into one per-finding and per-family count.

This module imports no test framework at module level: the differential and
regrtest tools validate finding identities through it.
"""

from __future__ import annotations

from collections.abc import Sequence
from pathlib import Path
from typing import TYPE_CHECKING

from tests.findings.finding_ids import (
    FIXED_FINDING_IDS,
    OPEN_FINDING_IDS,
    OpenFindingId,
)

if TYPE_CHECKING:
    import pytest

FINDINGS_ROOT = Path(__file__).resolve().parent
MARK_NAME = "finding"
# A finding test reports its finding's defect as an assertion about the
# contract; an import error, a crashed fixture or a failed build is not it.
DEFECT_EXCEPTION = AssertionError


def finding(finding_id: OpenFindingId) -> pytest.MarkDecorator:
    """Mark a test as the executable statement of one open finding."""
    import pytest

    return pytest.mark.finding(finding_id)


def finding_problem(finding_id: object) -> str | None:
    """Why *finding_id* cannot own an expectation, or None when it is open."""
    if finding_id in OPEN_FINDING_IDS:
        return None
    if finding_id in FIXED_FINDING_IDS:
        return (
            f"{finding_id} is fixed in the findings ledger; remove its "
            "expectation in the change that closes the row"
        )
    return f"{finding_id!r} is not a finding of the findings ledger"


def finding_test_problem(
    path: Path, marks: Sequence[pytest.Mark], *, explicit_xfail: bool
) -> str | None:
    """Why a test with these finding marks is not a valid finding test.

    A finding test lives under tests/findings/, so the coverage projection
    sees every one; names exactly one open finding; and takes its expected
    failure only from that finding.
    """
    if len(marks) != 1 or len(marks[0].args) != 1 or marks[0].kwargs:
        return "name exactly one finding, as @finding(id)"
    finding_id = marks[0].args[0]
    if problem := finding_problem(finding_id):
        return problem
    if not path.resolve().is_relative_to(FINDINGS_ROOT):
        return "a finding test lives under tests/findings/"
    if explicit_xfail:
        return (
            f"a finding test takes its expected failure from {finding_id}; "
            "remove the explicit xfail"
        )
    return None


def strict_expectation(finding_id: str) -> pytest.MarkDecorator:
    """The strict expected failure that a finding mark stands for."""
    import pytest

    return pytest.mark.xfail(
        strict=True,
        raises=DEFECT_EXCEPTION,
        reason=f"{finding_id} (open finding)",
    )
