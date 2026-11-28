from __future__ import annotations

from collections import Counter
from collections.abc import Mapping
from copy import deepcopy
from functools import cache
from pathlib import Path
import tomllib
import io
import json
import sys
from typing import Literal, NoReturn, cast
import zipfile

import pytest

import tools.bench_suites as bench_suites
from tools import bench, bench_individual, bench_wasm, benchmark_applications
from tools import profile as profile_tool


ROOT = Path(__file__).resolve().parents[1]


def test_benchmark_runner_tables_use_canonical_suite() -> None:
    assert tuple(bench.BENCHMARKS) == bench_suites.BENCHMARKS
    assert tuple(bench_individual.BENCHMARKS) == bench_suites.BENCHMARKS
    assert tuple(bench_wasm.BENCHMARKS) == bench_suites.BENCHMARKS
    assert "tests/benchmarks/bench_import_time.py" in bench_individual.BENCHMARKS
    assert "tests/benchmarks/bench_procedural_gen.py" in bench_individual.BENCHMARKS

    assert tuple(bench.SMOKE_BENCHMARKS) == bench_suites.SMOKE_BENCHMARKS
    assert tuple(bench_wasm.SMOKE_BENCHMARKS) == bench_suites.SMOKE_BENCHMARKS
    assert tuple(bench.WS_BENCHMARKS) == bench_suites.WS_BENCHMARKS
    assert tuple(bench_wasm.WS_BENCHMARKS) == bench_suites.WS_BENCHMARKS
    assert tuple(bench.DYNAMIC_BUILTIN_SLICES) == bench_suites.DYNAMIC_BUILTIN_SLICES


def test_every_benchmark_file_has_exactly_one_primary_owner_or_exclusion() -> None:
    allowed_exclusion_kinds = {
        "self_timed",
        "profile_epoch",
        "diagnostic",
        "external_harness",
    }
    primary_suites = {
        "core": bench_suites.BENCHMARKS,
        "websocket": bench_suites.WS_BENCHMARKS,
        "dynamic_builtin": bench_suites.DYNAMIC_BUILTIN_SLICES,
    }
    primary_paths = [path for suite in primary_suites.values() for path in suite]
    counts = Counter(primary_paths)
    assert all(count == 1 for count in counts.values())
    for suite in primary_suites.values():
        assert len(suite) == len(set(suite))

    discovered = {
        path.relative_to(ROOT).as_posix()
        for path in (ROOT / "tests" / "benchmarks").glob("bench_*.py")
    }
    runnable = set(primary_paths)
    excluded = set(bench_suites.EXCLUDED_BENCHMARKS)
    assert runnable.isdisjoint(excluded)
    assert discovered == runnable | excluded

    # Smoke is deliberately an alias/subset, not another ownership class.
    assert set(bench_suites.SMOKE_BENCHMARKS) <= set(bench_suites.BENCHMARKS)
    for path, exclusion in bench_suites.EXCLUDED_BENCHMARKS.items():
        assert (ROOT / path).is_file()
        assert exclusion.kind in allowed_exclusion_kinds
        assert exclusion.reason.strip()


