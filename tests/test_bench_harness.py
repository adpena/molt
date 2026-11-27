from __future__ import annotations

import importlib.util
import json
import sys
from collections.abc import Sequence
from contextlib import AbstractContextManager, nullcontext
from pathlib import Path
from typing import Literal

import molt.dx as molt_dx
from molt import custody_layout
from molt.checkout_state import scratch_state_root
from molt.python_interpreter import PythonInterpreter
import pytest
from tests.tools.perf_scoreboard_fixtures import (
    benchmark_oracle_interpreter,
    synthetic_workload,
)


REPO_ROOT = Path(__file__).resolve().parents[1]
HARNESS_PATH = REPO_ROOT / "bench" / "harness.py"
SPEC = importlib.util.spec_from_file_location("bench_harness_under_test", HARNESS_PATH)
assert SPEC is not None and SPEC.loader is not None
bench_harness = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bench_harness)


@pytest.fixture
def cpython() -> PythonInterpreter:
    return benchmark_oracle_interpreter()


def test_bench_harness_run_cmd_uses_memory_guard(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append({"cmd": cmd, **kwargs})
        return bench_harness.harness_memory_guard.GuardedCompletedProcess(
            cmd,
            0,
            "ok\n",
            "",
            elapsed_s=9.0,
            child_elapsed_s=0.02,
            child_returncode=0,
            child_stderr="",
            descendants_closed=True,
            peak=bench_harness.harness_memory_guard.memory_guard.RssViolation(
                pid=1, rss_kb=1024, command="fixture", scope="process"
            ),
        )

    monkeypatch.setattr(
        bench_harness.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    # Select the fixture's exact root independently of host environment,
    # candidate drives and durable checkout-family bindings.
    for key in (
        *molt_dx.CANONICAL_RUN_ENV_KEYS,
        *molt_dx.DEVELOPMENT_ARTIFACT_REQUEST_ENV_KEYS,
        molt_dx.EXTERNAL_ARTIFACT_ROOTS_ENV,
    ):
        monkeypatch.delenv(key, raising=False)
    artifact_root = (tmp_path / "artifacts").resolve()
    toolchain_root = (tmp_path / "toolchains").resolve()
    monkeypatch.setenv("MOLT_EXT_ROOT", str(artifact_root))
    monkeypatch.setenv("MOLT_TARGET_ROOT", str(toolchain_root))

    outcome = bench_harness.run_cmd(
        ["python3", "--version"],
        9.0,
        cwd=tmp_path,
    )

    assert outcome.ok
    assert (outcome.stdout, outcome.stderr, outcome.exit_code, outcome.elapsed_s) == (
        "ok\n",
        "",
        0,
        0.02,
    )
    call = calls[0]
    assert call["cmd"] == ["python3", "--version"]
    assert call["prefix"] == bench_harness.BENCH_MEMORY_PREFIX
    assert call["cwd"] == tmp_path
    assert call["capture_output"] is True
    assert call["text"] is False
    assert call["encoding"] == "utf-8"
    assert call["errors"] == "surrogateescape"
    assert call["timeout"] == 9.0
    assert call["env"]["MOLT_EXT_ROOT"] == str(artifact_root)
    assert call["env"]["MOLT_TARGET_ROOT"] == str(toolchain_root)
    assert toolchain_root.is_dir()
    assert call["env"]["CARGO_TARGET_DIR"] == str(
        molt_dx.cargo_target_dir_for_environment(
            artifact_root, call["env"], source_root=bench_harness.REPO_ROOT
        )
    )
    assert call["env"]["TMPDIR"] == str(
        scratch_state_root(
            custody_layout.scratch_root(artifact_root, bench_harness.REPO_ROOT),
            owner=bench_harness.REPO_ROOT,
            create=False,
        )
    )


@pytest.mark.parametrize("selector", [None, "3.12.16"])
def test_bench_harness_supports_explicit_molt_profile(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    cpython: PythonInterpreter,
    selector: str | None,
) -> None:
    captured: dict[str, object] = {}
    script = tmp_path / "driver.py"
    script.write_text("print(1)\n", encoding="utf-8")
    second_script = tmp_path / "driver_two.py"
    second_script.write_text("print(2)\n", encoding="utf-8")
    resolutions = []

    def resolve(**kwargs: object) -> PythonInterpreter:
        resolutions.append(kwargs["override"])
        return cpython

    monkeypatch.setattr(bench_harness, "resolve_benchmark_python", resolve)
    admit = bench_harness.admit_benchmark_workload
    admitted_commands = []

    def capture_admission(
        script: Path,
        *,
        project_root: Path,
        run_args: Sequence[str],
        molt_args: Sequence[str],
        cpython_command: Sequence[str] | None,
    ) -> AbstractContextManager[bench_harness.AdmittedBenchmarkWorkload]:
        admitted_commands.append(cpython_command)
        return admit(
            script,
            project_root=project_root,
            run_args=run_args,
            molt_args=molt_args,
            cpython_command=cpython_command,
        )

    monkeypatch.setattr(bench_harness, "admit_benchmark_workload", capture_admission)
    monkeypatch.setattr(bench_harness, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(
        bench_harness.bench_suites,
        "molt_args_for_benchmark",
        lambda script: ["--capabilities", "fs.read"],
    )

    def fake_run_suite(
        suite_name,
        scripts,
        molt_cmd,
        python_cmd,
        timeout_s,
        parallel,
        colors,
        verbose,
        *,
        workloads,
        cpython: PythonInterpreter,
    ):
        assert list(workloads) == [script, second_script]
        for workload in workloads.values():
            assert workload.record["molt_args"] == [
                "run",
                "--profile",
                "release",
                "--python-version",
                "3.12",
                "--capabilities",
                "fs.read",
            ]
        assert python_cmd is None
        assert cpython is not None and cpython.command[1:] == ("-I", "-S", "-B")
        captured["molt_cmd"] = molt_cmd
        return [], bench_harness.SuiteSummary(suite=suite_name)

    monkeypatch.setattr(
        bench_harness,
        "collect_bench_scripts",
        lambda filter_pat=None, applications=False: [script, second_script],
    )
    monkeypatch.setattr(bench_harness, "run_suite", fake_run_suite)
    monkeypatch.setattr(bench_harness, "detect_regressions", lambda *args, **kwargs: [])
    monkeypatch.setattr(
        bench_harness, "print_summary_table", lambda *args, **kwargs: None
    )
    report_calls: list[dict[str, object]] = []

    def fake_build_json_report(*args, **kwargs):
        report_calls.append(kwargs)
        return {}

    class FakeSentinel:
        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, tb) -> None:
            return None

    sentinel_calls: list[dict[str, object]] = []

    def fake_repo_process_sentinel(**kwargs):
        sentinel_calls.append(kwargs)
        return FakeSentinel()

    monkeypatch.setattr(bench_harness, "build_json_report", fake_build_json_report)
    monkeypatch.setattr(
        bench_harness.harness_memory_guard,
        "repo_process_sentinel",
        fake_repo_process_sentinel,
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "bench/harness.py",
            "--bench",
            "--molt",
            ".venv/bin/molt",
            "--molt-profile",
            "release",
            "--output",
            str(tmp_path / "bench.json"),
            *([] if selector is None else ["--python", selector]),
        ],
    )

    with pytest.raises(SystemExit) as excinfo:
        bench_harness.main()

    assert excinfo.value.code == 0
    assert resolutions == [selector]
    assert admitted_commands == [cpython.command, cpython.command]
    assert captured["molt_cmd"] == [
        ".venv/bin/molt",
        "run",
        "--profile",
        "release",
        "--python-version",
        "3.12",
    ]
    assert sentinel_calls[0]["repo_root"] == bench_harness.REPO_ROOT
    harness_scratch = sentinel_calls[0]["artifact_root"]
    assert harness_scratch.parts[-2:] == ("bench", "harness")
    assert not harness_scratch.is_relative_to(bench_harness.REPO_ROOT.resolve())
    assert sentinel_calls[0]["label"] == "bench_harness"
    assert "memory_guard" in report_calls[0]


def test_bench_harness_uses_canonical_defaults() -> None:
    assert bench_harness.DEFAULT_OUTPUT == (
        bench_harness.BENCH_DIR / "results" / "harness.json"
    )
    assert bench_harness.DEFAULT_BASELINE == (
        bench_harness.BENCH_DIR / "results" / "harness-baseline.json"
    )


@pytest.mark.parametrize("mode", ["--bench", "--diff", "--full"])
def test_harness_modes_keep_reference_and_target_selection_distinct(
    mode: Literal["--bench", "--diff", "--full"],
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    cpython: PythonInterpreter,
) -> None:
    bench_script = tmp_path / "bench_script.py"
    bench_script.write_text("print(1)\n", encoding="utf-8")
    diff_script = tmp_path / "diff_script.py"
    diff_script.write_text("print(1)\n", encoding="utf-8")
    intrinsic = REPO_ROOT / "tests/benchmarks/bench_ptr_registry.py"
    commands: list[list[str]] = []
    resolutions = []

    def resolve(**kwargs: object) -> PythonInterpreter:
        assert mode == "--full"
        resolutions.append(kwargs["override"])
        return cpython

    def run(
        command: list[str], timeout: float, **kwargs: object
    ) -> bench_harness.RunOutcome:
        commands.append(command)
        return bench_harness.RunOutcome(
            True, 0.1, 1.0, "ok", 0, "100000\n" if mode == "--bench" else "1\n", ""
        )

    monkeypatch.setattr(bench_harness, "resolve_benchmark_python", resolve)
    monkeypatch.setattr(bench_harness, "run_cmd", run)
    monkeypatch.setattr(
        bench_harness,
        "collect_bench_scripts",
        lambda *args, **kwargs: [intrinsic if mode == "--bench" else bench_script],
    )
    monkeypatch.setattr(bench_harness, "collect_diff_scripts", lambda _: [diff_script])
    monkeypatch.setattr(
        bench_harness.harness_memory_guard,
        "repo_process_sentinel",
        lambda **_: nullcontext(),
    )
    output = tmp_path / "report.json"
    monkeypatch.setattr(
        sys,
        "argv",
        [
            str(HARNESS_PATH),
            mode,
            "--molt",
            "fixture-molt",
            "--output",
            str(output),
            "--baseline",
            str(tmp_path / "baseline.json"),
            *(["--python", cpython.version] if mode == "--full" else []),
        ],
    )

    with pytest.raises(SystemExit) as caught:
        bench_harness.main()

    assert caught.value.code == 0
    moltc = ["fixture-molt", "run", "--profile", "dev"]
    report = json.loads(output.read_text(encoding="utf-8"))
    if mode == "--bench":
        extra_args = bench_harness.bench_suites.molt_args_for_benchmark(intrinsic)
        assert commands == [[*moltc, *extra_args, str(intrinsic)]]
        (cell,) = report["benchmarks"].values()
        assert cell["cpython_oracle"] is None and cell["molt_cpython_ratio"] is None
    elif mode == "--diff":
        assert commands == [["python3", str(diff_script)], [*moltc, str(diff_script)]]
    else:
        assert commands == [
            [*cpython.command, str(bench_script)],
            [*moltc, "--python-version", cpython.major_minor, str(bench_script)],
            [cpython.executable, str(diff_script)],
            [*moltc, str(diff_script)],
        ]
    assert resolutions == ([cpython.version] if mode == "--full" else [])


@pytest.mark.parametrize("current_time", [0.01, 2.0])
def test_legacy_harness_refuses_unbound_timing_comparison_and_keeps_raw_measurement(
    tmp_path: Path, current_time: float
) -> None:
    baseline = tmp_path / "baseline.json"
    baseline.write_text(
        json.dumps(
            {
                "provenance": {"profile": "release-fast"},
                "results": {
                    "bench": {"sample.py": {"status": "pass", "molt_time_s": 1.0}}
                },
                "benchmarks": {"sample.py": {"molt_ok": True, "molt_time_s": 1.0}},
            }
        ),
        encoding="utf-8",
    )
    result = bench_harness.TestResult(
        name="sample.py",
        suite="bench",
        workload=synthetic_workload("sample.py"),
        status="pass",
        molt_time_s=current_time,
        cpython_time_s=1.0,
    )
    before = result.to_dict()
    findings = bench_harness.detect_regressions([result], baseline)
    assert len(findings) == 1
    assert findings[0]["type"] == "incompatible_timing"
    assert "selected build profiles unbound" in findings[0]["detail"]
    assert result.to_dict() == before

    report = bench_harness.build_json_report(
        [result], [], findings, molt_profile="release"
    )
    assert report["provenance"]["requested_profile"] == "release"
    assert report["provenance"]["authoritative"] is False
    cell = report["benchmarks"]["sample.py"]
    assert cell["molt_time_s"] == current_time
    assert cell["requested_profile"] == "release" and cell["build_observation"] is None


def test_legacy_harness_keeps_status_regression_independent_of_unbound_profiles(
    tmp_path: Path,
) -> None:
    baseline = tmp_path / "baseline.json"
    baseline.write_text(
        json.dumps(
            {
                "results": {
                    "bench": {"sample.py": {"status": "pass", "molt_time_s": 1.0}}
                },
                "benchmarks": {"old-name.py": {"status": "pass", "molt_time_s": 1.0}},
            }
        ),
        encoding="utf-8",
    )
    result = bench_harness.TestResult(
        name="sample.py",
        suite="bench",
        status="fail",
        workload=synthetic_workload("bench/sample.py"),
        molt_time_s=2.0,
    )
    assert bench_harness.detect_regressions([result], baseline) == [
        {
            "name": "sample.py",
            "type": "status",
            "detail": "was passing, now fail",
        }
    ]


def test_harness_fresh_cpython_comparison_retains_actual_source_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, cpython: PythonInterpreter
) -> None:
    from contextlib import ExitStack
    from tools.bench_evidence import admit_benchmark_workload
    from tools import bench_diff

    source = tmp_path / "program.py"
    source.write_text("print(1)\n", encoding="utf-8")
    commands = []

    def run(command, timeout, **kwargs):
        commands.append(command)
        return bench_harness.RunOutcome(True, 0.02, 1.0, "ok", 0, "1\n", "")

    monkeypatch.setattr(bench_harness, "run_cmd", run)
    with ExitStack() as custody:
        admitted = custody.enter_context(
            admit_benchmark_workload(
                source,
                project_root=tmp_path,
                run_args=[],
                molt_args=["run", "--profile", "dev"],
                cpython_command=cpython.command,
            )
        )
        result = bench_harness.run_single_test(
            source,
            "bench",
            ["molt", "run", "--profile", "dev"],
            None,
            1,
            workload=admitted,
            cpython=cpython,
        )
        report = bench_harness.build_json_report([result], [], [])
    assert commands == [
        [*cpython.command, str(source)],
        ["molt", "run", "--profile", "dev", str(source)],
    ]
    cells = report["benchmarks"]
    assert cells[str(source)]["workload"] == admitted.record
    assert cells[str(source)]["cpython_ok"] is True
    assert cells[str(source)]["cpython_oracle"]["cmd"] == list(cpython.command)
    rows = bench_diff._compute_metric_diffs("cpython_time_s", cells, cells)
    assert len(rows) == 1 and rows[0].delta == 0
    assert cells[str(source)]["build_observation"] is None


