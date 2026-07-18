"""Shared fail-closed verdicts for counted proof sweeps.

Named proof lanes may distinguish a disproved property from an infrastructure
error, but neither condition -- and especially not zero executed cases -- may
produce a successful process status.
"""

from __future__ import annotations


def fail_closed_proof_exit_code(
    *,
    executed: int,
    failed: int,
    errors: int,
) -> int:
    """Return 0 only when at least one case executed and every case passed.

    Exit 1 denotes an executed counterexample/failure.  Exit 2 denotes an
    invalid or incomplete proof run (errors or zero execution).
    """
    counts = {"executed": executed, "failed": failed, "errors": errors}
    if any(not isinstance(value, int) or value < 0 for value in counts.values()):
        raise ValueError(f"proof counts must be non-negative integers: {counts}")
    if failed > executed:
        raise ValueError(f"failed proofs cannot exceed executed proofs: {counts}")
    if errors or executed == 0:
        return 2
    if failed:
        return 1
    return 0
