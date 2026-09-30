from __future__ import annotations

import importlib.util
import json
import math
import os
import shutil
import subprocess
import sys
from pathlib import Path

import molt.dx as molt_dx
import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]
DEMO_BENCH_PATH = REPO_ROOT / "bench" / "scripts" / "run_demo_bench.py"
SPEC = importlib.util.spec_from_file_location(
    "demo_bench_under_test",
    DEMO_BENCH_PATH,
)
assert SPEC is not None and SPEC.loader is not None
demo_bench = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = demo_bench
SPEC.loader.exec_module(demo_bench)


def test_demo_bench_run_cmd_uses_memory_guard(monkeypatch: pytest.MonkeyPatch) -> None:
    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": cmd, **kwargs})
        return demo_bench.harness_memory_guard.GuardedCompletedProcess(
            cmd,
            0,
            "k6 v0\n",
            "",
            elapsed_s=0.01,
        )

    monkeypatch.setattr(
        demo_bench.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    assert demo_bench.run_cmd(["k6", "version"]) == "k6 v0"
    call = calls[0]
    assert call["cmd"] == ["k6", "version"]
    assert call["prefix"] == demo_bench.BENCH_MEMORY_PREFIX
    assert call["cwd"] == demo_bench.ROOT
    assert call["capture_output"] is True
    assert call["text"] is True
    assert call["env"]["MOLT_EXT_ROOT"] == str(
        molt_dx.canonical_molt_root(demo_bench.ROOT)
    )
    assert call["env"]["CARGO_TARGET_DIR"] == str(
        molt_dx.cargo_target_dir_for_environment(
            Path(call["env"]["MOLT_EXT_ROOT"]),
            call["env"],
        )
    )
    assert call["env"]["TMPDIR"] == str(Path(call["env"]["MOLT_EXT_ROOT"]) / "tmp")


def test_demo_bench_base_env_forces_repo_roots_unless_explicit(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    ambient_root = tmp_path / "ambient"
    explicit_root = tmp_path / "explicit"
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ambient_root))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(ambient_root / "target"))

    env = demo_bench.base_env()

    assert env["MOLT_EXT_ROOT"] == str(molt_dx.canonical_molt_root(demo_bench.ROOT))
    assert env["CARGO_TARGET_DIR"] == str(
        molt_dx.cargo_target_dir_for_environment(
            Path(env["MOLT_EXT_ROOT"]),
            env,
        )
    )

    explicit = demo_bench.base_env({"MOLT_EXT_ROOT": str(explicit_root)})

    assert explicit["MOLT_EXT_ROOT"] == str(explicit_root.resolve())
    assert explicit["CARGO_TARGET_DIR"] == str(
        molt_dx.cargo_target_dir_for_environment(
            explicit_root.resolve(),
            explicit,
        )
    )