@pytest.mark.parametrize(
    "molt_args,has_oracle,diagnostic",
    [
        (
            ["run", "--profile", "release"],
            True,
            "command differs from admitted Molt arguments",
        ),
        (
            ["run", "--profile", "dev"],
            False,
            "requires an admitted CPython oracle",
        ),
    ],
)
def test_harness_refuses_admitted_arguments_it_will_not_execute(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    molt_args: list[str],
    has_oracle: bool,
    diagnostic: str,
    cpython: PythonInterpreter,
) -> None:
    from tools.bench_evidence import admit_benchmark_workload

    script = tmp_path / "program.py"
    script.write_text("print(1)\n", encoding="utf-8")
    monkeypatch.setattr(
        bench_harness,
        "run_cmd",
        lambda *a, **k: pytest.fail("unexecuted admitted argv reached child"),
    )
    with admit_benchmark_workload(
        script, project_root=tmp_path, run_args=[], molt_args=molt_args
    ) as admitted:
        with pytest.raises(ValueError, match=diagnostic):
            bench_harness.run_single_test(
                script,
                "bench",
                ["molt", "run", "--profile", "dev"],
                None if has_oracle else "python3",
                1.0,
                workload=admitted,
                cpython=cpython if has_oracle else None,
            )


def test_harness_executes_admitted_guest_arguments(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, cpython: PythonInterpreter
) -> None:
    from tools.bench_evidence import admit_benchmark_workload

    script = tmp_path / "program.py"
    script.write_text("print(1)\n", encoding="utf-8")
    calls = []

    def run(command, timeout, **kwargs):
        calls.append((command, kwargs))
        return bench_harness.RunOutcome(True, 0.1, 1.0, "ok", 0, "1\n", "")

    monkeypatch.setattr(bench_harness, "run_cmd", run)
    with admit_benchmark_workload(
        script,
        project_root=tmp_path,
        run_args=["--count", "7"],
        molt_args=["run", "--profile", "dev"],
    ) as admitted:
        result = bench_harness.run_single_test(
            script,
            "bench",
            ["molt", "run", "--profile", "dev"],
            None,
            1,
            workload=admitted,
            cpython=cpython,
        )
    assert result.status == "pass"
    assert calls[0][0] == [*cpython.command, str(script), "--count", "7"]
    assert calls[1][0] == [
        "molt",
        "run",
        "--profile",
        "dev",
        str(script),
        "--",
        "--count",
        "7",
    ]


