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


def test_auto_workers_are_bounded_by_the_cpus_the_process_may_use(monkeypatch) -> None:
    # Inspect the actual selection without launching one worker per host CPU.
    # The bounded-memory and explicit-count cases below exercise child wiring.
    monkeypatch.delenv("PYTEST_XDIST_AUTO_NUM_WORKERS", raising=False)
    config = SimpleNamespace(getoption=lambda name: 0.0001)
    assert (
        pytest_capacity_plugin.pytest_xdist_auto_num_workers(config)
        == host_capacity.cpu_count()
    )


def test_auto_workers_fill_the_slice_their_parent_exported(tmp_path: Path) -> None:
    # A proof executor's per-command share already holds the reserve, so three
    # 1 GiB workers fit a 3 GiB share; subtracting the reserve again gave two.
    if host_capacity.cpu_count() < 3:
        pytest.skip("needs three CPUs for the memory term to bind")
    if (host_capacity.memory_capacity().bounded_bytes[1] or 0) < 3 * 1024**3:
        pytest.skip("needs 3 GiB of live memory")
    env = dict(os.environ, **{host_capacity.MEMORY_SLICE_ENV: str(3 * 1024**3)})
    # This case owns the default sizing policy; the next case owns overrides.
    env.pop("PYTEST_XDIST_AUTO_NUM_WORKERS", None)
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


def _run_order(tmp_path: Path, *options: str) -> dict[str, list[str]]:
    """Run 40 short tests, then 4 marked long poles; return each worker's order."""
    project = tmp_path / "order-project"
    project.mkdir()
    (project / "pyproject.toml").write_text(
        '[tool.pytest.ini_options]\naddopts = ""\n', encoding="utf-8"
    )
    record = tmp_path / "order-record"
    record.mkdir()
    (project / "test_order.py").write_text(
        "import os, pathlib, pytest\n"
        "def _record(name):\n"
        "    worker = os.environ.get('PYTEST_XDIST_WORKER', 'serial')\n"
        f"    path = pathlib.Path({str(record)!r}) / worker\n"
        "    with path.open('a', encoding='utf-8') as handle:\n"
        "        handle.write(name + '\\n')\n"
        "@pytest.mark.parametrize('index', range(40))\n"
        "def test_short(index):\n"
        "    _record(f'short-{index}')\n"
        "@pytest.mark.long_pole\n"
        "@pytest.mark.parametrize('index', range(4))\n"
        "def test_long(index):\n"
        "    _record(f'long-{index}')\n",
        encoding="utf-8",
    )
    environment = dict(os.environ)
    environment["PYTHONPATH"] = os.pathsep.join([str(ROOT / "src"), str(ROOT)])
    # The child is its own session, not this run's xdist worker.
    for key in (
        "PYTEST_ADDOPTS",
        "PYTEST_XDIST_WORKER",
        "PYTEST_XDIST_WORKER_COUNT",
        "PYTEST_XDIST_TESTRUNUID",
    ):
        environment.pop(key, None)
    result = run_guarded_test_process(
        [
            sys.executable,
            "-m",
            "pytest",
            "-q",
            # The plugin registers the marker; strict markers prove it.
            "--strict-markers",
            "-p",
            "molt.pytest_capacity_plugin",
            "-p",
            "no:cacheprovider",
            *options,
            str(project / "test_order.py"),
        ],
        cwd=project,
        env=environment,
        timeout=180.0,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    return {
        path.name: path.read_text(encoding="utf-8").split()
        for path in sorted(record.iterdir())
    }


def test_long_poles_start_first_on_every_worker_two_tests_at_a_time(
    tmp_path: Path,
) -> None:
    runs = _run_order(tmp_path, "-n", "2")
    assert sorted(runs) == ["gw0", "gw1"]
    assert sorted(name for names in runs.values() for name in names) == sorted(
        [f"short-{index}" for index in range(40)]
        + [f"long-{index}" for index in range(4)]
    )
    for names in runs.values():
        # Each worker runs its share of the long poles before any short test.
        kinds = [name.split("-")[0] for name in names]
        assert kinds == sorted(kinds), names
    # xdist's default first step would hand all four poles to one worker.
    assert all(
        any(name.startswith("long-") for name in names) for names in runs.values()
    )


def test_an_explicit_schedule_chunk_still_wins(tmp_path: Path) -> None:
    runs = _run_order(tmp_path, "-n", "2", "--maxschedchunk=44")
    holders = [
        worker
        for worker, names in runs.items()
        if any(name.startswith("long-") for name in names)
    ]
    assert len(holders) == 1, runs


def test_a_serial_run_keeps_the_collection_order(tmp_path: Path) -> None:
    runs = _run_order(tmp_path)
    assert runs == {
        "serial": [f"short-{index}" for index in range(40)]
        + [f"long-{index}" for index in range(4)]
    }


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


@pytest.mark.parametrize(("slice_gib", "expected"), [(40, 2), (24, 1), (12, 1)])
def test_compiled_contract_worker_estimate_limits_actual_capacity_planner(
    monkeypatch, slice_gib: int, expected: int
) -> None:
    from tools.proof_plan import ProofPlan

    row = next(
        command
        for command in ProofPlan.load().commands
        if command.id == "python.unit.compiled-contracts.macos"
    )
    monkeypatch.delenv("PYTEST_XDIST_AUTO_NUM_WORKERS", raising=False)
    monkeypatch.setenv(host_capacity.MEMORY_SLICE_ENV, str(slice_gib * 1024**3))
    monkeypatch.setattr(
        host_capacity, "_probe_memory_bytes", lambda: (64 * 1024**3, 64 * 1024**3)
    )
    monkeypatch.setattr(host_capacity, "cpu_count", lambda: 32)
    config = SimpleNamespace(getoption=lambda name: row.data["pytest_worker_memory_gb"])
    assert pytest_capacity_plugin.pytest_xdist_auto_num_workers(config) == expected
