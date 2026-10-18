"""Size pytest-xdist workers from this machine (docs/design/host_capacity.md).

``-n auto`` resolves through ``molt.host_capacity``: as many workers as the
process's CPUs and memory admit for a declared per-worker unit, the measured
peak of one worker and the builds its tests start. Each proof-plan command
declares its unit with ``--molt-worker-memory-gb``, from the ``worker_peaks``
its receipts record, so the same argv runs as many workers as each machine
holds. Memory is a parent's slice when one was exported (the guard's tree
cap, the proof executor's per-command share), which already holds the
machine reserve. CPU is elastic until one jobserver shares it.
"""

from __future__ import annotations

import math
import os

import pytest

from molt import host_capacity

_GIB = 1024**3


def pytest_addoption(parser: pytest.Parser) -> None:
    parser.getgroup("molt-capacity").addoption(
        "--molt-worker-memory-gb",
        type=float,
        default=host_capacity.DEFAULT_PYTEST_WORKER_MEMORY_GB,
        help=(
            "measured peak memory of one xdist worker and the processes its tests "
            "start, which -n auto divides into "
            f"(default {host_capacity.DEFAULT_PYTEST_WORKER_MEMORY_GB})"
        ),
    )


def _worker_bytes(config: pytest.Config) -> int:
    gigabytes = config.getoption("molt_worker_memory_gb")
    if gigabytes is None or not math.isfinite(gigabytes) or gigabytes <= 0:
        raise pytest.UsageError("--molt-worker-memory-gb must be positive and finite")
    size = gigabytes * _GIB
    if not math.isfinite(size) or size < 1:
        raise pytest.UsageError(
            "--molt-worker-memory-gb must describe a finite size of at least one byte"
        )
    return int(size)


@pytest.hookimpl(tryfirst=True, optionalhook=True)
def pytest_xdist_auto_num_workers(config: pytest.Config) -> int | None:
    # xdist's own override stays the explicit way to pin a count.
    if os.environ.get("PYTEST_XDIST_AUTO_NUM_WORKERS"):
        return None
    capacity = host_capacity.memory_capacity(os.environ)
    total, available = capacity.bounded_bytes
    return host_capacity.worker_count_from_samples(
        bytes_per_worker=_worker_bytes(config),
        headroom_bytes=host_capacity.headroom_bytes(capacity),
        total_memory_bytes=total,
        available_memory_bytes=available,
    )