def test_harness_sealed_invocations_use_payload_cwd_and_import_roots(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, cpython: PythonInterpreter
) -> None:
    from tests.tools.perf_scoreboard_fixtures import sealed_workload

    calls = []

    def run(command, timeout, **kwargs):
        calls.append((command, kwargs))
        return bench_harness.RunOutcome(True, 0.1, 1.0, "ok", 0, "sealed:7\n", "")

    monkeypatch.setattr(bench_harness, "run_cmd", run)
    monkeypatch.setattr(
        bench_harness.bench_suites,
        "molt_args_for_benchmark",
        lambda script: ["--capabilities", "fs.read"],
    )
    with sealed_workload(
        tmp_path,
        molt_args=["run", "--profile", "dev", "--capabilities", "fs.read"],
    ) as workload:
        result = bench_harness.run_single_test(
            workload.script,
            "bench",
            ["molt", "run", "--profile", "dev"],
            None,
            1,
            workload=workload,
            cpython=cpython,
        )
        assert result.status == "pass"
        assert result.workload["entry"] == str(workload.script)
        assert calls[0][0][:5] == [*cpython.command, "-c"]
        assert calls[0][0][6:] == [
            str(workload.execution_script),
            "1",
            str(workload.execution_root / "packages"),
            "data/input.txt",
            "7",
        ]
        assert calls[1][0] == [
            "molt",
            "run",
            "--profile",
            "dev",
            "--capabilities",
            "fs.read",
            str(workload.execution_script),
            "--",
            "data/input.txt",
            "7",
        ]
        assert all(kwargs["cwd"] == workload.execution_root for _, kwargs in calls)
        assert "PYTHONPATH" not in calls[0][1]["env"]
        assert "PYTHONHASHSEED" not in calls[0][1]["env"]
        assert calls[1][1]["env"]["MOLT_HERMETIC_MODULE_ROOTS"] == "1"
        assert calls[1][1]["env"]["MOLT_MODULE_ROOTS"] == str(
            workload.execution_root / "packages"
        )


