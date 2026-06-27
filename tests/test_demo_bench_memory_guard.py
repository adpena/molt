from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

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
    assert call["env"]["MOLT_EXT_ROOT"] == str(demo_bench.ROOT)
    assert call["env"]["CARGO_TARGET_DIR"] == str(
        demo_bench.ROOT / "target" / "sessions" / call["env"]["MOLT_SESSION_ID"]
    )
    assert call["env"]["TMPDIR"] == str(demo_bench.ROOT / "tmp")


def test_demo_bench_base_env_forces_repo_roots_unless_explicit(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    ambient_root = tmp_path / "ambient"
    explicit_root = tmp_path / "explicit"
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ambient_root))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(ambient_root / "target"))

    env = demo_bench.base_env()

    assert env["MOLT_EXT_ROOT"] == str(demo_bench.ROOT)
    assert env["CARGO_TARGET_DIR"] == str(
        demo_bench.ROOT / "target" / "sessions" / env["MOLT_SESSION_ID"]
    )

    explicit = demo_bench.base_env({"MOLT_EXT_ROOT": str(explicit_root)})

    assert explicit["MOLT_EXT_ROOT"] == str(explicit_root.resolve())
    assert explicit["CARGO_TARGET_DIR"] == str(
        explicit_root.resolve() / "target" / "sessions" / explicit["MOLT_SESSION_ID"]
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
    assert call["cmd"] == ["k6", "run", "--quiet", str(tmp_path / "scenario.js")]
    assert call["prefix"] == demo_bench.BENCH_MEMORY_PREFIX
    assert call["cwd"] == demo_bench.ROOT
    assert call["capture_output"] is True
    assert call["text"] is True
    assert call["env"]["K6_SUMMARY_EXPORT"] == str(summary_path)
    assert call["env"]["MOLT_EXT_ROOT"] == str(demo_bench.ROOT)
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
    monkeypatch.setenv("MOLT_MEMORY_GUARD", "0")

    demo_bench.main()

    assert events == ["enter", "baseline", "offload", "offload_table", "exit"]
