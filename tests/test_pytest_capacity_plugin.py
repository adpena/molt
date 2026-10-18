"""-n auto sizes xdist workers from the host capacity authority."""

from __future__ import annotations

import os
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt import host_capacity, pytest_capacity_plugin
from tests.process_guard_common import run_guarded_test_process

ROOT = Path(__file__).resolve().parents[1]


def _workers(tmp_path: Path, *options: str, env: dict[str, str] | None = None) -> int:
    project = tmp_path / "project"
    project.mkdir(exist_ok=True)
    (project / "pyproject.toml").write_text(
        '[tool.pytest.ini_options]\naddopts = ""\n', encoding="utf-8"
    )
    record = tmp_path / "workers.txt"
    (project / "test_count.py").write_text(
        "import os, pathlib\n"
        "def test_count():\n"
        f"    pathlib.Path({str(record)!r}).write_text(\n"
        "        os.environ['PYTEST_XDIST_WORKER_COUNT'])\n",
        encoding="utf-8",
    )
    environment = dict(os.environ if env is None else env)
    environment["PYTHONPATH"] = os.pathsep.join([str(ROOT / "src"), str(ROOT)])
    environment.pop("PYTEST_XDIST_AUTO_NUM_WORKERS", None)
    if env is not None and "PYTEST_XDIST_AUTO_NUM_WORKERS" in env:
        environment["PYTEST_XDIST_AUTO_NUM_WORKERS"] = env[
            "PYTEST_XDIST_AUTO_NUM_WORKERS"
        ]
    result = run_guarded_test_process(
        [
            sys.executable,
            "-m",
            "pytest",
            "-q",
            "-p",
            "molt.pytest_capacity_plugin",
            "-p",
            "no:cacheprovider",
            "-n",
            "auto",
            *options,
            str(project / "test_count.py"),
        ],
        cwd=project,
        env=environment,
        timeout=180.0,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    return int(record.read_text(encoding="utf-8"))


def test_auto_workers_fit_the_declared_unit_in_memory(tmp_path: Path) -> None:
    # One worker's unit exceeds any machine: memory admits one.
    assert _workers(tmp_path, "--molt-worker-memory-gb=1000000") == 1


def test_auto_workers_are_bounded_by_the_cpus_the_process_may_use(
    tmp_path: Path,
) -> None:
    # A unit too small to matter leaves the CPU count as the bound.
    assert _workers(tmp_path, "--molt-worker-memory-gb=0.0001") == (
        host_capacity.cpu_count()
    )


def test_auto_workers_fill_the_slice_their_parent_exported(tmp_path: Path) -> None:
    # A proof executor's per-command share already holds the reserve, so three
    # 1 GiB workers fit a 3 GiB share; subtracting the reserve again gave two.
    if host_capacity.cpu_count() < 3:
        pytest.skip("needs three CPUs for the memory term to bind")
    if (host_capacity.memory_capacity().bounded_bytes[1] or 0) < 3 * 1024**3:
        pytest.skip("needs 3 GiB of live memory")
    env = dict(os.environ, **{host_capacity.MEMORY_SLICE_ENV: str(3 * 1024**3)})
    assert _workers(tmp_path, "--molt-worker-memory-gb=1", env=env) == 3


def test_xdists_explicit_count_still_wins(tmp_path: Path) -> None:
    env = dict(os.environ, PYTEST_XDIST_AUTO_NUM_WORKERS="3")
    assert _workers(tmp_path, "--molt-worker-memory-gb=1000000", env=env) == 3


@pytest.mark.parametrize(("available", "expected"), [(40, 3), (2, 2), (None, 3)])
def test_auto_worker_sizing_uses_one_snapshot_and_its_delegated_headroom(
    monkeypatch, available, expected
):
    gib = 1024**3
    probes = []

    def observe():
        probes.append(None)
        assert len(probes) == 1
        return 64 * gib, None if available is None else available * gib

    monkeypatch.delenv("PYTEST_XDIST_AUTO_NUM_WORKERS", raising=False)
    monkeypatch.setenv(host_capacity.MEMORY_SLICE_ENV, str(3 * gib))
    monkeypatch.setattr(host_capacity, "_probe_memory_bytes", observe)
    monkeypatch.setattr(host_capacity, "cpu_count", lambda: 8)
    config = SimpleNamespace(getoption=lambda name: 1.0)
    assert pytest_capacity_plugin.pytest_xdist_auto_num_workers(config) == expected
    assert len(probes) == 1


@pytest.mark.parametrize("unit", [0, -1, float("nan"), float("inf"), 1e-100, 1e308])
def test_worker_memory_unit_requires_a_positive_finite_byte_count(unit):
    config = SimpleNamespace(getoption=lambda name: unit)
    with pytest.raises(pytest.UsageError, match="--molt-worker-memory-gb"):
        pytest_capacity_plugin._worker_bytes(config)


@pytest.mark.parametrize(
    ("total", "expected"),
    [
        (None, host_capacity.RESERVE_MIN_BYTES),
        (8 * 1024**3, host_capacity.RESERVE_MIN_BYTES),
        (64 * 1024**3, int(64 * 1024**3 * host_capacity.RESERVE_FRACTION)),
        (1024 * 1024**3, host_capacity.RESERVE_MAX_BYTES),
    ],
)
def test_reserve_is_a_clamped_fraction_of_physical_memory(total, expected) -> None:
    assert host_capacity.reserve_bytes(total) == expected
