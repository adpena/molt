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

import os

import pytest

from molt import host_capacity

# A run that declares no unit (an ad hoc `pytest -n auto`, or a landing's
# merged run of several suites). Measured units span 0.22 GiB
# (repository.agent-coordination) to 4.94 GiB (the cold/warm build test of
# python.unit.cli.import-collection), macOS arm64, 2026-10-17; this default
# covers every suite but the two that declare more.
DEFAULT_WORKER_MEMORY_GB = 2.5
_GIB = 1024**3


def pytest_addoption(parser: pytest.Parser) -> None:
    parser.getgroup("molt-capacity").addoption(
        "--molt-worker-memory-gb",
        type=float,
        default=DEFAULT_WORKER_MEMORY_GB,
        help=(
            "measured peak memory of one xdist worker and the processes its tests "
            f"start, which -n auto divides into (default {DEFAULT_WORKER_MEMORY_GB})"
        ),
    )


def _worker_bytes(config: pytest.Config) -> int:
    gigabytes = config.getoption("molt_worker_memory_gb")
    if gigabytes is None or gigabytes <= 0:
        raise pytest.UsageError("--molt-worker-memory-gb must be positive")
    return int(gigabytes * _GIB)


@pytest.hookimpl(tryfirst=True, optionalhook=True)
def pytest_xdist_auto_num_workers(config: pytest.Config) -> int | None:
    # xdist's own override stays the explicit way to pin a count.
    if os.environ.get("PYTEST_XDIST_AUTO_NUM_WORKERS"):
        return None
    return host_capacity.worker_count(
        bytes_per_worker=_worker_bytes(config),
        headroom_bytes=host_capacity.headroom_bytes(),
    )