@pytest.mark.parametrize(
    "stdout,stderr,returncode",
    [("sealed:7", "", 0), ("sealed:7\n", "unexpected\n", 0), ("sealed:7\n", "", 2)],
)
def test_benchmark_harness_never_scores_incomplete_output(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    stdout: str,
    stderr: str,
    returncode: int,
    cpython: PythonInterpreter,
) -> None:
    from tests.tools.perf_scoreboard_fixtures import sealed_workload

    observations = iter(
        [
            bench_harness.RunOutcome(True, 0.2, 1.0, "ok", 0, "sealed:7\n", ""),
            bench_harness.RunOutcome(
                returncode == 0,
                0.1,
                1.0,
                "ok" if returncode == 0 else "nonzero",
                returncode,
                stdout,
                stderr,
            ),
        ]
    )
    monkeypatch.setattr(bench_harness, "run_cmd", lambda *a, **kw: next(observations))
    with sealed_workload(tmp_path, molt_args=["run"]) as workload:
        result = bench_harness.run_single_test(
            workload.script,
            "bench",
            ["molt", "run"],
            None,
            1,
            workload=workload,
            cpython=cpython,
        )
    assert not result.output_match
    assert result.status in {"fail", "error"}
    assert result.molt_speedup is None
    assert result.molt_stderr == stderr


