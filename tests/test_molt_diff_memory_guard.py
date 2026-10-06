from __future__ import annotations

import importlib.util
import os
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "tests" / "molt_diff.py"


def _load_diff_module():
    spec = importlib.util.spec_from_file_location(
        "molt_diff_memory_guard_under_test", SCRIPT_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@pytest.mark.parametrize("child_returncode", [0, 137])
def test_run_subprocess_keeps_infrastructure_outcome_without_inventing_rss_trip(
    tmp_path, monkeypatch, child_returncode
):
    module = _load_diff_module()
    failure = module.memory_guard.GuardInfrastructureFailure(
        phase="temporary_artifact_custody", details=("invalid retained index",)
    )
    guarded = module.harness_memory_guard.GuardedCompletedProcess(
        ["fixture"],
        child_returncode or module.memory_guard.INFRASTRUCTURE_RETURN_CODE,
        "partial",
        "custody incomplete",
        elapsed_s=0.1,
        child_returncode=child_returncode,
        infrastructure_failure=failure,
    )
    monkeypatch.setattr(module, "_memory_guard_trip_outcome", lambda: None)
    monkeypatch.setattr(module, "_diff_root", lambda: tmp_path)
    monkeypatch.setattr(module, "_diff_memory_guard_limits", lambda *_: None)
    monkeypatch.setattr(
        module.harness_memory_guard.HarnessExecutionContext,
        "from_env",
        lambda *_args, **_kwargs: SimpleNamespace(
            run=lambda *_args, **_kwargs: guarded
        ),
    )
    events = []
    monkeypatch.setattr(module, "_record_memory_guard_event", events.append)
    result = module._run_subprocess(["fixture"], env={}, timeout=5)
    assert isinstance(result, module.compat_backends.BackendResult)
    assert result.diagnostic_stderr == guarded.child_stderr
    assert result.child_returncode == child_returncode
    assert result.infrastructure_failure is failure
    assert result.stdout == "partial" and result.stderr == "custody incomplete"
    assert events == []


def _configure_guard(
    module,
    monkeypatch,
    tmp_path: Path,
    *,
    process_gb: float,
    tree_gb: float,
    global_gb: float,
) -> object:
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(tmp_path / "diff"))
    monkeypatch.setenv("MOLT_DIFF_TMPDIR", str(tmp_path / "tmp"))
    monkeypatch.setenv("MOLT_DIFF_MAX_PROCESS_RSS_GB", str(process_gb))
    monkeypatch.setenv("MOLT_DIFF_MAX_TREE_RSS_GB", str(tree_gb))
    monkeypatch.setenv("MOLT_DIFF_GLOBAL_RSS_LIMIT_GB", str(global_gb))
    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD_POLL_SEC", "0.02")
    guard_root = tmp_path / "diff" / "memory_guard"
    monkeypatch.setenv(
        module._DIFF_MEMORY_GUARD_TRIP_FILE_ENV,
        str(guard_root / "tripped.json"),
    )
    monkeypatch.setenv(
        module._DIFF_MEMORY_GUARD_EVENTS_JSONL_ENV,
        str(guard_root / "events.jsonl"),
    )
    monkeypatch.setenv(
        module._DIFF_MEMORY_GUARD_GLOBAL_SAMPLES_JSONL_ENV,
        str(guard_root / "global_samples.jsonl"),
    )
    config = module._diff_memory_guard_config()
    module._prepare_memory_guard_run(config)
    module._LAST_SENTINEL_SAMPLE_WRITE = 0.0
    return config


def test_run_subprocess_guard_kills_fast_allocator(tmp_path: Path, monkeypatch) -> None:
    module = _load_diff_module()
    _configure_guard(
        module,
        monkeypatch,
        tmp_path,
        process_gb=0.03,
        tree_gb=0.20,
        global_gb=0.30,
    )
    script = """
import time
chunks = []
for _ in range(16):
    chunks.append(bytearray(4 * 1024 * 1024))
    time.sleep(0.02)
time.sleep(10)
"""

    result = module._run_subprocess(
        [sys.executable, "-c", script],
        env=os.environ.copy(),
        timeout=10.0,
    )

    assert result.returncode == module._DIFF_MEMORY_GUARD_RETURN_CODE
    assert "molt_diff memory guard: RSS limit exceeded" in result.stderr