def test_demo_bench_run_k6_uses_live_tree_guard(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    guard_calls: list[dict[str, object]] = []

    summary_path = tmp_path / "summary.json"
    summary_path.write_text(
        '{"metrics":{"http_reqs":{"rate":1,"count":1},"http_req_duration":{},'
        '"http_req_failed":{"rate":0}}}',
        encoding="utf-8",
    )

    def fake_guarded_completed_process(cmd, **kwargs):
        guard_calls.append({"cmd": cmd, **kwargs})
        return demo_bench.harness_memory_guard.GuardedCompletedProcess(
            cmd,
            0,
            "",
            "",
            elapsed_s=0.01,
        )

    monkeypatch.setattr(demo_bench, "extract_proc_matchers", lambda env: {})
    monkeypatch.setattr(demo_bench, "bench_memory_limits", lambda env=None: object())
    monkeypatch.setattr(
        demo_bench.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    data, proc_metrics = demo_bench.run_k6(
        tmp_path / "scenario.js",
        {"K6_SUMMARY_EXPORT": str(summary_path)},
    )

    assert data["metrics"]["http_reqs"]["rate"] == 1
    assert proc_metrics == {}
    call = guard_calls[0]
    assert call["cmd"] == [
        "k6",
        "run",
        "--quiet",
        "--summary-export",
        str(summary_path),
        str(tmp_path / "scenario.js"),
    ]
    assert call["prefix"] == demo_bench.BENCH_MEMORY_PREFIX
    assert call["cwd"] == demo_bench.ROOT
    assert call["capture_output"] is True
    assert call["text"] is True
    assert call["env"]["K6_SUMMARY_EXPORT"] == str(summary_path)
    assert call["env"]["MOLT_EXT_ROOT"] == str(
        molt_dx.canonical_molt_root(demo_bench.ROOT)
    )
    assert call["limits"] is not None


def test_demo_bench_main_wraps_scenarios_in_repo_sentinel(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    events: list[str] = []

    class FakeSentinel:
        def __enter__(self):
            events.append("enter")
            return self

        def __exit__(self, exc_type, exc, tb) -> None:
            events.append("exit")

    def fake_repo_process_sentinel(**kwargs):
        assert kwargs["repo_root"] == demo_bench.ROOT
        assert kwargs["artifact_root"] == demo_bench.ROOT / "tmp" / "bench" / "demo"
        assert kwargs["label"] == "demo_bench"
        return FakeSentinel()

    def fake_run_scenario(name, script, env):
        events.append(name)
        return demo_bench.BenchResult(
            name=name,
            req_per_s=1.0,
            p50=1.0,
            p95=1.0,
            p99=1.0,
            p999=1.0,
            error_rate=0.0,
            raw={"metrics": {}},
        )

    monkeypatch.setattr(
        demo_bench.harness_memory_guard,
        "repo_process_sentinel",
        fake_repo_process_sentinel,
    )
    monkeypatch.setattr(demo_bench, "run_scenario", fake_run_scenario)
    monkeypatch.setattr(demo_bench, "RESULTS_DIR", tmp_path)
    monkeypatch.setattr(demo_bench, "collect_tool_versions", lambda: {})
    monkeypatch.setattr(demo_bench, "collect_machine_info", lambda: {})
    monkeypatch.setattr(demo_bench, "run_cmd", lambda cmd: "test revision")
    monkeypatch.setenv("MOLT_MEMORY_GUARD", "0")

    demo_bench.main()

    assert events == ["enter", "baseline", "offload", "offload_table", "exit"]


def scenario_summary(p95=10.0, *, nested=False):
    duration = {"percentiles": {"95": p95}} if nested else {"p(95)": p95}
    return {
        "metrics": {
            "http_req_duration": duration,
            "http_reqs": {"count": 20, "rate": 2.0},
            "http_req_failed": {"rate": 0.0},
        }
    }


@pytest.mark.parametrize("nested", [False, True])
def test_demo_regressions_validate_both_summary_schemas(nested):
    artifact = {
        name: scenario_summary(nested=nested)
        for name in ("baseline", "offload", "offload_table")
    }
    assert demo_bench.check_regressions(artifact) == []
    artifact["baseline"] = scenario_summary(1000.0, nested=nested)
    assert demo_bench.check_regressions(artifact) == [
        "baseline: p95 1000.0ms >= 1000.0ms"
    ]


@pytest.mark.parametrize("value", [None, -1, math.nan, math.inf, "10", True])
def test_demo_regressions_fail_closed_on_invalid_latency(value):
    artifact = {
        name: scenario_summary() for name in ("baseline", "offload", "offload_table")
    }
    artifact["offload"] = scenario_summary(value)
    assert demo_bench.check_regressions(artifact) == ["offload: missing or invalid p95"]


def test_demo_regressions_reject_missing_and_empty_scenarios():
    artifact = {"baseline": scenario_summary(), "offload": scenario_summary()}
    artifact["baseline"]["metrics"]["http_reqs"]["count"] = 0
    artifact["offload"]["metrics"]["http_req_failed"]["rate"] = 0.01
    assert demo_bench.check_regressions(artifact) == [
        "baseline: no requests completed",
        "offload: error rate 0.01 >= 0.01",
        "offload_table: missing scenario summary",
    ]


def test_demo_regression_cli_reads_actual_artifact(tmp_path):
    artifact = {
        name: scenario_summary() for name in ("baseline", "offload", "offload_table")
    }
    path = tmp_path / "demo.json"
    path.write_text(json.dumps(artifact))
    cmd = [sys.executable, str(DEMO_BENCH_PATH), "--check-regressions", str(path)]
    passing = subprocess.run(cmd, capture_output=True, text=True)
    assert passing.returncode == 0, passing.stderr
    assert passing.stdout.strip() == "Perf check OK"
    artifact["offload_table"]["metrics"]["http_req_duration"] = {}
    path.write_text(json.dumps(artifact))
    failing = subprocess.run(cmd, capture_output=True, text=True)
    assert failing.returncode != 0
    assert "offload_table: missing or invalid p95" in failing.stderr


def test_demo_k6_failure_retains_full_diagnostic(monkeypatch, tmp_path, capsys):
    monkeypatch.setattr(demo_bench, "RESULTS_DIR", tmp_path)
    monkeypatch.setattr(demo_bench, "extract_proc_matchers", lambda env: {})

    def failed_k6(cmd, **kwargs):
        return demo_bench.harness_memory_guard.GuardedCompletedProcess(
            cmd, 99, "threshold breached\n", "guard repro context\n", elapsed_s=0.1
        )

    monkeypatch.setattr(
        demo_bench.harness_memory_guard, "guarded_completed_process", failed_k6
    )
    with pytest.raises(SystemExit, match="exit 99"):
        demo_bench.run_k6(
            tmp_path / "baseline.js",
            {"K6_SUMMARY_EXPORT": str(tmp_path / "summary.json")},
        )
    assert "threshold breached" in capsys.readouterr().err
    assert (
        tmp_path / "k6_baseline_stderr.log"
    ).read_text() == "threshold breached\nguard repro context\n"


BASH = (
    str(Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "Git/bin/bash.exe")
    if os.name == "nt"
    else shutil.which("bash")
)


@pytest.mark.skipif(
    not BASH or not Path(BASH).is_file(), reason="Requires Bash service process groups"
)
@pytest.mark.parametrize("exit_status", [0, 7])
def test_stack_cleanup_waits_for_graceful_service_exit(tmp_path, exit_status):
    service = tmp_path / "service.sh"
    ready = tmp_path / "ready"
    finished = tmp_path / "finished"
    service.write_text(
        'trap \'kill "$child" 2>/dev/null || true; wait "$child" 2>/dev/null || true; '
        'sleep 0.2; printf drained > "$2"; exit 0\' TERM\n'
        'sleep 60 &\nchild=$!\nprintf "%s" "$child" > "$1"\nwait "$child"\n'
    )
    helper = REPO_ROOT / "bench/scripts/stack_lifecycle.sh"
    script = tmp_path / "lifecycle.sh"
    script.write_text(
        'set -euo pipefail\nROOT="$1"\nsource "$2"\n'
        'bash "$3" "$4" "$5" &\nSERVICE_PIDS+=("$!")\n'
        'for _ in {1..100}; do [[ ! -f "$4" ]] || break; sleep 0.05; done\n'
        '[[ -f "$4" ]] || exit 90\nexit "$6"\n'
    )
    proc = subprocess.run(
        [
            BASH,
            script.as_posix(),
            tmp_path.as_posix(),
            helper.as_posix(),
            service.as_posix(),
            ready.as_posix(),
            finished.as_posix(),
            str(exit_status),
        ],
        capture_output=True,
        text=True,
        timeout=20,
    )
    assert proc.returncode == exit_status, proc.stderr
    assert finished.read_text() == "drained"
    descendant_pid = ready.read_text()
    probe = subprocess.run(
        [BASH, "-c", 'kill -0 "$1" 2>/dev/null', "probe", descendant_pid],
        capture_output=True,
        text=True,
    )
    assert probe.returncode != 0, "Service descendant survived stack cleanup"


@pytest.mark.parametrize("key", ["http_req_duration", "http_reqs", "http_req_failed"])
def test_demo_regressions_reject_malformed_metric_blocks(key):
    artifact = {
        name: scenario_summary() for name in ("baseline", "offload", "offload_table")
    }
    artifact["baseline"]["metrics"][key] = []
    assert demo_bench.check_regressions(artifact) == [
        f"baseline: missing or invalid metrics {key}"
    ]


def test_demo_regression_cli_checks_partial_failed_run(tmp_path):
    (tmp_path / "k6_baseline_summary.json").write_text(
        json.dumps(scenario_summary(1100))
    )
    proc = subprocess.run(
        [sys.executable, str(DEMO_BENCH_PATH), "--check-regressions", str(tmp_path)],
        capture_output=True,
        text=True,
    )
    assert proc.returncode != 0
    assert "baseline: p95 1100ms >= 1000.0ms" in proc.stderr
    assert "offload: missing scenario summary" in proc.stderr
    assert "offload_table: missing scenario summary" in proc.stderr
    assert "Perf check OK" not in proc.stdout
