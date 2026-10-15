"""The outcome of one Nightly item, as every per-test result reader returns it.

Differential (`tools/compat/diff_results.py`) and regrtest
(`tools/compat/regrtest_results.py`) children append one row per test as each
finishes, so a killed shard keeps what it finished. Each reader turns its own
row schema into these outcomes; the shard runner applies one policy to both.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal, get_args

ItemStatus = Literal["passed", "failed", "errors", "skipped", "not_run"]
ITEM_STATUSES: tuple[ItemStatus, ...] = get_args(ItemStatus)


@dataclass(frozen=True, slots=True)
class ItemOutcome:
    """One test: its shard status, why it did not pass, and how long it took."""

    status: ItemStatus
    reason: str  # "" for a pass; otherwise the failure, skip or admission class
    detail: str
    duration_s: float | None  # None exactly when the test did not run
