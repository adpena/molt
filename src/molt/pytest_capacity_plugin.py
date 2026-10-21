"""Size and order pytest-xdist workers' work (docs/design/host_capacity.md).

``-n auto`` resolves through ``molt.host_capacity``: as many workers as the
process's CPUs and memory admit for a declared per-worker unit, the measured
peak of one worker and the builds its tests start. Each proof-plan command
declares its unit with ``--molt-worker-memory-gb``, from the ``worker_peaks``
its receipts record, so the same argv runs as many workers as each machine
holds. Memory is a parent's slice when one was exported (the guard's tree
cap, the proof executor's per-command share), which already holds the
machine reserve. CPU is elastic until one jobserver shares it.

Under xdist's load scheduling, a test marked ``long_pole`` starts before
every unmarked test, and the controller sends at most two tests at a time.
A long test that starts last otherwise bounds the whole run: in the CLI
suite one worker ran 529 s of tests while the mean was 474 s. Longest
first is the classic list-scheduling remedy (Graham's LPT rule); small
dispatch steps keep a queue of long tests from piling onto one worker,
which is what xdist's ``--maxschedchunk`` exists for. Unmarked tests keep
their collection order, so module fixtures still run once per worker.
"""

from __future__ import annotations

import math
import os
from typing import Any

import pytest

from molt import host_capacity

_GIB = 1024**3

LONG_POLE_MARKER = "long_pole"
# xdist keeps two tests queued on each worker, the running one and the next.
LOAD_SCHEDULE_CHUNK = 2
_LONG_POLES_FIRST = "molt_long_poles_first"


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


def _load_scheduled(config: pytest.Config) -> bool:
    return getattr(config.option, "dist", "no") == "load"


def pytest_configure(config: pytest.Config) -> None:
    config.addinivalue_line(
        "markers",
        f"{LONG_POLE_MARKER}: a test that takes far longer than most (measured "
        "10 s or more); xdist load scheduling starts it first",
    )
    if getattr(config, "workerinput", None) is not None or not _load_scheduled(config):
        return
    # An explicit --maxschedchunk still wins.
    if getattr(config.option, "maxschedchunk", None) is None:
        config.option.maxschedchunk = LOAD_SCHEDULE_CHUNK


@pytest.hookimpl(optionalhook=True)
def pytest_configure_node(node: Any) -> None:
    # Workers cannot tell how the controller schedules, so it tells them.
    node.workerinput[_LONG_POLES_FIRST] = _load_scheduled(node.config)


@pytest.hookimpl(trylast=True)
def pytest_collection_modifyitems(
    config: pytest.Config, items: list[pytest.Item]
) -> None:
    workerinput = getattr(config, "workerinput", None)
    if workerinput is None or not workerinput.get(_LONG_POLES_FIRST, False):
        return
    # Every worker collects the same items and applies the same stable
    # partition, so xdist still sees one identical collection.
    poles = [item for item in items if item.get_closest_marker(LONG_POLE_MARKER)]
    if poles:
        rest = [item for item in items if not item.get_closest_marker(LONG_POLE_MARKER)]
        items[:] = poles + rest
