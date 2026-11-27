"""Actual audit consumers retain or close the canonical benchmark allocation."""

from __future__ import annotations

import json
import sys
from contextlib import AbstractContextManager, nullcontext
from pathlib import Path
from types import SimpleNamespace

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))

from tools import bench  # noqa: E402
from tools import bench_audit as audit  # noqa: E402
from molt import temporary_artifacts  # noqa: E402
from molt.process_environment import ProcessEnvironment  # noqa: E402
from molt.python_interpreter import PythonInterpreter  # noqa: E402
from tests.tools.perf_scoreboard_fixtures import (  # noqa: E402
    benchmark_oracle_interpreter,
    sealed_workload,
)


def test_audit_uses_one_isolated_baseline_and_observed_child_time(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    sources = [tmp_path / "first.py", tmp_path / "second.py"]
    for source in sources:
        source.write_text("print('ordinary benchmark')\n", encoding="utf-8")
    baseline = benchmark_oracle_interpreter()
    resolutions = []
    commands = []

    def resolve(**kwargs: object) -> PythonInterpreter:
        resolutions.append(kwargs)
        return baseline

    def launch(
        command: list[str], **kwargs: object
    ) -> audit.harness_memory_guard.GuardedCompletedProcess:
        commands.append(tuple(command))
        env = kwargs["env"]
        assert isinstance(env, ProcessEnvironment)
        assert "PYTHONPATH" not in env and "PYTHONHASHSEED" not in env
        return audit.harness_memory_guard.GuardedCompletedProcess(
            list(command),
            0,
            "ordinary benchmark\n",
            "",
            elapsed_s=9.0,
            child_elapsed_s=0.25,
            child_returncode=0,
            child_stderr="",
            descendants_closed=True,
            peak=audit.harness_memory_guard.memory_guard.RssViolation(
                pid=1, rss_kb=1024, command="fixture", scope="process"
            ),
        )

    monkeypatch.setattr(audit, "resolve_benchmark_python", resolve)
    monkeypatch.setattr(audit.harness_memory_guard, "guarded_completed_process", launch)
    monkeypatch.setattr(
        audit,
        "prepare_molt_binary",
        lambda *args, **kwargs: pytest.fail("CPython-only build"),
    )
    monkeypatch.setenv("PYTHONPATH", str(tmp_path / "hostile"))
    monkeypatch.setenv("PYTHONHASHSEED", "0")
    results = audit.audit_benchmarks(
        [str(path) for path in sources], samples=2, skip_molt=True
    )

    assert len(resolutions) == 1
    assert commands == [
        (*baseline.command, str(path)) for path in sources for _ in range(2)
    ]
    assert [row.cpython_s for row in results] == [0.25, 0.25]
    assert all(
        row.classification == "Skipped" and row.speedup is None for row in results
    )
    assert all(
        row.cpython_oracle is not None
        and row.cpython_oracle["cmd"] == list(baseline.command)
        for row in results
    )


@pytest.mark.parametrize("phase", ["median", "perf-rebuild"])
@pytest.mark.parametrize("outcome", ["success", "unavailable", "error", "cancel"])
@pytest.mark.parametrize("cleanup_failure", [False, True])
def test_audit_consumers_close_or_retain_exact_owner(
    monkeypatch, tmp_path, phase, outcome, cleanup_failure
):
    source = tmp_path / "ordinary.py"
    source.write_text("pass\n", encoding="utf-8")
    owners = []
    original = (
        KeyboardInterrupt("independent audit cancellation")
        if outcome == "cancel"
        else OSError("independent audit observation failure")
    )
    target_count = 1 if phase == "median" else 2

    def build(*args, **kwargs):
        owner = temporary_artifacts.OwnedTemporaryDirectory(dir=tmp_path)
        owners.append(owner)
        artifact = Path(owner.name) / "program"
        artifact.write_bytes(b"independent artifact")
        return bench.MoltBinary(artifact, owner, 0.1, 1.0)

    def observe(path, *args, **kwargs):
        assert Path(path).is_file()
        if len(owners) == target_count:
            if outcome in {"error", "cancel"}:
                raise original
            if outcome == "unavailable":
                return bench.MoltFailure("run", "runtime_failed", 1, False, None)
        return bench.RunSample(1.0, "", "")

    def perf(path: str | Path, **kwargs: object) -> dict[str, int] | None:
        result = observe(path)
        return None if isinstance(result, bench.MoltFailure) else {"instructions": 123}

    monkeypatch.setattr(audit, "prepare_molt_binary", build)
    monkeypatch.setattr(audit, "measure_molt_run", observe)
    monkeypatch.setattr(audit, "_collect_perf_data", perf)
    monkeypatch.setattr(
        audit,
        "_cpython_samples",
        lambda *a: bench.SampleBatch([bench.RunSample(2.0, "", "")], True),
    )
    monkeypatch.setattr(
        audit, "resolve_benchmark_python", lambda **_: benchmark_oracle_interpreter()
    )
    monkeypatch.setattr(audit, "platform", SimpleNamespace(system=lambda: "Linux"))
    monkeypatch.setattr(
        audit, "shutil", SimpleNamespace(which=lambda command: "/fixture/perf")
    )
    real_delete = temporary_artifacts.delete_path

    def delete(path, *, owning_root=None):
        if len(owners) == target_count and path == Path(owners[-1].name):
            return False, "independent audit deletion refusal"
        return real_delete(path, owning_root=owning_root)

    unrelated = RuntimeError("unrelated caller exception")

    def run():
        try:
            raise unrelated
        except RuntimeError:
            if phase == "median":
                with audit.admit_benchmark_workload(
                    source, project_root=tmp_path, run_args=(), molt_args=()
                ) as workload:
                    return audit._molt_samples(workload, 1, 1.0, ProcessEnvironment({}))
            return audit.audit_benchmarks([str(source)], samples=1)

    with monkeypatch.context() as scoped:
        if cleanup_failure:
            scoped.setattr(temporary_artifacts, "delete_path", delete)
        if outcome in {"error", "cancel"} or cleanup_failure:
            with pytest.raises(
                type(original) if outcome in {"error", "cancel"} else OSError
            ) as caught:
                run()
            if outcome in {"error", "cancel"}:
                assert caught.value is original
            if cleanup_failure:
                assert caught.value.benchmark_temporary_directories == (owners[-1],)
                assert Path(owners[-1].name).is_dir()
        else:
            result = run()
            if phase == "median":
                batch, build_s = result
                assert build_s == 0.1
                assert batch.times_s == ([] if outcome == "unavailable" else [1.0])
            else:
                assert result[0].perf_data == (
                    None if outcome == "unavailable" else {"instructions": 123}
                )
    assert not hasattr(unrelated, "benchmark_temporary_directories")
    assert len(owners) == target_count
    for owner in owners:
        if Path(owner.name).exists():
            assert cleanup_failure and owner is owners[-1]
            owner.cleanup()
        assert not Path(owner.name).exists()


@pytest.mark.parametrize(
    ("changes", "parity_reason"),
    [
        ({}, "match"),
        ({("molt", 2): ("out\udcfe\r\n", "err\n")}, "molt_unstable"),
        ({("molt", 2): ("out\udcff\r\n", "err")}, "molt_unstable"),
        ({("cpython", 2): ("out\udcfe\r\n", "err\n")}, "reference_unstable"),
        ({("cpython", 2): ("out\udcff\r\n", "err")}, "reference_unstable"),
        ({}, "reference_unavailable"),
        (
            {("molt", i): ("out\udcfe\r\n", "err\n") for i in (1, 2)},
            "stdout_mismatch",
        ),
        (
            {("molt", i): ("out\udcff\r\n", "err") for i in (1, 2)},
            "stderr_mismatch",
        ),
    ],
)
def test_audit_speedup_requires_exact_output_from_every_sample(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    changes: dict[tuple[str, int], tuple[str, str]],
    parity_reason: str,
) -> None:
    source = tmp_path / "audit.py"
    source.write_text("pass\n", encoding="utf-8")
    baseline = benchmark_oracle_interpreter()
    counts = {"cpython": 0, "molt": 0}
    owners: list[temporary_artifacts.OwnedTemporaryDirectory] = []

    def build(*args: object, **kwargs: object) -> bench.MoltBinary:
        owner = temporary_artifacts.OwnedTemporaryDirectory(dir=tmp_path)
        owners.append(owner)
        artifact = Path(owner.name) / "program"
        artifact.write_bytes(b"audit fixture")
        return bench.MoltBinary(artifact, owner, 0.1, 1.0)

    def launch(
        command: list[str], **kwargs: object
    ) -> audit.harness_memory_guard.GuardedCompletedProcess:
        runtime = "cpython" if command[0] == baseline.executable else "molt"
        counts[runtime] += 1
        stdout, stderr = changes.get(
            (runtime, counts[runtime]), ("out\udcff\r\n", "err\n")
        )
        returncode = (
            1
            if parity_reason == "reference_unavailable"
            and runtime == "cpython"
            and counts[runtime] == 2
            else 0
        )
        assert kwargs["text"] is False and kwargs["errors"] == "surrogateescape"
        return audit.harness_memory_guard.GuardedCompletedProcess(
            command,
            returncode,
            stdout,
            "guard diagnostics",
            elapsed_s=9.0,
            child_elapsed_s=2.0 if runtime == "cpython" else 1.0,
            child_returncode=returncode,
            child_stderr=stderr,
            descendants_closed=True,
            peak=audit.harness_memory_guard.memory_guard.RssViolation(
                pid=1, rss_kb=1024, command="fixture", scope="process"
            ),
        )

    monkeypatch.setattr(audit, "resolve_benchmark_python", lambda **_: baseline)
    monkeypatch.setattr(audit.harness_memory_guard, "guarded_completed_process", launch)
    monkeypatch.setattr(audit, "prepare_molt_binary", build)
    monkeypatch.setattr(audit, "shutil", SimpleNamespace(which=lambda _: None))
    results = audit.audit_benchmarks([str(source)], samples=2)
    output = tmp_path / "audit.json"
    audit.save_results(results, output)
    (record,) = json.loads(output.read_text(encoding="utf-8"))

    matched = parity_reason == "match"
    reference_ready = parity_reason not in (
        "reference_unavailable",
        "reference_unstable",
    )
    assert counts == {"cpython": 2, "molt": 2 if reference_ready else 0}
    assert len(owners) == (1 if reference_ready else 0)
    assert all(not Path(owner.name).exists() for owner in owners)
    assert record["workload"]["entry"] == str(source)
    assert record["molt_output_parity"]["reason"] == parity_reason
    if parity_reason == "reference_unavailable":
        assert record["molt_output_parity"]["ok"] is None
        assert record["error"].startswith("CPython ")
    else:
        assert record["molt_output_parity"]["ok"] is matched
        if not matched:
            assert record["error"] == parity_reason
    assert record["class"] == ("Green" if matched else "Error")
    assert record["speedup"] == (2.0 if matched else None)
    assert record["molt_s"] == (1.0 if matched else None)


def test_audit_uses_the_same_sealed_payload_for_both_runtimes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    baseline = benchmark_oracle_interpreter()
    owner = temporary_artifacts.OwnedTemporaryDirectory(dir=tmp_path)
    artifact = Path(owner.name) / "program"
    artifact.write_bytes(b"audit fixture")
    with sealed_workload(tmp_path, molt_args=["--profile", "dev"]) as sealed:
        record = sealed.record
        roots = str(sealed.execution_root / "packages")

        def admit(
            script: Path, **kwargs: object
        ) -> AbstractContextManager[audit.AdmittedBenchmarkWorkload]:
            assert script == sealed.script
            assert kwargs["run_args"] == record["run_args"]
            assert kwargs["molt_args"] == record["molt_args"]
            assert kwargs["cpython_command"] == baseline.command
            return nullcontext(sealed)

        def build(script: str, **kwargs: object) -> bench.MoltBinary:
            assert script == str(sealed.execution_script)
            assert kwargs["extra_args"] == record["molt_args"]
            assert kwargs["source_cwd"] == sealed.execution_root
            env = kwargs["env"]
            assert isinstance(env, ProcessEnvironment)
            assert env["MOLT_HERMETIC_MODULE_ROOTS"] == "1"
            assert env["MOLT_MODULE_ROOTS"] == roots
            return bench.MoltBinary(artifact, owner, 0.1, 1.0)

        def reference(command: list[str], **kwargs: object) -> bench.RunSample:
            assert command[:5] == [*baseline.command, "-c"]
            assert command[6:] == [
                str(sealed.execution_script),
                "1",
                roots,
                *record["run_args"],
            ]
            assert kwargs["cwd"] == sealed.execution_root
            return bench.RunSample(2.0, "sealed:7\n", "")

        def guest(binary: Path, **kwargs: object) -> bench.RunSample:
            assert binary == artifact
            assert kwargs["cwd"] == sealed.execution_root
            assert kwargs["run_args"] == record["run_args"]
            return bench.RunSample(1.0, "sealed:7\n", "")

        monkeypatch.setattr(audit, "admit_benchmark_workload", admit)
        monkeypatch.setattr(audit, "resolve_benchmark_python", lambda **_: baseline)
        monkeypatch.setattr(audit, "prepare_molt_binary", build)
        monkeypatch.setattr(audit, "measure_molt_run", guest)
        monkeypatch.setattr(bench, "measure_runtime", reference)
        monkeypatch.setattr(
            audit.bench_suites, "application_run_args", lambda _: record["run_args"]
        )
        monkeypatch.setattr(
            audit.bench_suites, "molt_args_for_benchmark", lambda _: record["molt_args"]
        )
        monkeypatch.setattr(audit, "shutil", SimpleNamespace(which=lambda _: None))
        (result,) = audit.audit_benchmarks([str(sealed.script)], samples=1)

    assert result.workload == record
    assert result.speedup == 2.0
    assert not Path(owner.name).exists()


@pytest.mark.parametrize("stdout", ["100000\n", "100000"])
def test_audit_molt_only_uses_its_oracle_without_resolving_cpython(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, stdout: str
) -> None:
    owner = temporary_artifacts.OwnedTemporaryDirectory(dir=tmp_path)
    artifact = Path(owner.name) / "program"
    artifact.write_bytes(b"audit fixture")

    def unexpected_resolution(**kwargs: object) -> PythonInterpreter:
        pytest.fail("Molt-only benchmark requested a CPython oracle")

    monkeypatch.setattr(audit, "resolve_benchmark_python", unexpected_resolution)
    monkeypatch.setattr(
        audit,
        "prepare_molt_binary",
        lambda *a, **k: bench.MoltBinary(artifact, owner, 0.1, 1.0),
    )
    monkeypatch.setattr(
        audit, "measure_molt_run", lambda *a, **k: bench.RunSample(1.0, stdout, "")
    )
    monkeypatch.setattr(audit, "shutil", SimpleNamespace(which=lambda _: None))
    (result,) = audit.audit_benchmarks(
        ["tests/benchmarks/bench_ptr_registry.py"], samples=1
    )

    assert result.cpython_oracle is None
    assert result.speedup is None
    if stdout.endswith("\n"):
        assert result.classification == "Molt-only"
        assert result.molt_s == 1.0
    else:
        assert result.classification == "Error"
        assert result.error == "intrinsic_oracle_mismatch"
        assert result.molt_s is None
    assert not Path(owner.name).exists()