@pytest.fixture
def inventory_project(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    declarations = {
        "BENCHMARKS": ("tests/benchmarks/bench_selected.py",),
        "SMOKE_BENCHMARKS": ("tests/benchmarks/bench_selected.py",),
        "WS_BENCHMARKS": ("tests/benchmarks/bench_ws.py",),
        "DYNAMIC_BUILTIN_SLICES": ("tests/benchmarks/bench_slice.py",),
    }
    for name, paths in declarations.items():
        monkeypatch.setattr(bench_suites, name, paths)
    excluded = "tests/benchmarks/bench_diag.py"
    monkeypatch.setattr(
        bench_suites,
        "EXCLUDED_BENCHMARKS",
        {excluded: bench_suites.BenchmarkExclusion("diagnostic", "epoch-only proof")},
    )
    monkeypatch.setattr(bench_suites, "_applications", lambda: ())
    for path in {
        excluded,
        *(path for paths in declarations.values() for path in paths),
    }:
        source = tmp_path / path
        source.parent.mkdir(parents=True, exist_ok=True)
        source.write_text(
            "raise RuntimeError('inventory must not execute sources')\n",
            encoding="utf-8",
        )
    return tmp_path


def test_inventory_keeps_primary_alias_and_exclusion_denominators_separate(
    inventory_project: Path,
) -> None:
    inventory = bench_suites.benchmark_inventory(root=inventory_project)
    assert inventory.as_record() == {
        "primary_suites": {
            "core": ["tests/benchmarks/bench_selected.py"],
            "websocket": ["tests/benchmarks/bench_ws.py"],
            "dynamic_builtin": ["tests/benchmarks/bench_slice.py"],
        },
        "smoke": ["tests/benchmarks/bench_selected.py"],
        "applications": [],
        "exclusions": {
            "tests/benchmarks/bench_diag.py": {
                "kind": "diagnostic",
                "reason": "epoch-only proof",
            }
        },
    }
    assert bench_suites.benchmark_suite_keys("core", inventory=inventory) == (
        "tests/benchmarks/bench_selected.py",
    )
    assert (
        bench_suites.benchmark_suite_keys("smoke", inventory=inventory)
        == inventory.smoke
    )
    inventory.as_record()["primary_suites"]["core"].append("changed")
    assert bench_suites.benchmark_suite_keys("core", inventory=inventory) == (
        "tests/benchmarks/bench_selected.py",
    )
    with pytest.raises(ValueError, match="unknown benchmark set"):
        bench_suites.benchmark_set("unknown")


@pytest.mark.parametrize("drift", ["new", "missing", "duplicate", "excluded", "smoke"])
def test_inventory_refuses_drift_instead_of_silently_selecting_it(
    inventory_project: Path,
    monkeypatch: pytest.MonkeyPatch,
    drift: Literal["new", "missing", "duplicate", "excluded", "smoke"],
) -> None:
    if drift == "new":
        (inventory_project / "tests/benchmarks/bench_new.py").write_text(
            "pass\n", encoding="utf-8"
        )
    elif drift == "missing":
        (inventory_project / "tests/benchmarks/bench_selected.py").unlink()
    elif drift == "duplicate":
        monkeypatch.setattr(bench_suites, "WS_BENCHMARKS", bench_suites.BENCHMARKS)
    elif drift == "excluded":
        monkeypatch.setattr(
            bench_suites,
            "EXCLUDED_BENCHMARKS",
            {
                bench_suites.BENCHMARKS[0]: bench_suites.BenchmarkExclusion(
                    "diagnostic", "double ownership"
                )
            },
        )
    else:
        monkeypatch.setattr(
            bench_suites, "SMOKE_BENCHMARKS", bench_suites.WS_BENCHMARKS
        )
    with pytest.raises(ValueError):
        bench_suites.benchmark_inventory(root=inventory_project)


def test_inventory_application_selection_is_derived_from_catalog_without_acquisition(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def unexpected_acquisition(*args: object, **kwargs: object) -> None:
        pytest.fail("inventory acquired application inputs")

    monkeypatch.setattr(
        benchmark_applications, "prepare_application", unexpected_acquisition
    )
    monkeypatch.setattr(benchmark_applications, "_archive", unexpected_acquisition)
    inventory = bench_suites.benchmark_inventory()
    assert inventory.applications == tuple(
        app.driver for app in benchmark_applications.applications()
    )
    assert bench_suites.benchmark_suite_keys("applications", inventory=inventory) == (
        inventory.applications
    )
    assert set(inventory.applications).isdisjoint(
        path for _, paths in inventory.primary_suites for path in paths
    )


@pytest.mark.parametrize("selection", ["core", "applications", "shard", "explicit"])
def test_scoreboard_plan_is_exact_and_precedes_workload_side_effects(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    selection: Literal["core", "applications", "shard", "explicit"],
) -> None:
    from tools import bench_evidence, perf_scoreboard, perf_scoreboard_cli

    def unexpected_work(*args: object, **kwargs: object) -> None:
        pytest.fail("metadata-only plan crossed the measurement boundary")

    for module, name in (
        (perf_scoreboard, "_resolve_benchmark_cpython"),
        (perf_scoreboard, "wait_for_quiescence"),
        (perf_scoreboard_cli, "_observe_cpython_runtime"),
        (bench_evidence, "admit_benchmark_workload"),
        (bench, "admit_wasm_execution"),
        (bench, "_BenchBatchBuildServer"),
        (benchmark_applications, "prepare_application"),
        (benchmark_applications, "_archive"),
    ):
        monkeypatch.setattr(module, name, unexpected_work)
    output = tmp_path / "must-not-exist" / "board.json"
    monkeypatch.setattr(perf_scoreboard, "SCOREBOARD_DIR", output.parent)
    argv = [
        "--plan",
        "--plan-format",
        "json",
        "--backend",
        "native",
        "--backend",
        "wasm",
        "--profile",
        "release-fast",
        "--out",
        str(output),
    ]
    if selection == "applications":
        argv += ["--set", "applications"]
        selected = set(bench_suites.APPLICATION_BENCHMARKS)
    elif selection == "explicit":
        argv += ["--benchmark", "bench_sum.py"]
        selected = {"tests/benchmarks/bench_sum.py"}
    elif selection == "shard":
        argv += ["--set", "smoke", "--shard", "0/2"]
        selected = {"tests/benchmarks/bench_bytes_find.py"}
    else:
        selected = set(bench_suites.BENCHMARKS)
    assert perf_scoreboard.main(argv) == 0
    plan = cast(
        perf_scoreboard_cli.BenchmarkPlanRecord, json.loads(capsys.readouterr().out)
    )
    assert plan["kind"] == "molt.benchmark-plan.v1"
    assert {
        (cell["benchmark"], cell["target"], cell["backend"], cell["profile"])
        for cell in plan["planned_cells"]
    } == {
        (key, target, backend, "release-fast")
        for key in selected
        for target, backend in (("native", "native"), ("wasm", "wasm"))
    }
    assert len(plan["planned_cells"]) == 2 * len(selected)
    if selection == "shard":
        assert plan["shard"] is not None
        assert plan["shard"]["members"] == sorted(selected)
        assert plan["shard"]["set_members"] == sorted(bench_suites.SMOKE_BENCHMARKS)
    else:
        assert plan["shard"] is None
    assert plan["inventory"] == bench_suites.benchmark_inventory().as_record()
    assert not output.parent.exists()


def test_scoreboard_plan_summary_lists_each_benchmark_once(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    from tools import perf_scoreboard

    def unexpected_work(*args: object, **kwargs: object) -> None:
        pytest.fail("plan summary crossed the measurement boundary")

    monkeypatch.setattr(perf_scoreboard, "_resolve_benchmark_cpython", unexpected_work)
    assert (
        perf_scoreboard.main(
            ["--plan", "--set", "smoke", "--backend", "native", "--backend", "llvm"]
        )
        == 0
    )
    output = capsys.readouterr().out
    assert "planning only; no measurement" in output
    assert "set=smoke shard=all benchmarks=2 coordinates=4" in output
    assert "backends=native,llvm profiles=release-fast" in output
    assert f"core={len(bench_suites.BENCHMARKS)}" in output
    assert f"applications={len(bench_suites.APPLICATION_BENCHMARKS)}" in output
    assert f"exclusions={len(bench_suites.EXCLUDED_BENCHMARKS)}" in output
    for path in bench_suites.SMOKE_BENCHMARKS:
        assert output.splitlines().count(path) == 1
    assert output.split("selected benchmark keys:\n")[1].splitlines() == list(
        bench_suites.SMOKE_BENCHMARKS
    )


@pytest.mark.parametrize(
    "args",
    [
        ["--backend", "native", "--backend", "native"],
        ["--profile", "release-fast", "--profile", "release-fast"],
        ["--benchmark", "bench_sum.py", "--benchmark", "bench_sum.py"],
        ["--benchmark", "missing-benchmark.py"],
        ["--benchmark", "tests/benchmarks"],
        ["--benchmark", ""],
        ["--merge", "board.json"],
        ["--rebuild-summary", "board.json"],
        ["--sample-hot-only"],
        ["--self-test"],
        ["--plan-format", "unknown"],
    ],
)
def test_scoreboard_plan_refuses_ambiguous_or_incompatible_selection(
    monkeypatch: pytest.MonkeyPatch, args: list[str]
) -> None:
    from tools import perf_scoreboard

    def unexpected_work(*args: object, **kwargs: object) -> None:
        pytest.fail("invalid plan crossed the measurement boundary")

    monkeypatch.setattr(perf_scoreboard, "_resolve_benchmark_cpython", unexpected_work)
    with pytest.raises(SystemExit):
        perf_scoreboard.main(["--plan", *args])


def test_scoreboard_plan_format_requires_plan() -> None:
    from tools import perf_scoreboard

    with pytest.raises(SystemExit, match="2"):
        perf_scoreboard.main(["--plan-format", "json"])


@pytest.fixture
def retained_signals_board(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    from tools import perf_schema, perf_scoreboard
    from tests.tools.test_perf_scoreboard import _board, _cell, _signal_failure

    cells = [
        _cell(benchmark=benchmark, build_ok=False)
        for benchmark in bench_suites.BENCHMARKS[:10]
    ]
    for cell in cells:
        _signal_failure(cell, phase="build", status="build_failed")

    def fixture_revision() -> str:
        return "a" * 40

    with monkeypatch.context() as fixture_identity:
        fixture_identity.setattr(perf_scoreboard, "_git_rev", fixture_revision)
        document = _board(cells)
    assert perf_schema.validate_board(document) == []
    evidence = tmp_path / "retained.json"
    evidence.write_text(json.dumps(document), encoding="utf-8")
    return evidence


@pytest.mark.parametrize(
    "output_format,signal_limit", [("summary", None), ("json", None), ("json", 1)]
)
def test_scoreboard_signals_reads_real_evidence_before_all_workload_side_effects(
    retained_signals_board: Path,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    output_format: Literal["summary", "json"],
    signal_limit: int | None,
) -> None:
    from tools import bench_evidence, perf_scoreboard, perf_scoreboard_cli

    def unexpected_work(*args: object, **kwargs: object) -> None:
        pytest.fail("read-only signals crossed the measurement or publication boundary")

    original = retained_signals_board.read_bytes()
    output_directory = tmp_path / "must-not-exist"
    argv = ["--signals", str(retained_signals_board), "--expected-source", "a" * 40]
    if output_format == "json":
        argv += ["--signals-format=json"]
    if signal_limit is not None:
        argv += [f"--signal-limit={signal_limit}"]
    with monkeypatch.context() as boundary:
        for module, name in (
            (bench_suites, "benchmark_inventory"),
            (perf_scoreboard, "_resolve_benchmark_set"),
            (perf_scoreboard, "_resolve_benchmark_cpython"),
            (perf_scoreboard, "_git_rev"),
            (perf_scoreboard, "wait_for_quiescence"),
            (perf_scoreboard, "gather_provenance"),
            (perf_scoreboard, "measure_cell"),
            (perf_scoreboard, "_write_scoreboard_doc"),
            (perf_scoreboard_cli, "_observe_cpython_runtime"),
            (bench_evidence, "admit_benchmark_workload"),
            (bench, "admit_wasm_execution"),
            (bench, "_BenchBatchBuildServer"),
            (benchmark_applications, "prepare_application"),
            (benchmark_applications, "_archive"),
            (Path, "mkdir"),
        ):
            boundary.setattr(module, name, unexpected_work)
        boundary.setattr(perf_scoreboard, "SCOREBOARD_DIR", output_directory)
        assert perf_scoreboard.main(argv) == 0
    output = capsys.readouterr().out
    if output_format == "json":
        payload = cast(Mapping[str, object], json.loads(output))
        snapshot = cast(Mapping[str, object], payload["snapshot"])
        assert snapshot["evidence_path"] == str(retained_signals_board)
        assert snapshot["scope"] == "retained_snapshot"
        assert snapshot["current_main_proof"] is False
        assert snapshot["methodology_sha256"] == "d" * 64
        signals = cast(list[object], payload["signals"])
        assert len(signals) == (signal_limit if signal_limit is not None else 8)
        assert cast(int, payload["total"]) >= 10
        assert payload["omitted"] == cast(int, payload["total"]) - len(signals)
    else:
        assert str(retained_signals_board) in output
        assert "a" * 40 in output
        assert "d" * 64 in output
    assert retained_signals_board.read_bytes() == original
    assert not output_directory.exists()


@pytest.mark.parametrize(
    "args",
    [
        ["--plan"],
        ["--plan-format", "summary"],
        ["--merge", "other.json"],
        ["--rebuild-summary", "other.json"],
        ["--self-test"],
        ["--sample-hot-only"],
        ["--set=core"],
        ["--benchmark", "bench_fib.py"],
        ["--backend", "native"],
        ["--profile", "release-fast"],
        ["--cpython", "python"],
        ["--samples", "5"],
        ["--warmup", "2"],
        ["--rss-mb", "1024"],
        ["--timeout", "30"],
        ["--build-timeout", "30"],
        ["--build-timeout=30"],
        ["--require-quiescent"],
        ["--quiescence-wait-s", "0"],
        ["--quiescence-poll-s", "15"],
        ["--print-provenance"],
        ["--repeat", "1"],
        ["--classify"],
        ["--emit-cycle-profile"],
        ["--inner-repeat", "10"],
        ["--profile-build"],
        ["--shard", "0/1"],
        ["--out", "must-not-exist/board.json"],
        ["--baseline"],
        ["--no-gate"],
        ["--emit-wasm-phases"],
        ["--strict-cold"],
        ["--allow-nonauthoritative"],
        ["--pypy"],
        ["--codon"],
    ],
)
def test_scoreboard_signals_rejects_every_ignored_mode_or_measurement_option(
    args: list[str], capsys: pytest.CaptureFixture[str]
) -> None:
    from tools import perf_scoreboard

    # The absent path also proves conflict validation precedes evidence loading.
    with pytest.raises(SystemExit, match="2"):
        perf_scoreboard.main(["--signals", "absent.json", *args])
    assert "--signals cannot be combined with" in capsys.readouterr().err


@pytest.mark.parametrize(
    "timeout", ["0", "-1", "nan", "inf", "-inf", "invalid", "1e100"]
)
def test_scoreboard_rejects_invalid_build_timeout(
    timeout: str, capsys: pytest.CaptureFixture[str]
) -> None:
    from tools import perf_scoreboard

    with pytest.raises(SystemExit, match="2"):
        perf_scoreboard.main([f"--build-timeout={timeout}"])
    assert "--build-timeout: build timeout must be positive and finite" in (
        capsys.readouterr().err
    )


@pytest.mark.parametrize(
    "mode_args",
    [["--plan"], ["--merge", "absent.json"], ["--rebuild-summary", "absent.json"]],
)
def test_scoreboard_rejects_build_timeout_in_modes_without_builds(
    mode_args: list[str], capsys: pytest.CaptureFixture[str]
) -> None:
    from tools import perf_scoreboard

    with pytest.raises(SystemExit, match="2"):
        perf_scoreboard.main([*mode_args, "--build-timeout=600"])
    assert f"--build-timeout cannot be combined with {mode_args[0]}" in (
        capsys.readouterr().err
    )


@pytest.mark.parametrize("build_timeout", [None, 0.125])
@pytest.mark.parametrize("hot_only", [False, True])
def test_scoreboard_forwards_distinct_build_and_execution_timeouts(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    build_timeout: float | None,
    hot_only: bool,
) -> None:
    from molt.temporary_artifacts import OwnedTemporaryDirectory
    from tests.tools.perf_scoreboard_fixtures import write_synthetic_build_observation
    from tools import perf_scoreboard, perf_scoreboard_cli
    from tools import perf_scoreboard_measure as measure
    from tools import perf_scoreboard_profile as profiling
    from tools.safe_run import CapturedStreamIdentity

    class ReachedExecutionBoundary(Exception):
        pass

    builds: list[float] = []
    argv = ["--benchmark", "bench_fib.py", "--backend", "native"]
    if hot_only:
        argv.append("--sample-hot-only")
    else:
        argv.extend(["--timeout", "31.75"])
    if build_timeout is not None:
        argv.extend(["--build-timeout", str(build_timeout)])

    def prepare(
        script: str, *, build_timeout_s: float, **_kwargs: object
    ) -> bench.MoltBinary | bench.MoltFailure:
        builds.append(build_timeout_s)
        if hot_only:
            assert "for _ in range(" in Path(script).read_text(encoding="utf-8")
            return bench.classify_molt_process_failure(
                phase="build",
                returncode=bench.memory_guard.GUARD_RETURN_CODE,
                elapsed_s=build_timeout_s,
                stderr="backend daemon returned empty response",
                violation=bench.memory_guard.RssViolation(
                    123, 8192, "fixture compiler", "process"
                ),
                stderr_identity=CapturedStreamIdentity.from_capture(
                    b"backend daemon returned empty response"
                ),
            )
        owner = OwnedTemporaryDirectory(dir=tmp_path)
        artifact = Path(owner.name) / "bench_fib"
        observation = write_synthetic_build_observation(
            artifact, backend="native", profile="release-fast"
        )
        return bench.MoltBinary(artifact, owner, 0.1, 1.0, observation)

    def run_guest(
        _cmd: list[str], *, timeout_s: float, label: str, **_kwargs: object
    ) -> NoReturn:
        assert label == "molt-cold:tests/benchmarks/bench_fib.py"
        assert timeout_s == 31.75
        raise ReachedExecutionBoundary

    def checkpoint(
        *_args: object, provenance: Mapping[str, object], **_kwargs: object
    ) -> None:
        invocation = cast(list[dict[str, object]], provenance["producer_invocations"])[
            0
        ]
        assert cast(list[str], invocation["argv"])[2:] == argv

    from tests.tools.perf_scoreboard_fixtures import benchmark_oracle_interpreter

    oracle = perf_scoreboard.CpythonOracle(
        benchmark_oracle_interpreter(sys.executable, "3.12.15")
    )
    monkeypatch.setattr(perf_scoreboard, "_resolve_benchmark_cpython", lambda _: oracle)
    monkeypatch.setattr(
        perf_scoreboard,
        "wait_for_quiescence",
        lambda **_: {"quiet": True, "loadavg_1m": 0, "ncpu": 1, "runnable_signal": 0},
    )
    monkeypatch.setattr(
        perf_scoreboard, "gather_provenance", lambda *_, **__: {"authoritative": True}
    )
    monkeypatch.setattr(perf_scoreboard, "_git_rev", lambda: "a" * 40)
    monkeypatch.setattr(perf_scoreboard, "_checkpoint", checkpoint)
    monkeypatch.setattr(perf_scoreboard, "SCOREBOARD_DIR", tmp_path)
    monkeypatch.setattr(perf_scoreboard_cli, "_observe_cpython_runtime", lambda _: {})
    monkeypatch.setattr(
        perf_scoreboard_cli, "capture_current_python_runtime", lambda: {}
    )
    monkeypatch.setattr(bench, "_BenchBatchBuildServer", lambda _: None)
    monkeypatch.setattr(bench, "prepare_molt_binary", prepare)
    monkeypatch.setattr(measure, "REPO_ROOT", tmp_path)
    monkeypatch.setattr(measure, "_run_measured_command", run_guest)
    monkeypatch.setattr(profiling, "SCOREBOARD_DIR", tmp_path)
    monkeypatch.setattr(profiling, "_profiling_tmp_root", lambda _: tmp_path)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path))

    expected = 600.0 if build_timeout is None else build_timeout
    if hot_only:
        out = tmp_path / "profile.json"
        assert perf_scoreboard.main([*argv, "--out", str(out)]) == 1
        board = json.loads(out.read_text(encoding="utf-8"))
        assert board["cells"][0]["build_timeout_s"] == expected
        build = board["cells"][0]["build"]
        assert build["reason"] == "profiling build failed: rss_limit_exceeded"
        failure = build["molt_failure"]
        assert failure["detail"] == "backend_daemon_empty_response"
        assert failure["elapsed_s"] == expected
        assert failure["guard_violation"]["rss_kb"] == 8192
        assert failure["stderr_identity"]["byte_length"] == len(
            b"backend daemon returned empty response"
        )
    else:
        with pytest.raises(ReachedExecutionBoundary):
            perf_scoreboard.main(argv)
    assert builds == [expected]


@pytest.mark.parametrize(
    "args",
    [
        ["--signals-format", "summary"],
        ["--signal-limit=8"],
        ["--expected-source", "a" * 40],
    ],
)
def test_scoreboard_signal_options_require_signals(
    args: list[str], capsys: pytest.CaptureFixture[str]
) -> None:
    from tools import perf_scoreboard

    with pytest.raises(SystemExit, match="2"):
        perf_scoreboard.main(args)
    assert "requires --signals" in capsys.readouterr().err


@pytest.mark.parametrize(
    "args",
    [
        ["--signals-format", "yaml"],
        ["--signal-limit", "0"],
        ["--signal-limit", "-1"],
        ["--signal-limit", "1.5"],
        ["--expected-source", "main"],
        ["--expected-source", "a" * 39],
        ["--expected-source", "A" * 40],
    ],
)
def test_scoreboard_signals_refuses_invalid_options(args: list[str]) -> None:
    from tools import perf_scoreboard

    with pytest.raises(SystemExit, match="2"):
        perf_scoreboard.main(["--signals", "absent.json", *args])


@pytest.mark.parametrize("contents", [None, b"{", b"[]", b"null", b"{}", b"\xff"])
def test_scoreboard_signals_reports_input_errors_as_argparse_diagnostics(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    contents: bytes | None,
) -> None:
    from tools import perf_scoreboard

    evidence = tmp_path / "invalid.json"
    if contents is not None:
        evidence.write_bytes(contents)
    with pytest.raises(SystemExit, match="2"):
        perf_scoreboard.main(["--signals", str(evidence)])
    error = capsys.readouterr().err
    assert "error: --signals" in error
    assert "Traceback" not in error


def test_scoreboard_signals_refuses_exact_source_mismatch(
    retained_signals_board: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    from tools import perf_scoreboard

    with pytest.raises(SystemExit, match="2"):
        perf_scoreboard.main(
            ["--signals", str(retained_signals_board), "--expected-source", "b" * 40]
        )
    assert "error: --signals:" in capsys.readouterr().err


def test_molt_benchmark_args_are_canonicalized_for_all_call_shapes() -> None:
    expected_msgpack = ["--stdlib-profile", "full"]
    assert (
        bench_suites.molt_args_for_benchmark("tests/benchmarks/bench_parse_msgpack.py")
        == expected_msgpack
    )
    assert (
        bench_suites.molt_args_for_benchmark("bench_parse_msgpack.py")
        == expected_msgpack
    )
    assert (
        bench_suites.molt_args_for_benchmark(
            ROOT / "tests" / "benchmarks" / "bench_parse_msgpack.py"
        )
        == expected_msgpack
    )
    assert bench_suites.molt_args_for_benchmark("tests/benchmarks/bench_sum.py") == []

    first = bench_suites.molt_args_for_benchmark("bench_parse_msgpack.py")
    first.append("--mutated")
    assert (
        bench_suites.molt_args_for_benchmark("bench_parse_msgpack.py")
        == expected_msgpack
    )


def test_msgpack_full_stdlib_args_reach_wasm_and_profile_tools() -> None:
    expected = ["--stdlib-profile", "full"]
    script = "tests/benchmarks/bench_parse_msgpack.py"

    assert bench.molt_args_for_benchmark(script) == expected
    assert bench_individual.molt_args_for_benchmark(script) == expected
    assert bench_wasm.molt_args_for_benchmark(script) == expected
    assert profile_tool.molt_args_for_benchmark("bench_parse_msgpack.py") == expected
    assert bench_wasm.MOLT_ARGS_BY_BENCH[script] == expected


def test_heap_lifecycle_benchmarks_use_canonical_profile_epochs() -> None:
    expected_labels = {
        "bench_heap_canonical_cache.py": {"canonical_cache_hits"},
        "bench_weakref_native.py": {
            "weakref_constructor_cache_hits",
            "weakref_calls",
            "weakref_sticky_hash_hits",
        },
        "bench_gc_pressure.py": {"gc_pressure_allocation"},
        "bench_object_class_lifecycle.py": {
            "bare_object_lifecycle",
            "user_class_lifecycle",
        },
    }
    for filename, labels in expected_labels.items():
        source = (ROOT / "tests" / "benchmarks" / filename).read_text(encoding="utf-8")
        assert 'load_intrinsic("molt_profile_epoch_reset")' in source
        assert 'load_intrinsic("molt_profile_epoch_dump")' in source
        for label in labels:
            assert f'"{label}"' in source

    lifecycle_source = (
        ROOT / "tests" / "benchmarks" / "bench_object_class_lifecycle.py"
    ).read_text(encoding="utf-8")
    assert "allocate_bare_objects(10_000)" in lifecycle_source
    assert "allocate_plain_instances(10_000)" in lifecycle_source
    assert lifecycle_source.count("del value") == 2


_APPLICATION_CATALOG = """
[artifact.example]
name = "example"
version = "1.0"
url = "https://example.org/example-1.0-py3-none-any.whl"
size = 123
sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[[benchmark]]
id = "example-app"
[benchmark.execution]
driver = "tests/benchmarks/apps/example.py"
packages = ["example"]
data_archive = "example"
data_member = "example/input.txt"
data_path = "data/input.txt"
data_format = "bytes"
oracle = "tests/benchmarks/apps/oracles/example.txt"
run_args = ["data/input.txt", "1"]
molt_args = ["--capabilities", "fs.read"]
"""


def test_application_catalog_resolves_pins_and_owns_snapshot() -> None:
    raw = tomllib.loads(_APPLICATION_CATALOG)
    (app,) = benchmark_applications._decode_catalog(raw)
    raw["artifact"]["example"]["sha256"] = "b" * 64
    raw["benchmark"][0]["execution"]["run_args"].append("changed")
    raw["benchmark"][0]["execution"]["molt_args"].append("changed")
    assert app.name == "example-app"
    assert app.packages == (app.data_archive,)
    assert app.data_archive.name == "example"
    assert app.data_archive.version == "1.0"
    assert app.data_archive.size == 123
    assert app.data_archive.sha256 == "a" * 64
    assert app.run_args == ("data/input.txt", "1")
    assert app.molt_args == ("--capabilities", "fs.read")


def test_application_recipe_allows_explicit_empty_compiler_args() -> None:
    raw = tomllib.loads(_APPLICATION_CATALOG)
    raw["benchmark"][0]["execution"]["molt_args"] = []
    (app,) = benchmark_applications._decode_catalog(raw)
    assert app.molt_args == ()


@pytest.mark.parametrize("name", ["sqlglot_pipeline", "html5_document"])
def test_application_compiler_permissions_reach_existing_runners(name) -> None:
    script = f"tests/benchmarks/apps/{name}.py"
    expected = ["--capabilities", "fs.read"]
    for runner in (bench_suites, bench, bench_individual, bench_wasm, profile_tool):
        for path in (script, ROOT / script):
            args = runner.molt_args_for_benchmark(path)
            assert args == expected
            args.append("--mutated")
    assert bench_suites.molt_args_for_benchmark(Path("/elsewhere") / script) == []
    assert bench_suites.molt_args_for_benchmark(f"{name}.py") == []


@pytest.mark.parametrize(
    ("field", "value", "diagnostic"),
    [
        ("packages", "example", "list of strings"),
        ("packages", [1], "list of strings"),
        ("packages", ["example", "example"], "duplicate archive"),
        ("packages", ["absent"], "unknown archive reference"),
        ("data_archive", "absent", "unknown archive reference"),
        ("data_archive", False, "nonempty string"),
        ("run_args", "input.txt", "list of strings"),
        ("run_args", [False], "list of strings"),
        ("run_args", ["bad\x00arg"], "NUL"),
        ("molt_args", "--capabilities fs.read", "list of strings"),
        ("molt_args", [False], "list of strings"),
        ("molt_args", ["bad\x00arg"], "NUL"),
        ("data_format", [], "expected bytes or python-literals"),
        ("driver", "../example.py", "portable relative"),
        ("driver", 1, "portable relative"),
        ("data_member", "C:/input.txt", "portable relative"),
        ("data_path", "data/CON", "portable relative"),
        ("oracle", "/oracle.txt", "portable relative"),
    ],
)
def test_application_recipe_rejects_malformed_fields(field, value, diagnostic) -> None:
    raw = tomllib.loads(_APPLICATION_CATALOG)
    raw["benchmark"][0]["execution"][field] = value
    with pytest.raises(ValueError, match=diagnostic):
        benchmark_applications._decode_catalog(raw)


@pytest.mark.parametrize("value", [None, False, [], "recipe"])
def test_application_recipe_requires_table(value) -> None:
    raw = tomllib.loads(_APPLICATION_CATALOG)
    raw["benchmark"][0]["execution"] = value
    with pytest.raises(ValueError, match="expected a table"):
        benchmark_applications._decode_catalog(raw)


@pytest.mark.parametrize("field", ["run_args", "molt_args", "runner"])
def test_application_recipe_rejects_unknown_or_missing_fields(field) -> None:
    raw = tomllib.loads(_APPLICATION_CATALOG)
    recipe = raw["benchmark"][0]["execution"]
    if field != "runner":
        del recipe[field]
    else:
        recipe["runner"] = "another-runner"
    with pytest.raises(ValueError, match="missing fields.*unknown fields"):
        benchmark_applications._decode_catalog(raw)


@pytest.mark.parametrize(
    ("field", "value", "diagnostic"),
    [
        ("name", "different", "must match archive reference"),
        ("version", 1, "nonempty string"),
        ("size", True, "positive integer"),
        ("size", "123", "positive integer"),
        ("size", 0, "positive integer"),
        ("sha256", "A" * 64, "lowercase hex"),
        ("sha256", "a" * 63, "lowercase hex"),
        ("url", "http://example.org/archive.whl", "credential-free HTTPS"),
        ("url", "https://example.org/archive.whl?token=x", "literal filename"),
        ("url", 3, "nonempty string"),
    ],
)
def test_application_archive_rejects_malformed_pins(field, value, diagnostic) -> None:
    raw = tomllib.loads(_APPLICATION_CATALOG)
    raw["artifact"]["example"][field] = value
    with pytest.raises(ValueError, match=diagnostic):
        benchmark_applications._decode_catalog(raw)


@pytest.mark.parametrize("driver", ["example.py", "EXAMPLE.py"])
def test_application_catalog_rejects_ambiguous_drivers(driver) -> None:
    raw = tomllib.loads(_APPLICATION_CATALOG)
    second = deepcopy(raw["benchmark"][0])
    second["id"] = "second-app"
    second["execution"]["driver"] = "tests/benchmarks/apps/" + driver
    raw["benchmark"].append(second)
    with pytest.raises(ValueError, match="duplicate or ambiguous driver"):
        benchmark_applications._decode_catalog(raw)


def test_application_catalog_rejects_duplicate_ids() -> None:
    raw = tomllib.loads(_APPLICATION_CATALOG)
    raw["benchmark"].append({"id": "example-app"})
    with pytest.raises(ValueError, match="duplicate id"):
        benchmark_applications._decode_catalog(raw)


def test_application_selection_uses_one_snapshot_and_exact_path(
    tmp_path, monkeypatch
) -> None:
    catalog = tmp_path / "registry.toml"
    catalog.write_text(_APPLICATION_CATALOG, encoding="utf-8")
    monkeypatch.setattr(benchmark_applications, "CATALOG", catalog)
    monkeypatch.setattr(benchmark_applications, "ROOT", tmp_path)
    monkeypatch.setattr(
        benchmark_applications,
        "applications",
        cache(benchmark_applications.applications.__wrapped__),
    )
    (app,) = benchmark_applications.applications()
    catalog.write_text("invalid TOML [", encoding="utf-8")
    assert benchmark_applications.application_for_script(tmp_path / app.driver) is app
    assert benchmark_applications.applications() == (app,)

    def unexpected_acquisition(*args, **kwargs):
        pytest.fail("unregistered local workload attempted application preparation")

    monkeypatch.setattr(
        benchmark_applications, "prepare_application", unexpected_acquisition
    )
    assert benchmark_applications.prepare_for_script(tmp_path / "example.py") is None
    assert (
        benchmark_applications.prepare_for_script(tmp_path / "other" / app.driver)
        is None
    )


@pytest.mark.parametrize(
    "payload",
    [
        b"sealed \xc3\xa9 output\n",
        b"raw\xff\n",
        b"raw\xef\xbf\xbd\n",
        b"raw\r\nno-final-newline",
    ],
)
def test_application_oracle_records_exact_reference_bytes(
    tmp_path, monkeypatch, payload
) -> None:
    """A real guarded CPython child produces the oracle; bytes are never replaced.

    Invalid UTF-8 must refuse instead of becoming U+FFFD, which would make the
    oracle accept any undecodable byte; a literal U+FFFD stays exact.
    """
    import sys

    from molt.cli.source_package_seal import (
        SourcePackageInput,
        stage_source_package_seal,
    )

    source = tmp_path / "source"
    driver = source / "drivers" / "emit.py"
    driver.parent.mkdir(parents=True)
    driver.write_text(f"import os\nos.write(1, {payload!r})\n", encoding="utf-8")
    project = source / "pyproject.toml"
    project.write_text(
        '[project]\nname = "oracle-fixture"\nversion = "0"\n', encoding="utf-8"
    )
    seal = stage_source_package_seal(
        tmp_path / "seals",
        [
            SourcePackageInput(driver, "drivers/emit.py", "source"),
            SourcePackageInput(project, "pyproject.toml", "project"),
        ],
    )
    pin = benchmark_applications.ArchivePin("fixture", "0", "https://x/f", 1, "0" * 64)
    app = benchmark_applications.Application(
        name="oracle-fixture",
        driver="drivers/emit.py",
        packages=(),
        data_archive=pin,
        data_member="data",
        data_path="data/input",
        data_format="bytes",
        oracle="oracles/oracle-fixture.txt",
        run_args=(),
        molt_args=(),
    )
    monkeypatch.setattr(benchmark_applications, "ROOT", tmp_path)
    monkeypatch.setattr(benchmark_applications, "applications", lambda: (app,))
    monkeypatch.setattr(
        benchmark_applications,
        "prepare_application",
        lambda selected, *, include_oracle: (
            benchmark_applications.PreparedApplication(
                seal, ("src",), "oracle/stdout.txt", ()
            )
            if selected is app and include_oracle is False
            else pytest.fail("oracle production prepared the wrong application")
        ),
    )
    monkeypatch.setattr(
        sys, "argv", ["benchmark_applications", "--write-oracle", app.name]
    )
    target = tmp_path / app.oracle
    if payload == b"raw\xff\n":
        with pytest.raises(ValueError, match="not valid UTF-8"):
            benchmark_applications.main()
        assert not target.exists()
    else:
        assert benchmark_applications.main() == 0
        assert target.read_bytes() == payload


def test_application_sql_fixture_retains_semantic_values_without_execution() -> None:
    fixture = benchmark_applications._sql_fixture(
        b'SQL = "SELECT a FROM t"\nTPCH_SCHEMA = {"t": {"a": "INT"}}\n'
        b'raise RuntimeError("must never execute fixture code")\n'
    )
    assert fixture == {"SQL": "SELECT a FROM t", "TPCH_SCHEMA": {"t": {"a": "INT"}}}


@pytest.mark.parametrize(
    "source",
    [
        b"SQL = 3\nTPCH_SCHEMA = {}",
        b'SQL = "select 1"',
        b'SQL = "select 1"\nSQL = "select 2"\nTPCH_SCHEMA = {}',
        b'SQL = "select 1"\nTPCH_SCHEMA = []',
        b'SQL = "select 1"\nTPCH_SCHEMA = {"t": {"a": 3}}',
    ],
)
def test_application_sql_fixture_rejects_incomplete_or_wrong_types(source) -> None:
    with pytest.raises(ValueError, match="upstream SQL fixture"):
        benchmark_applications._sql_fixture(source)


@pytest.mark.parametrize("missing_notice", [False, True])
def test_application_data_wheel_retains_declared_notices(
    tmp_path, missing_notice
) -> None:
    archive = io.BytesIO()
    with zipfile.ZipFile(archive, "w") as wheel:
        wheel.writestr("example/input.txt", b"dataset bytes")
        wheel.writestr(
            "example-1.0.dist-info/METADATA",
            "Metadata-Version: 2.4\nName: example\nVersion: 1.0\n"
            "License-File: COPYING\nLicense-File: AUTHORS\n\n",
        )
        wheel.writestr("example-1.0.dist-info/licenses/COPYING", b"License terms")
        if not missing_notice:
            wheel.writestr(
                "example-1.0.dist-info/licenses/AUTHORS", b"Upstream authors"
            )
    with zipfile.ZipFile(archive) as wheel:
        if missing_notice:
            with pytest.raises(ValueError, match="missing declared member.*AUTHORS"):
                benchmark_applications._data_inputs(
                    wheel, "example/input.txt", tmp_path
                )
        else:
            data, inputs = benchmark_applications._data_inputs(
                wheel, "example/input.txt", tmp_path
            )
            assert data == b"dataset bytes"
            retained = {item.relative_path: item.source.read_bytes() for item in inputs}
            prefix = "provenance/data-archive/example-1.0.dist-info/"
            assert retained[prefix + "licenses/COPYING"] == b"License terms"
            assert retained[prefix + "licenses/AUTHORS"] == b"Upstream authors"
            assert prefix + "METADATA" in retained
            assert all(item.role == "provenance" for item in inputs)
