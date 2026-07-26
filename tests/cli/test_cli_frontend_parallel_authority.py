from __future__ import annotations

import inspect
from pathlib import Path

import pytest

import molt.cli as cli
from molt import dx
from molt.cli import build_diagnostics
from molt.cli import frontend_execution
from molt.cli import frontend_parallel

_FRONTEND_PARALLEL_NAMES = (
    "_append_frontend_parallel_layer_detail",
    "_append_frontend_serial_disabled_layer_detail",
    "_choose_frontend_parallel_layer_workers",
    "_fresh_frontend_parallel_layer_state",
    "_frontend_layer_plan",
    "_frontend_layer_policy_summary",
    "_frontend_layer_static_metrics",
    "_frontend_parallel_layer_detail",
    "_frontend_parallel_policy_payload",
    "_frontend_parallel_result_error",
    "_frontend_parallel_worker_timing_inputs",
    "_frontend_result_timings",
    "_frontend_serial_worker_mode",
    "_initialize_frontend_parallel_details",
    "_known_classes_snapshot_copy",
    "_layer_cache_hit_count",
    "_note_frontend_parallel_layer_failure",
    "_predict_frontend_module_cost",
    "_record_parallel_cached_module_result",
    "_record_parallel_layer_module_timing",
    "_record_parallel_worker_result",
    "_record_serial_frontend_worker_timing",
    "_resolve_frontend_parallel_config",
    "_resolve_frontend_parallel_min_modules",
    "_resolve_frontend_parallel_min_predicted_cost",
    "_resolve_frontend_parallel_module_workers",
    "_resolve_frontend_parallel_worker_resources",
    "_resolve_frontend_parallel_stdlib_min_cost_scale",
    "_resolve_frontend_parallel_target_cost_per_worker",
    "_summarize_frontend_parallel_worker_timings",
    "_summarize_worker_timing_items",
    "_take_frontend_parallel_layer_result",
    "_worker_timing_summary_payload",
)

_FRONTEND_PARALLEL_DEFINITIONS = tuple(
    f"def {name}(" for name in _FRONTEND_PARALLEL_NAMES
)


def test_cli_frontend_parallel_authority_is_single_home() -> None:
    for name in _FRONTEND_PARALLEL_NAMES:
        assert hasattr(frontend_parallel, name)
        assert not hasattr(frontend_execution, name)
        assert not hasattr(cli, name)

    frontend_execution_source = inspect.getsource(frontend_execution)
    cli_source = inspect.getsource(cli)
    for marker in _FRONTEND_PARALLEL_DEFINITIONS:
        assert marker not in frontend_execution_source
        assert marker not in cli_source


def test_serial_cache_hit_counts_as_layer_cache_hit(tmp_path: Path) -> None:
    recorded: list[dict[str, object]] = []

    def record_worker_timing(**kwargs: object) -> dict[str, object]:
        return dict(kwargs)

    frontend_parallel._record_serial_frontend_worker_timing(
        record_frontend_parallel_worker_timing=record_worker_timing,
        recorded_worker_timings=recorded,
        layer_index=0,
        module_name="cached",
        module_path=tmp_path / "cached.py",
        mode="serial_cache_hit",
        total_s=0.0,
        reused_s=2.5,
    )

    assert recorded[0]["mode"] == "serial_cache_hit"
    assert recorded[0]["reused_ms"] == 2500.0
    assert frontend_parallel._layer_cache_hit_count(recorded) == 1


def test_frontend_parallel_defaults_to_memory_bounded_auto(monkeypatch) -> None:
    gib = 1024**3
    monkeypatch.delenv("MOLT_FRONTEND_PARALLEL_MODULES", raising=False)
    monkeypatch.setattr(
        frontend_parallel, "_system_memory_bytes", lambda: (64 * gib, 7 * gib)
    )
    monkeypatch.setattr(frontend_parallel.os, "cpu_count", lambda: 16)

    assert frontend_parallel._resolve_frontend_parallel_module_workers() == 6

    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "12")
    assert frontend_parallel._resolve_frontend_parallel_module_workers() == 6

    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "0")
    monkeypatch.setattr(
        frontend_parallel,
        "_system_memory_bytes",
        lambda **_: pytest.fail("explicit disable must not probe host resources"),
    )
    assert frontend_parallel._resolve_frontend_parallel_module_workers() == 0

    monkeypatch.setattr(
        frontend_parallel, "_system_memory_bytes", lambda: (64 * gib, 7 * gib)
    )
    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "typo")
    with pytest.raises(ValueError, match="must be auto"):
        frontend_parallel._resolve_frontend_parallel_module_workers()

    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "-2")
    with pytest.raises(ValueError, match="integer must be"):
        frontend_parallel._resolve_frontend_parallel_module_workers()


