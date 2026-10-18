"""The host capacity authority counts the CPUs and memory a process may use."""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

import pytest

from molt import host_capacity
from tests.process_guard_common import (
    check_output_guarded_test_process,
    install_module_view,
)


def _unified(tmp_path: Path, quotas: dict[str, str]) -> tuple[Path, Path]:
    root = tmp_path / "cgroup"
    for relative, quota in quotas.items():
        directory = root / relative
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "cpu.max").write_text(quota + "\n", encoding="utf-8")
    membership = tmp_path / "membership"
    membership.write_text("0::/build/compiler\n", encoding="utf-8")
    return root, membership


@pytest.mark.parametrize(
    "quotas, expected",
    [
        ({"build/compiler": "250000 100000"}, 2.5),
        ({"build/compiler": "max 100000"}, None),
        # A quota on an ancestor bounds every cgroup beneath it.
        ({"build/compiler": "max 100000", "build": "100000 100000"}, 1.0),
        ({"build/compiler": "400000 100000", "build": "300000 100000"}, 3.0),
        ({"build/compiler": "100000 0"}, None),
    ],
    ids=["own", "unlimited", "ancestor", "tightest", "zero-period"],
)
def test_unified_cgroup_quota(tmp_path: Path, quotas, expected) -> None:
    root, membership = _unified(tmp_path, quotas)
    assert (
        host_capacity.cgroup_cpu_quota(cgroup_root=root, membership_path=membership)
        == expected
    )


@pytest.mark.parametrize(
    "quota, expected", [("150000", 1.5), ("-1", None)], ids=["quota", "unlimited"]
)
def test_legacy_cgroup_quota(tmp_path: Path, quota: str, expected) -> None:
    root = tmp_path / "cgroup"
    directory = root / "cpu,cpuacct" / "build"
    directory.mkdir(parents=True)
    (directory / "cpu.cfs_quota_us").write_text(quota, encoding="utf-8")
    (directory / "cpu.cfs_period_us").write_text("100000", encoding="utf-8")
    membership = tmp_path / "membership"
    membership.write_text("4:cpu,cpuacct:/build\n2:memory:/build\n", encoding="utf-8")
    assert (
        host_capacity.cgroup_cpu_quota(cgroup_root=root, membership_path=membership)
        == expected
    )


def test_no_cgroup_membership_is_no_quota(tmp_path: Path) -> None:
    assert (
        host_capacity.cgroup_cpu_quota(
            cgroup_root=tmp_path, membership_path=tmp_path / "absent"
        )
        is None
    )


def _linux_views(monkeypatch, affinity: set[int]):
    """A Python 3.12 Linux process, seen only by the module under test."""
    install_module_view(monkeypatch, "sys", sys, host_capacity, platform="linux")
    os_view = install_module_view(
        monkeypatch, "os", os, host_capacity, sched_getaffinity=lambda pid: affinity
    )
    monkeypatch.delattr(os_view, "process_cpu_count", raising=False)
    return os_view


def test_cpu_count_takes_the_quota_below_the_cpus(tmp_path: Path, monkeypatch) -> None:
    root, membership = _unified(tmp_path, {"build/compiler": "250000 100000"})
    os_view = _linux_views(monkeypatch, set(range(16)))
    assert host_capacity.cpu_count(cgroup_root=root, membership_path=membership) == 2
    # Affinity bounds the count when the quota is larger.
    monkeypatch.setattr(os_view, "sched_getaffinity", lambda pid: {0})
    assert host_capacity.cpu_count(cgroup_root=root, membership_path=membership) == 1


def test_cpu_count_is_at_least_one_under_a_fractional_quota(
    tmp_path: Path, monkeypatch
) -> None:
    root, membership = _unified(tmp_path, {"build/compiler": "50000 100000"})
    _linux_views(monkeypatch, set(range(16)))
    assert host_capacity.cpu_count(cgroup_root=root, membership_path=membership) == 1


def test_shared_worker_policy_caps_cpu_by_total_and_live_memory(monkeypatch) -> None:
    gib = 1024**3
    monkeypatch.delenv(host_capacity.MEMORY_SLICE_ENV, raising=False)
    monkeypatch.setattr(
        host_capacity, "_probe_memory_bytes", lambda: (64 * gib, 5 * gib)
    )

    assert (
        host_capacity.worker_count(
            bytes_per_worker=gib,
            headroom_bytes=2 * gib,
            cpus=32,
        )
        == 3
    )


def test_linux_worker_memory_respects_cgroup_capacity(tmp_path: Path) -> None:
    gib = 1024**3
    meminfo = tmp_path / "meminfo"
    cgroup = tmp_path / "cgroup"
    membership = tmp_path / "cgroup.membership"
    active_cgroup = cgroup / "tenant" / "compiler"
    active_cgroup.mkdir(parents=True)
    membership.write_text("0::/tenant/compiler\n", encoding="utf-8")
    meminfo.write_text(
        "MemTotal:       67108864 kB\nMemAvailable:   50331648 kB\n",
        encoding="utf-8",
    )
    (active_cgroup / "memory.max").write_text(str(8 * gib), encoding="utf-8")
    (active_cgroup / "memory.current").write_text(str(3 * gib), encoding="utf-8")
    (active_cgroup / "memory.stat").write_text(
        f"anon {2 * gib}\ninactive_file {gib}\n",
        encoding="utf-8",
    )

    assert host_capacity.linux_memory_bytes(
        meminfo_path=meminfo,
        cgroup_root=cgroup,
        cgroup_membership_path=membership,
    ) == (8 * gib, 6 * gib)


