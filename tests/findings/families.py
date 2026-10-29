"""Finding families: which open findings each burn-down family owns.

The ledger rows carry no family field yet, so this declaration classifies the
open rows of each family whose coverage is enforced. Members are
`OpenFindingId`s: closing a row removes it from the generated type, and `ty`
and `tools/finding_coverage.py` then reject the stale member until the change
that closes the row removes it here too.

A family is enforced when each member has at least one finding test, finding
program or regrtest expectation, and the coverage report counts the remaining
expected failures per family (the burn-down measure).
"""

from __future__ import annotations

from collections.abc import Mapping
from typing import Literal

from tests.findings.finding_ids import OpenFindingId

FindingFamily = Literal["import_system"]

FAMILY_FINDINGS: Mapping[FindingFamily, tuple[OpenFindingId, ...]] = {
    # importlib, module publication, finders and loaders, zip imports and
    # importlib.resources. Not members yet: HF-539 (namespace packages) while
    # its own lane repairs it; HF-124, an inventory of unused intrinsics with
    # no behavior to state; HF-126 and HF-140, whose named programs
    # (unittest_lazy_import.py, the encodings mbcs/oem probes) pass natively,
    # so no failing native behavior is known to state for them (HF-140 names a
    # WASM cell that the pilot did not run).
    "import_system": (
        "HF-247",
        "HF-276",
        "HF-297",
        "HF-383",
        "HF-530",
    ),
}