@pytest.mark.parametrize("descriptor", [1, 2])
def test_benchmark_harness_actual_guard_refuses_distinct_invalid_output_bytes(
    tmp_path, monkeypatch, descriptor
):
    from tools.bench_evidence import admit_benchmark_workload

    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifacts"))
    script = tmp_path / "raw_output.py"
    script.write_text(
        f"import os; os.write({descriptor}, b'raw\\xff\\r\\nno-final-newline')\n",
        encoding="utf-8",
    )
    # A real child replaces only the compiler in this observation test. Neither
    # run_cmd nor the shared process guard is mocked.
    guest_code = f"import os; os.write({descriptor}, b'raw\\xfe\\r\\nno-final-newline')"
    cpython = benchmark_oracle_interpreter(Path(sys.executable).resolve())
    with admit_benchmark_workload(
        script, project_root=tmp_path, run_args=[], molt_args=["-c", guest_code]
    ) as workload:
        result = bench_harness.run_single_test(
            script,
            "bench",
            [sys.executable, "-c", guest_code],
            None,
            15,
            workload=workload,
            cpython=cpython,
        )
    assert result.cpython_rc == result.molt_rc == 0
    reference = result.cpython_stdout if descriptor == 1 else result.cpython_stderr
    guest = result.molt_stdout if descriptor == 1 else result.molt_stderr
    assert reference.encode("utf-8", "surrogateescape") == (
        b"raw\xff\r\nno-final-newline"
    )
    assert guest.encode("utf-8", "surrogateescape") == (b"raw\xfe\r\nno-final-newline")
    assert result.status == "fail"
    assert result.output_match is False
    assert result.molt_speedup is None