def test_linux_worker_memory_supports_legacy_cgroup_membership(tmp_path: Path) -> None:
    gib = 1024**3
    meminfo = tmp_path / "meminfo"
    membership = tmp_path / "cgroup.membership"
    cgroup = tmp_path / "cgroup"
    active_cgroup = cgroup / "memory" / "legacy" / "compiler"
    active_cgroup.mkdir(parents=True)
    membership.write_text("5:cpu,memory:/legacy/compiler\n", encoding="utf-8")
    meminfo.write_text(
        "MemTotal:       67108864 kB\nMemAvailable:   50331648 kB\n",
        encoding="utf-8",
    )
    (active_cgroup / "memory.limit_in_bytes").write_text(
        str(12 * gib), encoding="utf-8"
    )
    (active_cgroup / "memory.usage_in_bytes").write_text(str(4 * gib), encoding="utf-8")
    (active_cgroup / "memory.stat").write_text(
        f"total_inactive_file {2 * gib}\n",
        encoding="utf-8",
    )

    assert host_capacity.linux_memory_bytes(
        meminfo_path=meminfo,
        cgroup_root=cgroup,
        cgroup_membership_path=membership,
    ) == (12 * gib, 10 * gib)


def test_linux_worker_memory_treats_zero_cgroup_usage_as_exact(tmp_path: Path) -> None:
    gib = 1024**3
    meminfo = tmp_path / "meminfo"
    membership = tmp_path / "cgroup.membership"
    active_cgroup = tmp_path / "cgroup" / "compiler"
    active_cgroup.mkdir(parents=True)
    membership.write_text("0::/compiler\n", encoding="utf-8")
    meminfo.write_text(
        "MemTotal:       67108864 kB\nMemAvailable:   50331648 kB\n",
        encoding="utf-8",
    )
    (active_cgroup / "memory.max").write_text(str(8 * gib), encoding="utf-8")
    (active_cgroup / "memory.current").write_text("0", encoding="utf-8")
    (active_cgroup / "memory.stat").write_text("inactive_file 0\n", encoding="utf-8")

    assert host_capacity.linux_memory_bytes(
        meminfo_path=meminfo,
        cgroup_root=tmp_path / "cgroup",
        cgroup_membership_path=membership,
    ) == (8 * gib, 8 * gib)


def test_darwin_available_bytes_counts_free_inactive_speculative_purgeable() -> None:
    pages = {
        "Pages free": 10,
        "Pages active": 99,
        "Pages inactive": 20,
        "Pages speculative": 3,
        "Pages purgeable": 2,
        "Pages wired down": 88,
        "Pages occupied by compressor": 7,
    }

    assert host_capacity.darwin_available_bytes(16_384, pages) == (
        (10 + 20 + 3 + 2) * 16_384
    )
    assert host_capacity.darwin_available_bytes(16_384, {"Pages active": 9}) is None


