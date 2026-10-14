"""The one deterministic partition of weighted work items into shards.

Nightly test shards and canonical performance-gate shards both use it, so a
plan built by one tool is reproducible by any other from the same inputs.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any


def lpt_shards(
    entries: Sequence[Mapping[str, Any]], count: int
) -> list[dict[str, Any]]:
    """Partition entries using deterministic longest-processing-time first.

    Each entry has a ``path`` and an integer ``weight``. Every shard is
    nonempty; ties break by path and then by shard index.
    """

    if count <= 0:
        raise ValueError("shard count must be positive")
    if len(entries) < count:
        raise ValueError(
            f"corpus has {len(entries)} entries but requires {count} nonempty shards"
        )
    bins: list[list[Mapping[str, Any]]] = [[] for _ in range(count)]
    weights = [0] * count
    for entry in sorted(
        entries, key=lambda row: (-int(row["weight"]), str(row["path"]))
    ):
        shard_id = min(range(count), key=lambda index: (weights[index], index))
        bins[shard_id].append(entry)
        weights[shard_id] += int(entry["weight"])
    return [
        {
            "id": shard_id,
            "weight": weights[shard_id],
            "entries": sorted(str(entry["path"]) for entry in bins[shard_id]),
        }
        for shard_id in range(count)
    ]