def test_run_subprocess_guard_accounts_recursive_children(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_diff_module()
    _configure_guard(
        module,
        monkeypatch,
        tmp_path,
        process_gb=0.08,
        tree_gb=0.08,
        global_gb=0.30,
    )
    child = "import time; buf = bytearray(36 * 1024 * 1024); time.sleep(10)"
    script = f"""
import subprocess
import sys
children = [
    subprocess.Popen([sys.executable, "-c", {child!r}])
    for _ in range(2)
]
try:
    for proc in children:
        proc.wait()
finally:
    for proc in children:
        proc.kill()
"""

    result = module._run_subprocess(
        [sys.executable, "-c", script],
        env=os.environ.copy(),
        timeout=10.0,
    )

    assert result.returncode == module._DIFF_MEMORY_GUARD_RETURN_CODE
    assert "scope=process_tree" in result.stderr


def test_shared_sentinel_kills_cumulative_parallel_trees(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_diff_module()
    _configure_guard(
        module,
        monkeypatch,
        tmp_path,
        process_gb=1.0,
        tree_gb=1.0,
        global_gb=0.06,
    )
    groups = [
        module.process_sentinel.ProcessGroup(
            pgid=200,
            matched=True,
            samples=(
                module.memory_guard.ProcessSample(
                    200, 1, 36 * 1024, "molt.cli build a", pgid=200, started_at_ns=1000
                ),
            ),
        ),
        module.process_sentinel.ProcessGroup(
            pgid=300,
            matched=True,
            samples=(
                module.memory_guard.ProcessSample(
                    300, 1, 36 * 1024, "molt.cli build b", pgid=300, started_at_ns=2000
                ),
            ),
        ),
    ]
    terminated: list[int] = []

    def fake_terminate_group(
        pgid: int,
        *,
        grace: float,
        expected_identities: object,
    ) -> None:
        del grace, expected_identities
        terminated.append(pgid)

    module.harness_memory_guard._TERMINATED_PGIDS.clear()
    monkeypatch.setattr(
        module.harness_memory_guard.process_sentinel,
        "process_groups",
        lambda *args, **kwargs: groups,
    )
    monkeypatch.setattr(
        module.harness_memory_guard.process_sentinel,
        "terminate_group",
        fake_terminate_group,
    )
    sentinel = module.harness_memory_guard.repo_process_sentinel(
        repo_root=REPO_ROOT,
        artifact_root=tmp_path / "diff",
        label="unit-diff",
        limits=module._diff_memory_guard_limits(),
        on_scan=module._record_memory_guard_sentinel_sample,
        on_violation=module._record_memory_guard_sentinel_violation,
    )

    sentinel.scan_once()

    assert terminated == [200, 300]
    assert module._memory_guard_trip_outcome() is not None
    evidence = module.harness_outcomes.read_suite_trip(module.os.environ)
    assert evidence is not None and evidence.infrastructure_failure is None
    assert {trip.victim_pgid for trip in evidence.trips} == {200, 300}


def test_memory_guard_clamps_parallel_jobs(tmp_path: Path, monkeypatch) -> None:
    module = _load_diff_module()
    config = _configure_guard(
        module,
        monkeypatch,
        tmp_path,
        process_gb=0.02,
        tree_gb=0.03,
        global_gb=0.07,
    )

    assert module._constrain_jobs_for_memory_guard(16, config=config, log=False) == 2


def test_memory_guard_jsonl_rotation_preserves_recent_file(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_diff_module()
    path = tmp_path / "global_samples.jsonl"
    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD_MAX_SAMPLE_MB", "0.001")
    path.write_text("x" * 1024, encoding="utf-8")

    module._append_memory_guard_jsonl(path, {"event": "sample", "total_gb": 1.0})

    assert path.with_name("global_samples.jsonl.1").exists()
    payload = path.read_text(encoding="utf-8")
    assert '"event": "sample"' in payload
    assert '"total_gb": 1.0' in payload


def test_memory_guard_sample_interval_env_is_bounded(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD_SAMPLE_INTERVAL_SEC", "120")

    assert module._diff_memory_guard_sample_interval_sec() == 60.0


def test_diff_memory_guard_defaults_are_adaptive(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_DIFF_TOTAL_MEMORY_GB", "128")
    monkeypatch.setenv("MOLT_DIFF_MEM_AVAILABLE_GB", "96")
    monkeypatch.delenv("MOLT_DIFF_GLOBAL_RSS_LIMIT_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_GLOBAL_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_TREE_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_TOTAL_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_PROCESS_RSS_GB", raising=False)

    config = module._diff_memory_guard_config()

    assert config.global_gb == pytest.approx(85.6704)
    assert config.max_tree_gb == pytest.approx(51.40224)
    assert config.max_process_gb == pytest.approx(46.262016)
    assert config.child_rlimit_gb == pytest.approx(46.262016)


def test_diff_memory_guard_refresh_accounts_active_tree_rss(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_DIFF_TOTAL_MEMORY_GB", "128")
    monkeypatch.setenv("MOLT_DIFF_MEM_AVAILABLE_GB", "46")
    monkeypatch.delenv("MOLT_DIFF_GLOBAL_RSS_LIMIT_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_GLOBAL_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_TREE_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_TOTAL_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_PROCESS_RSS_GB", raising=False)

    config = module._diff_memory_guard_config(accounted_rss_kb=50 * 1024 * 1024)

    assert config.global_gb == pytest.approx(85.6704)
    assert config.max_tree_gb == pytest.approx(51.40224)
    assert config.max_process_gb == pytest.approx(46.262016)
    assert config.child_rlimit_gb == pytest.approx(46.262016)


def test_shared_sentinel_refreshes_limits_from_active_tree_rss(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(tmp_path / "diff"))
    monkeypatch.setenv("MOLT_DIFF_TOTAL_MEMORY_GB", "128")
    monkeypatch.setenv("MOLT_DIFF_MEM_AVAILABLE_GB", "46")
    monkeypatch.delenv("MOLT_DIFF_GLOBAL_RSS_LIMIT_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_GLOBAL_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_TREE_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_TOTAL_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_PROCESS_RSS_GB", raising=False)
    module._prepare_memory_guard_run(module._diff_memory_guard_config())
    module._LAST_SENTINEL_SAMPLE_WRITE = 0.0
    gb = 1024 * 1024
    groups = [
        module.process_sentinel.ProcessGroup(
            pgid=200,
            matched=True,
            samples=(
                module.memory_guard.ProcessSample(200, 1, 1 * gb, "root", pgid=200),
                module.memory_guard.ProcessSample(
                    201, 200, 25 * gb, "rustc-a", pgid=200
                ),
                module.memory_guard.ProcessSample(
                    202, 200, 24 * gb, "rustc-b", pgid=200
                ),
            ),
        )
    ]
    sample_payloads: list[dict[str, object]] = []
    monkeypatch.setattr(
        module.harness_memory_guard.process_sentinel,
        "process_groups",
        lambda *args, **kwargs: groups,
    )
    monkeypatch.setattr(
        module,
        "_record_memory_guard_sample",
        lambda payload: sample_payloads.append(payload),
    )
    sentinel = module.harness_memory_guard.repo_process_sentinel(
        repo_root=REPO_ROOT,
        artifact_root=tmp_path / "diff",
        label="unit-diff-refresh",
        limits=module._diff_memory_guard_limits(),
        on_scan=module._record_memory_guard_sentinel_sample,
        on_violation=module._record_memory_guard_sentinel_violation,
    )

    sentinel.scan_once()

    assert not (tmp_path / "diff" / "memory_guard" / "tripped.json").exists()
    assert sample_payloads
    limits = sample_payloads[-1]["limits"]
    assert isinstance(limits, dict)
    assert limits["max_global_rss_gb"] == pytest.approx(85.6704)


def test_diff_scheduler_uses_memory_scaled_job_budget(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_DIFF_TOTAL_MEMORY_GB", "128")
    monkeypatch.setenv("MOLT_DIFF_MEM_AVAILABLE_GB", "96")
    monkeypatch.delenv("MOLT_DIFF_MEM_PER_JOB_GB", raising=False)
    monkeypatch.setattr(module.os, "cpu_count", lambda: 12)

    config = module._diff_memory_guard_config()

    assert module._memory_guard_scheduler_per_job_gb(config) == pytest.approx(7.1392)
    assert module._memory_guard_max_jobs(config) == 12
    assert module._default_jobs() == 12
    payload = module._config_payload(config)
    assert payload["resource_pressure"]["schema"] == "molt.resource_pressure.v2"
    assert payload["resource_pressure"]["diff"]["max_jobs"] == 12


def test_diff_default_jobs_use_guard_budget_under_memory_pressure(
    monkeypatch,
) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_DIFF_TOTAL_MEMORY_GB", "128")
    monkeypatch.setenv("MOLT_DIFF_MEM_AVAILABLE_GB", "32")
    monkeypatch.delenv("MOLT_DIFF_MEM_PER_JOB_GB", raising=False)
    monkeypatch.setattr(module.os, "cpu_count", lambda: 64)

    config = module._diff_memory_guard_config()

    assert config.global_gb == pytest.approx(23.5904)
    assert module._memory_guard_scheduler_per_job_gb(config) == pytest.approx(1.0)
    assert module._memory_guard_max_jobs(config) == 23
    assert module._default_jobs() == 23


def test_diff_memory_guard_inherits_shared_parent_overrides(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.delenv("MOLT_DIFF_MAX_PROCESS_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_TOTAL_RSS_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_GLOBAL_RSS_LIMIT_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_MAX_GLOBAL_RSS_GB", raising=False)
    monkeypatch.setenv("MOLT_MAX_PROCESS_RSS_GB", "7")
    monkeypatch.setenv("MOLT_MAX_TOTAL_RSS_GB", "8")
    monkeypatch.setenv("MOLT_MAX_GLOBAL_RSS_GB", "9")
    monkeypatch.setenv("MOLT_CHILD_RLIMIT_GB", "10")

    config = module._diff_memory_guard_config()

    assert config.max_process_gb == pytest.approx(7)
    assert config.max_tree_gb == pytest.approx(8)
    assert config.global_gb == pytest.approx(9)
    assert config.child_rlimit_gb == pytest.approx(10)


def test_diff_memory_guard_family_overrides_parent_controls(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_MAX_PROCESS_RSS_GB", "7")
    monkeypatch.setenv("MOLT_MAX_TOTAL_RSS_GB", "8")
    monkeypatch.setenv("MOLT_MAX_GLOBAL_RSS_GB", "9")
    monkeypatch.setenv("MOLT_CHILD_RLIMIT_GB", "10")
    monkeypatch.setenv("MOLT_DIFF_MAX_PROCESS_RSS_GB", "3")
    monkeypatch.setenv("MOLT_DIFF_MAX_TREE_RSS_GB", "4")
    monkeypatch.setenv("MOLT_DIFF_GLOBAL_RSS_LIMIT_GB", "5")
    monkeypatch.setenv("MOLT_DIFF_CHILD_RLIMIT_GB", "6")

    config = module._diff_memory_guard_config()

    assert config.max_process_gb == pytest.approx(3)
    assert config.max_tree_gb == pytest.approx(4)
    assert config.global_gb == pytest.approx(5)
    assert config.child_rlimit_gb == pytest.approx(6)


def test_diff_memory_guard_global_disable_is_ignored(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_MEMORY_GUARD", "0")
    monkeypatch.delenv("MOLT_DIFF_MEMORY_GUARD", raising=False)

    assert module._diff_memory_guard_enabled() is True

    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD", "0")
    assert module._diff_memory_guard_enabled() is True


def test_diff_stdlib_profile_ignores_ambient_build_profile() -> None:
    module = _load_diff_module()
    profile = module.compat_backends.stdlib_profile_from_environment(
        {
            "MOLT_STDLIB_PROFILE": "micro",
        }
    )

    assert profile is None


def test_diff_stdlib_profile_rejects_invalid_values() -> None:
    module = _load_diff_module()
    with pytest.raises(ValueError, match="must be 'micro' or 'full'"):
        module.compat_backends.stdlib_profile_from_environment(
            {"MOLT_DIFF_STDLIB_PROFILE": "wide"}
        )


def test_metadata_stdlib_profile_is_validated(tmp_path: Path) -> None:
    module = _load_diff_module()
    source = tmp_path / "case.py"

    source.write_text("# MOLT_META: stdlib_profile=full\n", encoding="utf-8")
    assert module._metadata_stdlib_profile(str(source)) == ("full", None)

    source.write_text("# MOLT_META: stdlib_profile=wide\n", encoding="utf-8")
    profile, error = module._metadata_stdlib_profile(str(source))
    assert profile is None
    assert error == "MOLT_META stdlib_profile contains unknown values: wide"

    source.write_text("# MOLT_META: stdlib_profile=full,micro\n", encoding="utf-8")
    profile, error = module._metadata_stdlib_profile(str(source))
    assert profile is None
    assert error == "MOLT_META stdlib_profile must select exactly one value"


def test_diff_rlimit_defaults_to_adaptive_process_budget(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_DIFF_TOTAL_MEMORY_GB", "128")
    monkeypatch.setenv("MOLT_DIFF_MEM_AVAILABLE_GB", "96")
    monkeypatch.delenv("MOLT_DIFF_RLIMIT_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_RLIMIT_MB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_CHILD_RLIMIT_GB", raising=False)

    config = module._diff_memory_guard_config()

    assert config.child_rlimit_gb == pytest.approx(config.max_process_gb)
    assert module._memory_limit_bytes() == config.child_rlimit_kb * 1024
    assert module._memory_limit_bytes() == config.max_process_kb * 1024


def test_diff_measure_rss_is_enabled_by_default(monkeypatch) -> None:
    module = _load_diff_module()
    monkeypatch.delenv("MOLT_DIFF_MEASURE_RSS", raising=False)

    assert module._diff_measure_rss() is True

    monkeypatch.setenv("MOLT_DIFF_MEASURE_RSS", "0")
    assert module._diff_measure_rss() is False


def test_popen_group_kwargs_applies_child_rlimit(monkeypatch) -> None:
    module = _load_diff_module()
    if module.os.name == "nt":
        return
    applied: list[int] = []
    monkeypatch.setenv("MOLT_DIFF_MAX_PROCESS_RSS_GB", "0.5")
    monkeypatch.setenv("MOLT_DIFF_MAX_TREE_RSS_GB", "1.0")
    monkeypatch.setenv("MOLT_DIFF_GLOBAL_RSS_LIMIT_GB", "2.0")
    monkeypatch.setenv("MOLT_DIFF_CHILD_RLIMIT_GB", "0.5")
    monkeypatch.setattr(
        module.memory_guard,
        "_apply_child_resource_limit",
        lambda limit_kb: applied.append(limit_kb),
    )

    kwargs = module._popen_group_kwargs()

    assert kwargs["start_new_session"] is True
    assert callable(kwargs["preexec_fn"])
    kwargs["preexec_fn"]()
    assert applied == [512 * 1024]


def test_popen_group_kwargs_can_disable_child_rlimit(monkeypatch) -> None:
    module = _load_diff_module()
    if module.os.name == "nt":
        return
    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD", "1")
    monkeypatch.setenv("MOLT_DIFF_CHILD_RLIMIT_GB", "0")

    kwargs = module._popen_group_kwargs()

    assert kwargs == {"start_new_session": True}


def test_popen_group_kwargs_keeps_child_rlimit_when_guard_disabled(
    monkeypatch,
) -> None:
    module = _load_diff_module()
    if module.os.name == "nt":
        return
    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD", "0")

    kwargs = module._popen_group_kwargs()

    assert kwargs["start_new_session"] is True
    assert callable(kwargs["preexec_fn"])


def test_run_subprocess_preserves_signal_diagnostic(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _load_diff_module()
    monkeypatch.setenv("MOLT_DIFF_ROOT", str(tmp_path / "diff"))
    monkeypatch.setenv("MOLT_DIFF_TMPDIR", str(tmp_path / "tmp"))
    monkeypatch.setenv("MOLT_DIFF_MEMORY_GUARD", "0")

    signal_script = (
        "import os; os._exit(137)"
        if os.name == "nt"
        else "import os, signal; os.kill(os.getpid(), signal.SIGKILL)"
    )
    result = module._run_subprocess(
        [
            sys.executable,
            "-c",
            signal_script,
        ],
        env=os.environ.copy(),
        timeout=5,
    )

    assert module.memory_guard.exit_signal_payload(result.returncode) is not None
    assert "memory_guard: command exited with SIGKILL" in result.stderr


@pytest.mark.parametrize("identified", [False, True])
def test_suite_trip_partial_births_preserve_identified_victims(
    monkeypatch, tmp_path, identified
):
    import json

    module = _load_diff_module()
    marker = tmp_path / "trip.json"
    entry = _suite_trip_entry()
    samples = entry["shared_sentinel_event"]["violation"]["process_samples"]
    samples[:] = ([{"pid": 11, "started_at_ns": 1000}] if identified else []) + [
        {"pid": 12, "started_at_ns": None},
        {"pid": 13},
        {"pid": True, "started_at_ns": 3000},
        {"pid": 14, "started_at_ns": False},
        None,
    ]
    marker.write_text(
        json.dumps({"event": "guard_tripped", "trips": [entry]}), encoding="utf-8"
    )
    evidence = module.harness_outcomes.read_suite_trip({}, path=marker)
    assert evidence is not None
    if not identified:
        assert evidence.infrastructure_failure.phase == "rss_trip_evidence"
        assert "omitted 5 samples" in evidence.message
        return
    assert evidence.infrastructure_failure is None
    assert evidence.trips[0].process_identities == ((11, 1000),)
    assert evidence.trips[0].unidentified_samples == 5
    assert "omitted 5 unidentified victim samples" in evidence.message
    module.harness_outcomes.publish_suite_trip(marker, entry)
    result = module.compat_backends.suite_trip_outcome(evidence)
    assert result.rss_limit_exceeded and result.returncode == 137
    assert "omitted 5 unidentified victim samples" in result.stderr


@pytest.mark.parametrize("strict", [False, True])
@pytest.mark.parametrize("protocol_death", [False, True])
@pytest.mark.parametrize(
    "case",
    [
        "server",
        "descendant",
        "old_trip",
        "old_descendant",
        "reused",
        "unknown_birth",
        "unrelated",
        "timeout",
        "success",
        "malformed",
    ],
)
def test_batch_suite_trip_uses_request_custody_before_retry_or_fallback(
    monkeypatch, tmp_path, strict, protocol_death, case
):
    from tools.batch_compile_client import (
        BatchCompileRequestCustody,
        BatchCompileResponse,
    )

    module = _load_diff_module()
    marker = tmp_path / "trip.json"
    monkeypatch.setattr(module, "_diff_memory_guard_trip_file", lambda: marker)
    requests = []
    shutdown = []
    child = module.memory_guard.GuardedChildProcess(
        11,
        11,
        11,
        ("batch",),
        "fixture",
        None if case == "unknown_birth" else 1000,
    )
    descendant = case in {"descendant", "old_descendant"}
    victim = 22 if descendant or case == "unrelated" else 11
    born = 2000 if victim == 22 else 1001 if case == "reused" else 1000
    entry = _suite_trip_entry(victim, born)
    event = entry["shared_sentinel_event"]
    event["observed_at_ns"] = 90 if case == "old_trip" else 120
    if descendant:
        event["custody_ancestry"] = [
            {
                "pid": victim,
                "started_at_ns": born,
                "admitted_at_ns": 90 if case == "old_descendant" else 110,
                "ancestors": [{"pid": 11, "started_at_ns": 1000}],
            }
        ]

    class Client:
        def request(self, op, *, params, timeout):
            requests.append(op)
            if case == "malformed":
                marker.write_text("invalid JSON", encoding="utf-8")
            else:
                module.harness_outcomes.publish_suite_trip(marker, entry)
            if case == "timeout":
                raise TimeoutError("request deadline")
            rc = 0 if case == "success" else 1
            custody = BatchCompileRequestCustody(child, 100, rc)
            if protocol_death and case != "success":
                error = RuntimeError("response pipe closed")
                error.batch_request_custody = custody
                raise error
            return BatchCompileResponse(
                {
                    "id": 1,
                    "ok": rc == 0,
                    "returncode": rc,
                    "stdout": "compiler stdout",
                    "stderr": "compiler stderr",
                },
                custody,
            )

    monkeypatch.setattr(
        module, "_batch_compile_server_client", lambda *a, **k: (Client(), None)
    )
    monkeypatch.setattr(
        module, "_shutdown_batch_compile_server", lambda **k: shutdown.append(True)
    )
    monkeypatch.setattr(
        module, "_batch_compile_server_mark_disabled", lambda reason: None
    )
    monkeypatch.setattr(module, "_batch_compile_server_reset_disabled", lambda: None)
    result = module._run_batch_compile_build(
        env={},
        file_path="fixture.py",
        output_root=tmp_path,
        output_binary=tmp_path / "fixture",
        build_profile="dev",
        target_python=None,
        no_cache=False,
        rebuild=False,
        request_timeout=2.0,
        strict_mode=strict,
    )
    matched = case in {"server", "descendant"}
    assert result.rss_limit_exceeded is matched
    assert result.returncode == (
        137
        if matched
        else 124
        if case == "timeout"
        else 0
        if case == "success"
        else module.memory_guard.INFRASTRUCTURE_RETURN_CODE
        if case == "malformed" and protocol_death
        else 127
        if protocol_death
        else 1
    )
    assert requests == ["build"]
    if case != "success":
        assert result.build_failed
    assert result.timed_out is (case == "timeout")
    # Uncertain batch ownership must not erase the failing suite verdict.
    suite = module._memory_guard_trip_outcome()
    assert suite is not None and suite.returncode != 0
    assert suite.rss_limit_exceeded is (case != "malformed")


def test_batch_admission_preserves_prior_suite_trip_without_launch(
    monkeypatch, tmp_path
):
    module = _load_diff_module()
    marker = tmp_path / "trip.json"
    module.harness_outcomes.publish_suite_trip(marker, _suite_trip_entry())
    monkeypatch.setattr(module, "_diff_memory_guard_trip_file", lambda: marker)

    def forbidden(*args, **kwargs):
        raise AssertionError("suite trip must stop a new batch request")

    monkeypatch.setattr(module, "_batch_compile_server_client", forbidden)
    result = module._run_batch_compile_build(
        env={},
        file_path="fixture.py",
        output_root=tmp_path,
        output_binary=tmp_path / "fixture",
        build_profile="dev",
        target_python=None,
        no_cache=False,
        rebuild=False,
        request_timeout=2.0,
        strict_mode=True,
    )
    assert result.rss_limit_exceeded and result.build_failed
    assert result.child_returncode is None


def test_sentinel_publishes_live_request_ancestry_before_termination(
    monkeypatch, tmp_path
):
    module = _load_diff_module()
    guard = module.harness_memory_guard
    sample = module.memory_guard.ProcessSample
    samples = {
        100: sample(100, 1, 1, "suite", pgid=100, started_at_ns=900),
        11: sample(11, 100, 1, "batch", pgid=11, started_at_ns=1000),
        22: sample(22, 11, 5 * 1024 * 1024, "compiler", pgid=22, started_at_ns=2000),
    }
    victim = samples[22]
    marker = tmp_path / "trip.json"
    monkeypatch.delenv("MOLT_BACKEND_DAEMON_SUITE_LEASE", raising=False)
    monkeypatch.setattr(module.memory_guard, "sample_processes", lambda: samples)
    monkeypatch.setattr(guard.time, "monotonic_ns", lambda: 110)
    monkeypatch.setattr(guard, "_claim_terminated_pgid", lambda pgid: True)
    monkeypatch.setattr(
        guard.process_sentinel,
        "process_groups",
        lambda *a, **k: [guard.process_sentinel.ProcessGroup(22, (victim,), True)],
    )

    def publish(_violation, _resolved, payload):
        entry = _suite_trip_entry(22, 2000)
        entry["shared_sentinel_event"] = dict(payload)
        module.harness_outcomes.publish_suite_trip(marker, entry)

    def terminate(*args, **kwargs):
        assert marker.exists()
        samples.clear()

    monkeypatch.setattr(guard.process_sentinel, "terminate_group", terminate)
    sentinel = guard.repo_process_sentinel(
        repo_root=tmp_path,
        artifact_root=tmp_path,
        label="request_fixture",
        limits=guard.HarnessMemoryLimits(
            enabled=True,
            max_process_rss_gb=10,
            max_total_rss_gb=10,
            max_global_rss_gb=4,
            poll_interval=0.01,
        ),
        drain_on_exit=False,
        on_violation=publish,
    )
    sentinel._tree_tracker = module.memory_guard.ProcessTreeTracker(100)
    monkeypatch.setattr(
        sentinel, "_record_skipped_protected_groups", lambda samples: None
    )
    sentinel.scan_once()
    assert not samples
    evidence = module.harness_outcomes.read_suite_trip({}, path=marker)
    assert evidence.infrastructure_failure is None
    child = module.memory_guard.GuardedChildProcess(
        11, 11, 11, ("batch",), "fixture", 1000
    )
    assert evidence.trips[0].matches(child, (), request_started_at_ns=100)
    assert not evidence.trips[0].matches(child, (), request_started_at_ns=120)


def _suite_trip_entry(pid=11, born=1000):
    return {
        "event": "guard_tripped",
        "message": "observed RSS trip",
        "violation": {"rss_kb": 4096, "scope": "process_tree"},
        "shared_sentinel_event": {
            "event": "repo_process_guard_tripped",
            "victim_pgid": pid,
            "violation": {
                "pgid": pid,
                "process_samples": [{"pid": pid, "started_at_ns": born}],
            },
            "termination": {"rss_triggered": True, "attempted": True},
        },
    }


@pytest.mark.parametrize("valid", [False, True])
def test_suite_trip_requires_recorded_rss_and_identity_evidence(
    tmp_path, monkeypatch, valid
):
    import json

    module = _load_diff_module()
    marker = tmp_path / "trip.json"
    monkeypatch.setattr(module, "_diff_memory_guard_trip_file", lambda: marker)
    assert module._memory_guard_trip_outcome() is None
    payload = (
        {"event": "guard_tripped", "trips": [_suite_trip_entry()]}
        if valid
        else {"event": "guard_tripped", "violation": None}
    )
    marker.write_text(json.dumps(payload), encoding="utf-8")
    result = module._memory_guard_trip_outcome()
    assert result is not None
    assert result.rss_limit_exceeded is valid
    assert (result.infrastructure_failure is None) is valid
    assert result.resource_failure == ("rss_limit_exceeded" if valid else None)
    assert module._memory_guard_trip_outcome() is not None


@pytest.mark.parametrize("failure_kind", ["callback", "publication"])
def test_sentinel_failure_reaches_parent_without_trip_file(
    tmp_path, monkeypatch, failure_kind
):
    from molt import file_publication

    module = _load_diff_module()
    marker = tmp_path / "trip.json"
    monkeypatch.setattr(module, "_diff_memory_guard_trip_file", lambda: marker)

    def fail_callback(*args):
        if failure_kind == "publication":
            module._mark_memory_guard_tripped(_suite_trip_entry())
        raise OSError("callback fixture failed")

    if failure_kind == "publication":

        def fail_replace(*args):
            raise OSError("atomic marker publication failed")

        monkeypatch.setattr(file_publication, "durable_replace", fail_replace)
    sentinel = module.harness_memory_guard.repo_process_sentinel(
        repo_root=tmp_path,
        artifact_root=tmp_path,
        label="fixture",
        drain_on_exit=False,
        limits=module._diff_memory_guard_limits(),
        on_violation=fail_callback,
    )
    sentinel.tripped = True
    sentinel._notify_violation(None, None, {})
    result = module._memory_guard_trip_outcome(sentinel)
    assert not marker.exists()
    assert result.infrastructure_failure.phase == "rss_trip_evidence"
    assert result.resource_failure is None
    assert failure_kind in result.stderr


@pytest.mark.parametrize("publication_failed", [False, True])
def test_sentinel_retains_primary_and_stage_cleanup_failure(
    tmp_path, monkeypatch, publication_failed
):
    from molt import file_publication

    module = _load_diff_module()
    marker = tmp_path / "trip.json"
    stage = tmp_path / ".molt-fixture.tmp"
    monkeypatch.setattr(module, "_diff_memory_guard_trip_file", lambda: marker)
    monkeypatch.setattr(file_publication, "staged_file_path", lambda destination: stage)
    original_unlink = Path.unlink

    def publish(staged, destination):
        if publication_failed:
            raise OSError("primary marker publication failed")
        destination.write_bytes(staged.read_bytes())

    def fail_cleanup(path, *args, **kwargs):
        if path == stage:
            raise PermissionError("marker stage cleanup denied")
        return original_unlink(path, *args, **kwargs)

    monkeypatch.setattr(file_publication, "durable_replace", publish)
    monkeypatch.setattr(Path, "unlink", fail_cleanup)
    sentinel = module.harness_memory_guard.repo_process_sentinel(
        repo_root=tmp_path,
        artifact_root=tmp_path,
        label="fixture",
        drain_on_exit=False,
        limits=module._diff_memory_guard_limits(),
        on_violation=lambda *args: module._mark_memory_guard_tripped(
            _suite_trip_entry()
        ),
    )
    sentinel.tripped = True
    sentinel._notify_violation(None, None, {})
    result = module._memory_guard_trip_outcome(sentinel)
    assert marker.exists() is not publication_failed
    assert stage.exists()
    assert result.infrastructure_failure.phase == "rss_trip_evidence"
    assert result.resource_failure is None
    details = result.infrastructure_failure.details
    assert any("marker stage cleanup denied" in detail for detail in details)
    assert "marker stage cleanup denied" in result.stderr
    if publication_failed:
        assert "primary marker publication failed" in details[0]
        assert any(str(stage) in detail for detail in details[1:])
        assert "primary marker publication failed" in result.stderr


@pytest.mark.parametrize("phase", ["build", "run"])
@pytest.mark.parametrize("source", ["guard", "metrics", "batch"])
def test_native_resource_evidence_survives_build_and_run(
    tmp_path, monkeypatch, phase, source
):
    module = _load_diff_module()
    layout = SimpleNamespace(
        repo_root=tmp_path,
        cargo_target_root=tmp_path / "target",
        diff_root=tmp_path / "diff",
        cache_root=tmp_path / "cache",
    )
    monkeypatch.setattr(module, "_apply_memory_limit", lambda: None)
    monkeypatch.setattr(module, "_diff_artifact_layout", lambda **k: layout)
    monkeypatch.setattr(module, "_diff_measure_rss", lambda: source == "metrics")
    monkeypatch.setattr(module, "_diff_fail_rss_kb", lambda: 1024)
    monkeypatch.setattr(
        module, "_diff_batch_compile_server_enabled", lambda: source == "batch"
    )
    monkeypatch.setattr(module, "_diff_batch_compile_server_strict", lambda: False)
    monkeypatch.setattr(module, "_resolve_molt_cli_python", lambda: "fixture-python")
    monkeypatch.setattr(module, "_dyld_preflight_error", lambda _: None)
    monkeypatch.setattr(module, "_record_rss_metrics", lambda *a, **k: None)
    monkeypatch.setattr(
        module,
        "_parse_time_metrics",
        lambda path: {
            "max_rss": 2048 if path.name == phase + ".time" else 1,
        },
    )

    def result(active_phase):
        exhausted = active_phase == phase
        return module.harness_memory_guard.GuardedCompletedProcess(
            ["fixture"],
            125 if exhausted and source != "metrics" else 0,
            "partial stdout",
            "child diagnostic",
            elapsed_s=0.1,
            child_returncode=-9 if exhausted and source != "metrics" else 0,
            violation=module.memory_guard.RssViolation(7, 2048, "fixture")
            if exhausted and source != "metrics"
            else None,
        )

    monkeypatch.setattr(
        module,
        "_run_batch_compile_build",
        lambda **k: module.compat_backends.BackendResult.from_process(result("build")),
    )
    monkeypatch.setattr(
        module,
        "_run_with_optional_time",
        lambda command, **k: result("build" if "molt.cli" in command else "run"),
    )
    context = module.compat_backends.BackendExecutionContext(
        target_python=module.TargetPythonVersion(3, 12, 0),
        build_profile="dev",
        capabilities="",
        environment={},
    )
    actual = module._run_molt_owned(
        "fixture.py",
        build_only=False,
        build_profile="dev",
        daemon_enabled=False,
        no_cache=False,
        rebuild=False,
        extra_env=None,
        execution_context=context,
        output_root=tmp_path,
        environment={},
    )
    assert actual.rss_limit_exceeded and actual.resource_failure == "rss_limit_exceeded"
    assert actual.build_failed is (phase == "build")
    assert actual.diagnostic_stderr == "child diagnostic"
    assert actual.child_returncode == (0 if source == "metrics" else -9)
    assert "partial stdout" in (actual.stderr if phase == "build" else actual.stdout)