@pytest.mark.skipif(sys.platform != "darwin", reason="Darwin Mach VM statistics")
def test_actual_darwin_vm_pages_match_vm_stat() -> None:
    """vm_stat formats the same Mach counters through its own code path."""

    def vm_stat() -> tuple[str, dict[str, int]]:
        text = str(
            check_output_guarded_test_process(
                ["vm_stat"], env={**os.environ, "LC_ALL": "C"}, timeout=30.0
            )
        )
        reported: dict[str, int] = {}
        for line in text.splitlines()[1:]:
            name, _, value = line.partition(":")
            if value.strip().rstrip(".").isdigit():
                reported[name.strip().strip('"')] = int(value.strip().rstrip("."))
        return text.splitlines()[0], reported

    repo_root = Path(__file__).resolve().parents[1]

    def fresh_read() -> tuple[int, dict[str, int]]:
        # macOS rate-limits host_statistics64 for non-platform binaries and
        # answers a process's excess calls in one window with cached counters:
        # in CI this process read "Pages free" as 41,711 in three rounds while
        # vm_stat moved from 55,942 to 54,811. vm_stat is a platform binary and
        # exempt, and a new process's first call is never rate-limited.
        text = check_output_guarded_test_process(
            [
                sys.executable,
                "-c",
                "import json, molt.host_capacity as m; "
                "print(json.dumps(m.darwin_vm_pages()))",
            ],
            cwd=repo_root,
            env={**os.environ, "PYTHONPATH": str(repo_root / "src")},
            timeout=60.0,
        )
        page_size, pages = json.loads(str(text))
        return int(page_size), {str(k): int(v) for k, v in pages.items()}

    # Counters move while a busy host runs other tests, so bracket each read
    # between two vm_stat samples. Pages move between buckets in bursts, so
    # the slack is 1% of all pages; a wrong counter mapping is a fixed offset
    # that escapes every bracket.
    header, sample = vm_stat()
    rounds = []
    for _ in range(3):
        page_size, pages = fresh_read()
        _header, after = vm_stat()
        rounds.append((sample, pages, after))
        sample = after
    assert f"page size of {page_size} bytes" in header
    buckets = (
        "Pages free",
        "Pages active",
        "Pages inactive",
        "Pages speculative",
        "Pages throttled",
        "Pages wired down",
        "Pages occupied by compressor",
    )
    slack = max(1024, sum(rounds[0][0][name] for name in buckets) // 100)

    def bracketed(name: str, before: dict, count: int, after: dict) -> bool:
        low, high = sorted((before[name], after[name]))
        return low - slack <= count <= high + slack

    for name in rounds[0][1]:
        assert any(
            bracketed(name, before, pages[name], after)
            for before, pages, after in rounds
        ), (name, slack, [(b[name], p[name], a[name]) for b, p, a in rounds])


@pytest.mark.skipif(sys.platform != "darwin", reason="Darwin Mach VM statistics")
def test_darwin_memory_bytes_reports_live_available_memory(monkeypatch) -> None:
    """Available memory is vm_stat's free, inactive, speculative and purgeable
    pages, never the total alone (dx once read a sysctl macOS lacks)."""
    # The machine's memory, not the slice the enclosing guard exports.
    monkeypatch.delenv(host_capacity.MEMORY_SLICE_ENV, raising=False)
    text = str(
        check_output_guarded_test_process(
            ["vm_stat"], env={**os.environ, "LC_ALL": "C"}, timeout=30.0
        )
    )
    lines = text.splitlines()
    page_size = int(lines[0].split("page size of ")[1].split()[0])
    rows = {}
    for line in lines[1:]:
        name, _, value = line.partition(":")
        if value.strip().rstrip(".").isdigit():
            rows[name.strip().strip('"')] = int(value.strip().rstrip("."))
    expected = page_size * sum(
        rows[name] for name in host_capacity.DARWIN_AVAILABLE_PAGE_ROWS
    )
    total, available = host_capacity.memory_capacity().bounded_bytes
    assert total is not None and available is not None
    # Pages move between samples on a busy host; a wrong source is far off.
    assert abs(available - expected) <= total // 50, (available, expected, total)


def test_a_parent_memory_slice_keeps_raw_and_bounded_readings(monkeypatch) -> None:
    gib = 1024**3
    monkeypatch.setattr(
        host_capacity, "_probe_memory_bytes", lambda: (64 * gib, 40 * gib)
    )
    monkeypatch.setenv(host_capacity.MEMORY_SLICE_ENV, str(8 * gib))
    capacity = host_capacity.memory_capacity()
    assert capacity.total_bytes == 64 * gib
    assert capacity.available_bytes == 40 * gib
    assert capacity.tree_limit_bytes == 8 * gib
    assert capacity.bounded_bytes == (8 * gib, 8 * gib)
    monkeypatch.setenv(host_capacity.MEMORY_SLICE_ENV, str(128 * gib))
    assert host_capacity.memory_capacity().bounded_bytes == (64 * gib, 40 * gib)
    monkeypatch.setenv(host_capacity.MEMORY_SLICE_ENV, "eight")
    with pytest.raises(ValueError, match="must be a positive integer"):
        host_capacity.memory_capacity().bounded_bytes


def test_a_slice_already_holds_the_machine_reserve(monkeypatch) -> None:
    gib = 1024**3
    monkeypatch.setattr(
        host_capacity, "_probe_memory_bytes", lambda: (64 * gib, 40 * gib)
    )
    monkeypatch.delenv(host_capacity.MEMORY_SLICE_ENV, raising=False)
    capacity = host_capacity.memory_capacity()
    assert host_capacity.headroom_bytes(capacity) == host_capacity.reserve_bytes(
        64 * gib
    )
    # A parent cut the slice from memory beyond the reserve.
    monkeypatch.setenv(host_capacity.MEMORY_SLICE_ENV, str(3 * gib))
    delegated = host_capacity.memory_capacity()
    assert host_capacity.headroom_bytes(delegated) == 0
    assert delegated.bounded_bytes == (3 * gib, 3 * gib)
    # Headroom belongs to this captured allocation, not a later environment
    # or a second native observation of a different host state.
    monkeypatch.setenv(host_capacity.MEMORY_SLICE_ENV, "later-invalid-selection")
    monkeypatch.setattr(
        host_capacity,
        "_probe_memory_bytes",
        lambda: pytest.fail("headroom must not observe memory a second time"),
    )
    assert host_capacity.headroom_bytes(capacity) == host_capacity.reserve_bytes(
        64 * gib
    )
    assert host_capacity.headroom_bytes(delegated) == 0