@pytest.mark.parametrize("failed_side", ["reference", "guest"])
@pytest.mark.parametrize("condition", ["unclosed", "clock", "timeout", "nonzero"])
def test_harness_preserves_failed_observation_without_scoring(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    failed_side: Literal["reference", "guest"],
    condition: Literal["unclosed", "clock", "timeout", "nonzero"],
    cpython: PythonInterpreter,
) -> None:
    from tools.bench_evidence import admit_benchmark_workload

    script = tmp_path / "observation.py"
    script.write_text("print('ok')\n", encoding="utf-8")
    raw_out = b"out\xff\r\n".decode("utf-8", "surrogateescape")
    raw_err = b"err\xfe\r\n".decode("utf-8", "surrogateescape")
    calls = 0

    def guarded(command, **kwargs):
        nonlocal calls
        calls += 1
        failed = calls == (1 if failed_side == "reference" else 2)
        code = (
            124
            if failed and condition == "timeout"
            else 7
            if failed and condition == "nonzero"
            else 0
        )
        return bench_harness.harness_memory_guard.GuardedCompletedProcess(
            command,
            code,
            raw_out if failed else "ok\n",
            "guard diagnostics",
            elapsed_s=9.0,
            child_elapsed_s=None if failed and condition == "clock" else 0.025,
            child_returncode=code,
            child_stderr=raw_err if failed else "",
            descendants_closed=not (failed and condition == "unclosed"),
            timed_out=failed and condition == "timeout",
            peak=bench_harness.harness_memory_guard.memory_guard.RssViolation(
                pid=1, rss_kb=1024, command="fixture", scope="process"
            ),
        )

    monkeypatch.setattr(
        bench_harness.harness_memory_guard, "guarded_completed_process", guarded
    )
    with admit_benchmark_workload(
        script, project_root=tmp_path, run_args=[], molt_args=["run"]
    ) as workload:
        result = bench_harness.run_single_test(
            script,
            "bench",
            ["molt", "run"],
            None,
            1,
            workload=workload,
            cpython=cpython,
        )
    assert result.status != "pass"
    assert result.molt_speedup is None
    prefix = "cpython" if failed_side == "reference" else "molt"
    serialized = result.to_dict()
    assert serialized["cpython_ok"] is (failed_side != "reference")
    assert serialized[f"{prefix}_stdout"] == raw_out
    assert serialized[f"{prefix}_stderr"] == raw_err
    assert getattr(result, f"{prefix}_time_s") == (
        None if condition == "clock" else 0.025
    )
    assert calls == (1 if failed_side == "reference" else 2)


@pytest.mark.parametrize("reference_failure", [False, True])
def test_harness_preserves_legacy_diff_policy(tmp_path, monkeypatch, reference_failure):
    script = tmp_path / "diff.py"
    script.write_text("print('same')\n", encoding="utf-8")
    observations = iter(
        [
            bench_harness.RunOutcome(
                not reference_failure,
                0.1,
                1.0,
                "nonzero" if reference_failure else "ok",
                7 if reference_failure else 0,
                "same\n",
                "reference stderr",
            ),
            bench_harness.RunOutcome(True, 0.1, 1.0, "ok", 0, "same\n", "guest stderr"),
        ]
    )
    monkeypatch.setattr(bench_harness, "run_cmd", lambda *a, **kw: next(observations))
    result = bench_harness.run_single_test(
        script, "diff", ["molt", "run"], "python", 1, workload=None
    )
    assert result.status == ("skip" if reference_failure else "pass")
    assert result.molt_speedup is None