def test_shared_worker_policy_caps_cpu_by_total_and_live_memory(monkeypatch) -> None:
    gib = 1024**3
    monkeypatch.setattr(dx, "_system_memory_bytes", lambda: (64 * gib, 5 * gib))

    assert (
        dx._memory_bounded_worker_count(
            bytes_per_worker=gib,
            headroom_bytes=2 * gib,
            cpu_count=32,
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

    assert dx._linux_system_memory_bytes(
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

    assert dx._linux_system_memory_bytes(
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

    assert dx._linux_system_memory_bytes(
        meminfo_path=meminfo,
        cgroup_root=tmp_path / "cgroup",
        cgroup_membership_path=membership,
    ) == (8 * gib, 8 * gib)


def test_darwin_worker_memory_uses_native_sysctl_snapshot(monkeypatch) -> None:
    gib = 1024**3
    values = {
        "hw.memsize": 32 * gib,
        "hw.pagesize": 4096,
        "vm.page_free_count": 262_144,
        "vm.page_inactive_count": 524_288,
        "vm.page_speculative_count": 262_144,
    }
    monkeypatch.setattr(dx, "_darwin_sysctl_integer", values.get)

    assert dx._darwin_system_memory_bytes() == (32 * gib, 4 * gib)


def test_frontend_worker_policy_payload_exposes_resource_authority(monkeypatch) -> None:
    gib = 1024**3
    monkeypatch.delenv("MOLT_FRONTEND_PARALLEL_MODULES", raising=False)
    monkeypatch.setattr(
        frontend_parallel, "_system_memory_bytes", lambda: (64 * gib, 5 * gib)
    )
    monkeypatch.setattr(frontend_parallel.os, "cpu_count", lambda: 16)
    config = frontend_parallel._resolve_frontend_parallel_config(module_count=8)

    payload = frontend_parallel._frontend_parallel_policy_payload(config)

    assert config.enabled
    assert payload["worker_selection"] == "default_auto"
    assert payload["worker_cpu_count"] == 16
    assert payload["worker_memory_ceiling"] == 4
    assert payload["system_total_memory_bytes"] == 64 * gib
    assert payload["system_available_memory_bytes"] == 5 * gib
    assert payload["worker_memory_bytes"] > 0
    assert payload["worker_memory_headroom_bytes"] > payload["worker_memory_bytes"]


def test_frontend_pool_never_spawns_more_processes_than_modules(monkeypatch) -> None:
    gib = 1024**3
    monkeypatch.setenv("MOLT_FRONTEND_PARALLEL_MODULES", "32")
    monkeypatch.setattr(
        frontend_parallel, "_system_memory_bytes", lambda: (128 * gib, 64 * gib)
    )
    monkeypatch.setattr(frontend_parallel.os, "cpu_count", lambda: 32)

    config = frontend_parallel._resolve_frontend_parallel_config(module_count=2)

    assert config.enabled
    assert config.workers == 2


def test_frontend_resource_policy_is_visible_in_human_diagnostics(capsys) -> None:
    build_diagnostics._emit_build_diagnostics(
        diagnostics={
            "total_sec": 0.1,
            "frontend_parallel": {
                "enabled": True,
                "workers": 4,
                "mode": "process_pool_reused",
                "reason": "enabled",
                "policy": {
                    "min_modules": 2,
                    "min_predicted_cost": 32768.0,
                    "target_cost_per_worker": 65536.0,
                    "worker_selection": "adaptive_default",
                    "worker_memory_bytes": 768 * 1024 * 1024,
                    "worker_memory_headroom_bytes": 2 * 1024 * 1024 * 1024,
                    "worker_requested": None,
                    "worker_cpu_count": 16,
                    "worker_memory_ceiling": 8,
                    "system_total_memory_bytes": 32 * 1024 * 1024 * 1024,
                    "system_available_memory_bytes": 10 * 1024 * 1024 * 1024,
                },
            },
        },
        diagnostics_path=None,
        json_output=False,
    )

    stderr = capsys.readouterr().err
    assert (
        "frontend_parallel.resources: selection=adaptive_default "
        "worker_memory_mib=768.0 worker_memory_headroom_mib=2048.0 "
        "requested=auto cpu_count=16 memory_ceiling=8 "
        "system_total_mib=32768.0 system_available_mib=10240.0" in stderr
    )
