from __future__ import annotations

import contextlib
import hashlib
import importlib.util
import json
import math
import os
import shutil
import sys
from pathlib import Path

import molt.dx as molt_dx
import pytest

from tests.process_guard_common import (
    run_custody_subject_process,
    run_guarded_test_process,
)


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

SOURCE = {
    "schema": demo_bench.SOURCE_SCHEMA,
    "head": "0123456789abcdef0123456789abcdef01234567",
    "dirty": False,
    "status_sha256": hashlib.sha256(b"").hexdigest(),
    "tracked_diff_sha256": hashlib.sha256(b"").hexdigest(),
}


@pytest.fixture
def source_identity(monkeypatch: pytest.MonkeyPatch) -> dict[str, object]:
    """Typed Git identity; the real capture shells out to git."""
    monkeypatch.setattr(demo_bench, "capture_source_identity", lambda: dict(SOURCE))
    return SOURCE


def run_checker(target: Path):
    return run_guarded_test_process(
        [sys.executable, str(DEMO_BENCH_PATH), "--check-regressions", str(target)],
        cwd=REPO_ROOT,
    )


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
    source_identity: dict[str, object],
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
    monkeypatch.delenv("MOLT_DEMO_RUN_DIR", raising=False)

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
    passing = run_checker(path)
    assert passing.returncode == 0, passing.stderr
    assert passing.stdout.strip() == "Perf check OK"
    assert "not bound to a demo run" in passing.stderr
    artifact["offload_table"]["metrics"]["http_req_duration"] = {}
    path.write_text(json.dumps(artifact))
    failing = run_checker(path)
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
    # The raw shell's process-group cleanup is the subject under test.
    proc = run_custody_subject_process(
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
    probe = run_custody_subject_process(
        [BASH, "-c", 'kill -0 "$1" 2>/dev/null', "probe", descendant_pid],
        capture_output=True,
        text=True,
        timeout=20,
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


def test_demo_regression_cli_checks_partial_failed_run(tmp_path, source_identity):
    run_dir = demo_bench.prepare_run(tmp_path / "demo-runs")
    (run_dir / "k6_baseline_summary.json").write_text(
        json.dumps(scenario_summary(1100))
    )
    proc = run_checker(run_dir)
    assert proc.returncode != 0
    assert "baseline: p95 1100ms >= 1000.0ms" in proc.stderr
    assert "offload: missing scenario summary" in proc.stderr
    assert "offload_table: missing scenario summary" in proc.stderr
    assert "Perf check OK" not in proc.stdout


def stack_run(
    monkeypatch: pytest.MonkeyPatch, runs_root: Path, worker_bin: Path | None = None
) -> Path:
    """Bind a run as the outer stack does and stub what main() would launch."""
    run_dir = demo_bench.prepare_run(runs_root)
    monkeypatch.setenv("MOLT_DEMO_RUN_DIR", str(run_dir))
    if worker_bin is None:
        monkeypatch.delenv("MOLT_DEMO_WORKER_BIN", raising=False)
    else:
        monkeypatch.setenv("MOLT_DEMO_WORKER_BIN", str(worker_bin))
    monkeypatch.delenv("MOLT_DEMO_METRICS_PATH", raising=False)
    monkeypatch.setenv("MOLT_MEMORY_GUARD", "0")
    # main() rebinds RESULTS_DIR to the run; patching restores the module.
    monkeypatch.setattr(demo_bench, "RESULTS_DIR", runs_root.parent)
    monkeypatch.setattr(
        demo_bench.harness_memory_guard,
        "repo_process_sentinel",
        lambda **kwargs: contextlib.nullcontext(),
    )
    monkeypatch.setattr(demo_bench, "collect_tool_versions", lambda: {})
    monkeypatch.setattr(demo_bench, "collect_machine_info", lambda: {})
    monkeypatch.setattr(
        demo_bench,
        "run_scenario",
        lambda name, script, env: demo_bench.BenchResult(
            name=name,
            req_per_s=2.0,
            p50=5.0,
            p95=10.0,
            p99=12.0,
            p999=15.0,
            error_rate=0.0,
            raw=scenario_summary(),
        ),
    )
    return run_dir


def complete_run(
    monkeypatch: pytest.MonkeyPatch, runs_root: Path, worker_bin: Path | None = None
) -> Path:
    run_dir = stack_run(monkeypatch, runs_root, worker_bin)
    demo_bench.main()
    return run_dir


def test_demo_explicit_run_is_bound_end_to_end(monkeypatch, tmp_path, source_identity):
    worker = tmp_path / "molt-worker"
    worker.write_bytes(b"worker build")
    run_dir = complete_run(monkeypatch, tmp_path / "demo-runs", worker)
    manifest = json.loads((run_dir / "run.json").read_text(encoding="utf-8"))
    assert manifest["run_id"] == run_dir.name
    assert manifest["source"] == SOURCE
    (composite,) = run_dir.glob("demo_k6_*.json")
    run = json.loads(composite.read_text(encoding="utf-8"))["run"]
    assert run["run_id"] == run_dir.name
    assert run["source"] == run["source_end"] == SOURCE
    worker_digest = hashlib.sha256(b"worker build").hexdigest()
    assert run["worker_binary"]["sha256"] == worker_digest
    proc = run_checker(run_dir)
    assert proc.returncode == 0, proc.stderr
    assert f"Perf check OK: demo run {run_dir.name} at {SOURCE['head']}" in proc.stdout
    assert "not CPython-relative or release acceptance evidence" in proc.stdout


def test_demo_startup_failure_cannot_pass_on_prior_success(
    monkeypatch, tmp_path, source_identity
):
    runs_root = tmp_path / "demo-runs"
    prior = complete_run(monkeypatch, runs_root)
    retained = {path.name: path.read_bytes() for path in prior.iterdir()}
    # The stack binds the next run, then dies before main(): build, preflight,
    # readiness or a missing k6.
    current = demo_bench.prepare_run(runs_root)
    proc = run_checker(current)
    assert proc.returncode != 0
    assert "Perf check OK" not in proc.stdout
    assert f"Perf check FAILED: demo run {current.name}" in proc.stderr
    for name in ("baseline", "offload", "offload_table"):
        assert f"{name}: missing scenario summary" in proc.stderr
    assert f"run {current.name}: incomplete" in proc.stderr
    # The prior run survives untouched and passes only when named explicitly.
    assert {path.name: path.read_bytes() for path in prior.iterdir()} == retained
    assert demo_bench.check_run_directory(prior)[2] == []


def test_demo_results_root_is_not_a_run(monkeypatch, tmp_path, source_identity):
    # PERF-STALE-001: a lone passing composite in the results root used to pass.
    (tmp_path / "demo_k6_20260101T000000.json").write_text(
        json.dumps(
            {
                name: scenario_summary()
                for name in ("baseline", "offload", "offload_table")
            }
        )
    )
    complete_run(monkeypatch, tmp_path / "demo-runs")
    for root in (tmp_path, tmp_path / "demo-runs"):
        proc = run_checker(root)
        assert proc.returncode != 0
        assert "Perf check OK" not in proc.stdout
        assert "is not a demo run directory" in proc.stderr


def test_demo_run_directories_are_exclusive_and_contained(tmp_path, source_identity):
    runs_root = tmp_path / "demo-runs"
    first = demo_bench.prepare_run(runs_root)
    marker = (first / "run.json").read_bytes()
    with pytest.raises(ValueError, match="never reused"):
        demo_bench.prepare_run(runs_root, str(first))
    assert (first / "run.json").read_bytes() == marker
    ci_run = demo_bench.prepare_run(runs_root, str(runs_root / "perf-demo-123456-2"))
    assert ci_run == runs_root.resolve() / "perf-demo-123456-2"
    for escape in (
        tmp_path / "outside-run-0001",
        runs_root / ".." / "escaped-run-0001",
        runs_root / "nested" / "run-00000001",
        runs_root / "Upper-Case-Run",
        runs_root / "short",
    ):
        with pytest.raises(ValueError, match="must be"):
            demo_bench.prepare_run(runs_root, str(escape))
    assert [path.name for path in tmp_path.iterdir()] == ["demo-runs"]
    assert {path.name for path in runs_root.iterdir()} == {first.name, ci_run.name}


def test_demo_run_identity_mismatch_fails_closed(
    monkeypatch, tmp_path, source_identity
):
    runs_root = tmp_path / "demo-runs"
    first = complete_run(monkeypatch, runs_root)
    (composite,) = first.glob("demo_k6_*.json")
    retained = composite.read_bytes()
    # Rerunning the bench into a finished run is refused; its evidence is kept.
    with pytest.raises(SystemExit, match="already has benchmark output"):
        demo_bench.main()
    assert composite.read_bytes() == retained
    # Another run's composite copied into a failed run is not that run's evidence.
    failed = demo_bench.prepare_run(runs_root)
    shutil.copy2(composite, failed / composite.name)
    with pytest.raises(ValueError, match="not bound to demo run"):
        demo_bench.check_run_directory(failed)
    # A marker naming another run does not bind this directory.
    manifest = json.loads((first / "run.json").read_text(encoding="utf-8"))
    manifest["run_id"] = failed.name
    (first / "run.json").write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises(ValueError, match="does not match directory"):
        demo_bench.check_run_directory(first)


def test_demo_run_evidence_cannot_escape_through_symlinks(tmp_path, source_identity):
    run_dir = demo_bench.prepare_run(tmp_path / "demo-runs")
    outside = tmp_path / "outside.json"
    outside.write_text(json.dumps(scenario_summary()), encoding="utf-8")
    linked_run = tmp_path / "linked-run"
    try:
        (run_dir / "k6_baseline_summary.json").symlink_to(outside)
        linked_run.symlink_to(run_dir, target_is_directory=True)
    except OSError:
        pytest.skip("symlink creation is not permitted on this host")
    with pytest.raises(ValueError, match="not a regular file"):
        demo_bench.check_run_directory(run_dir)
    with pytest.raises(ValueError, match="must be a real directory"):
        demo_bench.check_run_directory(linked_run)


@pytest.mark.parametrize("end", ["mutated", "unavailable"])
def test_demo_source_change_or_loss_unbinds_the_run(monkeypatch, tmp_path, end):
    def mutated():
        return dict(SOURCE, tracked_diff_sha256="0" * 64)

    def unavailable():
        raise ValueError("git rev-parse HEAD failed with exit 128")

    runs_root = tmp_path / "demo-runs"
    captures = [lambda: dict(SOURCE), mutated if end == "mutated" else unavailable]
    monkeypatch.setattr(
        demo_bench, "capture_source_identity", lambda: captures.pop(0)()
    )
    run_dir = stack_run(monkeypatch, runs_root)
    with pytest.raises(SystemExit, match="not source-bound"):
        demo_bench.main()
    assert demo_bench.check_run_directory(run_dir)[2] == [
        f"run {run_dir.name}: source identity changed or was unavailable "
        "when the run finished"
    ]
    # Without Git at startup, no run directory is bound at all.
    monkeypatch.setattr(demo_bench, "capture_source_identity", unavailable)
    with pytest.raises(ValueError, match="exit 128"):
        demo_bench.prepare_run(runs_root)
    assert [path.name for path in runs_root.iterdir()] == [run_dir.name]


def test_stack_and_nightly_bind_and_check_one_run_directory():
    import yaml

    stack = (REPO_ROOT / "bench/scripts/run_stack.sh").read_text(encoding="utf-8")
    bind = stack.index("--prepare-run")
    for startup in (
        "tools/run_context_env.py",
        "tools/guarded_exec.py",
        "uv sync",
        "cargo build",
        "# Start worker",
        "/health/",
    ):
        assert bind < stack.index(startup), startup
    workflow = yaml.safe_load(
        (REPO_ROOT / ".github/workflows/perf_demo.yml").read_text(encoding="utf-8")
    )
    job = workflow["jobs"]["demo-perf"]
    run_dir = job["env"]["MOLT_DEMO_RUN_DIR"]
    assert run_dir.startswith("${{ github.workspace }}/bench/results/demo-runs/")
    assert "${{ github.run_id }}" in run_dir and "${{ github.run_attempt }}" in run_dir
    (check,) = [
        step for step in job["steps"] if "--check-regressions" in step.get("run", "")
    ]
    assert check["if"] == "always()"
    assert '"${MOLT_DEMO_RUN_DIR:?}"' in check["run"]


@pytest.mark.parametrize("schema", ["legacy", "handle-summary"])
@pytest.mark.parametrize("rate", [0.0, 0.01])
def test_k6_rate_export_family_preserves_error_gate(schema, rate):
    artifact = {
        name: scenario_summary() for name in ("baseline", "offload", "offload_table")
    }
    for block in artifact.values():
        block["metrics"]["http_req_failed"] = (
            {"value": rate} if schema == "legacy" else {"rate": rate}
        )
        if schema == "handle-summary":
            block["metrics"] = {
                name: {"values": values} for name, values in block["metrics"].items()
            }
    failures = demo_bench.check_regressions(artifact)
    assert failures == (
        [] if rate == 0.0 else [f"{name}: error rate 0.01 >= 0.01" for name in artifact]
    )
    parsed = demo_bench.parse_k6_summary("baseline", artifact["baseline"])
    assert parsed.error_rate == rate
    assert parsed.req_per_s == 2.0
    assert parsed.p95 == 10.0


def test_k6_conflicting_rate_representations_fail_closed():
    artifact = {
        name: scenario_summary() for name in ("baseline", "offload", "offload_table")
    }
    artifact["baseline"]["metrics"]["http_req_failed"] = {"value": 0.5, "rate": 0.0}
    assert demo_bench.check_regressions(artifact) == [
        "baseline: missing or invalid error_rate"
    ]
