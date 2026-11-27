from __future__ import annotations

import json
from collections.abc import Callable, Iterator, Sequence
from contextlib import contextmanager
from pathlib import Path
from types import SimpleNamespace
from typing import Literal

import pytest

from molt import file_publication
from molt.node_runtime import NodeRuntime
from molt.process_environment import ProcessEnvironment
from tests.tools.perf_scoreboard_fixtures import (
    benchmark_oracle_interpreter,
    sealed_workload,
)
from tests.wasm_execution_manifest import write_wasm_execution_manifest
from tools import output_startup_size_audit


def test_first_load_probe_pairs_one_fresh_inode_with_its_warm_runs(
    tmp_path, monkeypatch
) -> None:
    from tools import executable_scan_probe as probe

    source = tmp_path / "program"
    source.write_bytes(b"independent fresh executable")
    source.chmod(0o755)
    starts = []
    times = iter([0.25, 0.02, 0.04, 0.35, 0.03, 0.05, 0.45, 0.04, 0.06])

    def run(command, **_kwargs):
        image = Path(command[0])
        assert image.read_bytes() == b"independent fresh executable"
        assert not image.samefile(source)
        starts.append((image, image.stat().st_ino))
        return SimpleNamespace(
            returncode=0,
            stdout="hello\n",
            stderr="",
            child_elapsed_s=next(times),
            elapsed_s=2.0,
        )

    monkeypatch.setattr(probe.startup, "_run_guarded", run)
    monkeypatch.setattr(probe, "_load", lambda: [1.0, 3.0, 4.0])
    report = probe.probe(source, work=tmp_path / "work", arguments=[], warm_runs=2)
    assert report["unloaded"] is True
    assert [row["first_extra_s"] for row in report["records"]] == [0.22, 0.31, 0.4]
    for index in range(0, 9, 3):
        assert starts[index : index + 3] == [starts[index]] * 3
    assert len({row[0] for row in starts}) == 3
    assert not list((tmp_path / "work").iterdir())


def test_first_load_probe_does_not_certify_loaded_or_unknown_hosts(
    tmp_path, monkeypatch
):
    from tools import executable_scan_probe as probe

    source = tmp_path / "program"
    source.write_bytes(b"probe")
    monkeypatch.setattr(
        probe.startup,
        "_run_guarded",
        lambda *a, **k: SimpleNamespace(
            returncode=0, stdout="", stderr="", child_elapsed_s=0.1, elapsed_s=0.2
        ),
    )
    for load in ([2.1, 1.0, 1.0], None):
        monkeypatch.setattr(probe, "_load", lambda: load)
        report = probe.probe(source, work=tmp_path / "work", arguments=[], samples=1)
        assert report["unloaded"] is False


def test_first_load_probe_refuses_changed_observable_output(tmp_path, monkeypatch):
    import pytest
    from tools import executable_scan_probe as probe

    source = tmp_path / "program"
    source.write_bytes(b"probe")
    outputs = iter(["first", "warm"])
    monkeypatch.setattr(
        probe.startup,
        "_run_guarded",
        lambda *a, **k: SimpleNamespace(
            returncode=0,
            stdout=next(outputs),
            stderr="",
            child_elapsed_s=0.1,
            elapsed_s=0.2,
        ),
    )
    with pytest.raises(ValueError, match="output/exit/stderr differ"):
        probe.probe(source, work=tmp_path / "work", arguments=[], samples=1)
    assert not list((tmp_path / "work").iterdir())


def test_first_load_probe_copies_and_seals_only_private_interpreter_bytes(
    tmp_path, monkeypatch
):
    from tools import executable_scan_probe as probe

    tree = tmp_path / "installation"
    tree.mkdir()
    binary = tree / "python"
    binary.write_bytes(b"interpreter")
    library = tree / "libpython"
    library.write_bytes(b"runtime")
    before = binary.stat()

    def run(command, **_kwargs):
        private = Path(command[0])
        assert private.read_bytes() == b"interpreter"
        assert (private.parent / "libpython").read_bytes() == b"runtime"
        assert private.stat().st_ino != before.st_ino
        return SimpleNamespace(
            returncode=0, stdout="", stderr="", child_elapsed_s=0.1, elapsed_s=0.2
        )

    monkeypatch.setattr(probe.startup, "_run_guarded", run)
    for seal in (False, True):
        probe.probe(
            binary,
            work=tmp_path / "work",
            arguments=[],
            samples=1,
            tree=tree,
            seal=seal,
        )
    after = binary.stat()
    assert (after.st_ino, after.st_nlink, after.st_ctime_ns) == (
        before.st_ino,
        before.st_nlink,
        before.st_ctime_ns,
    )


def test_stats_are_deterministic() -> None:
    audit = output_startup_size_audit

    stats = audit._stats([0.3, 0.1, 0.2])

    assert stats == {
        "count": 3,
        "min_s": 0.1,
        "median_s": 0.2,
        "mean_s": 0.2,
        "max_s": 0.3,
        "samples_s": [0.3, 0.1, 0.2],
    }


def test_measure_executable_uses_fresh_path_copies(
    tmp_path: Path,
    monkeypatch,
) -> None:
    audit = output_startup_size_audit
    binary = tmp_path / "probe_molt"
    binary.write_text("#!/bin/sh\nprintf '1\\n'\n", encoding="utf-8")
    binary.chmod(0o755)
    commands: list[list[str]] = []

    def fake_run(command, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        commands.append(list(command))
        return SimpleNamespace(
            returncode=0,
            stdout="1\n",
            stderr="",
            elapsed_s=0.12 + len(commands) / 1000,
            child_elapsed_s=0.12 + len(commands) / 1000,
        )

    monkeypatch.setattr(audit, "_run_guarded", fake_run)

    result = audit._measure_executable(
        binary,
        samples=2,
        env=ProcessEnvironment({}),
        timeout=1.0,
        fresh_copies=True,
        label="molt_page_cache_cold",
    )

    assert result["ok"] is True
    assert result["mode"] == "page_cache_cold_copy"
    assert result["stats"]["count"] == 2
    assert commands[0][0].endswith(".fresh-0")
    assert commands[1][0].endswith(".fresh-1")
    assert not any((binary.parent / ".fresh_start_samples").iterdir())


def test_measure_cold_first_sighting_runs_artifact_once(
    tmp_path: Path,
    monkeypatch,
) -> None:
    audit = output_startup_size_audit
    binary = tmp_path / "probe_molt"
    binary.write_text("#!/bin/sh\nprintf '1\\n'\n", encoding="utf-8")
    binary.chmod(0o755)
    commands: list[list[str]] = []

    def fake_run(command, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        commands.append(list(command))
        return SimpleNamespace(
            returncode=0,
            stdout="1\n",
            stderr="",
            elapsed_s=0.30,
            child_elapsed_s=0.30,
        )

    monkeypatch.setattr(audit, "_run_guarded", fake_run)

    result = audit._measure_cold_first_sighting(
        binary,
        env=ProcessEnvironment({}),
        timeout=1.0,
        label="molt_cold_first_sighting",
        runner_factory=audit._native_runner_factory(),
    )

    assert result["ok"] is True
    assert result["mode"] == "cold_first_sighting"
    assert result["stats"]["count"] == 1
    assert len(commands) == 1
    assert commands[0] == [str(binary)]


def test_budget_status_flags_size_and_startup_regressions() -> None:
    audit = output_startup_size_audit

    status = audit._budget_status(
        binary_bytes=12 * 1024 * 1024,
        fresh_start_stats={"median_s": 0.25},
        max_artifact_mb=10.0,
        max_fresh_start_ms=100.0,
    )

    assert status["passed"] is False
    failures = {check["name"] for check in status["checks"] if not check["passed"]}
    assert failures == {"artifact_size", "fresh_start_median"}


def test_matrix_cases_expand_native_backends_only() -> None:
    audit = output_startup_size_audit

    cases = audit._iter_matrix_cases(
        targets=("native", "wasm", "mlir"),
        build_profiles=("dev", "release"),
        backends=("all",),
        stdlib_profile="micro",
        wasm_opt_level="Oz",
    )

    ids = [case.id for case in cases]
    assert "native-dev-auto-stdlib-micro" in ids
    assert "native-dev-llvm-stdlib-micro" in ids
    assert "wasm-dev-wasm-Oz-stdlib-micro" in ids
    assert "mlir-release-mlir-stdlib-micro" in ids
    assert not any("wasm-dev-llvm" in case_id for case_id in ids)


def test_wasm_fresh_copy_preserves_linked_suffix(tmp_path: Path) -> None:
    audit = output_startup_size_audit
    artifact = tmp_path / "output_linked.wasm"
    fresh_dir = tmp_path / ".fresh"

    copied = audit._fresh_copy_path(artifact, fresh_dir, 7)

    assert copied.name == "output.fresh-7_linked.wasm"


def test_wasm_fresh_copy_rebases_manifest_to_measured_bytes(
    tmp_path: Path, monkeypatch
) -> None:
    audit = output_startup_size_audit
    artifact = tmp_path / "output_linked.wasm"
    artifact.write_bytes(b"\0asm\x01\0\0\0")
    write_wasm_execution_manifest(tmp_path, linked=artifact)
    observed_modules: list[Path] = []

    def fake_run(command, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        manifest = Path(command[-1])
        payload = json.loads(manifest.read_text(encoding="utf-8"))
        observed_modules.append(manifest.parent / payload["modules"]["linked"]["path"])
        return SimpleNamespace(
            returncode=0, stdout="", stderr="", elapsed_s=0.01, child_elapsed_s=0.01
        )

    monkeypatch.setattr(audit, "_run_guarded", fake_run)
    result = audit._measure_artifact(
        artifact,
        samples=2,
        env=ProcessEnvironment({}),
        timeout=1.0,
        fresh_copies=True,
        label="wasm-cold",
        runner_factory=audit._node_runner_factory("node"),
    )

    assert result["ok"] is True
    assert [path.name for path in observed_modules] == [
        "output.fresh-0_linked.wasm",
        "output.fresh-1_linked.wasm",
    ]
    assert not any((tmp_path / ".fresh_start_samples").iterdir())


def test_wasm_startup_uses_shared_node_selector_and_deterministic_flags(
    tmp_path: Path, monkeypatch
) -> None:
    audit = output_startup_size_audit
    artifact = tmp_path / "output_linked.wasm"
    artifact.write_bytes(b"\0asm\x01\0\0\0")
    write_wasm_execution_manifest(tmp_path, linked=artifact)
    node = tmp_path / "selected-node"
    selections: list[dict[str, object]] = []
    commands: list[list[str]] = []

    def fake_resolve(**kwargs):  # type: ignore[no-untyped-def]
        selections.append(kwargs)
        return NodeRuntime(path=node, version="22.1.0", major=22)

    def fake_run(command, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        commands.append(list(command))
        return SimpleNamespace(
            returncode=0, stdout="", stderr="", elapsed_s=0.01, child_elapsed_s=0.01
        )

    monkeypatch.setattr(audit, "resolve_node_runtime", fake_resolve)
    monkeypatch.setattr(audit, "_run_guarded", fake_run)
    env = ProcessEnvironment({"MOLT_NODE_BIN": str(node)})

    startup = audit._measure_case_startup(
        audit.MatrixCase(target="wasm", build_profile="dev", backend="wasm"),
        artifact,
        samples=1,
        env=env,
        timeout=1.0,
    )

    assert selections == [
        {"source_root": audit.ROOT, "environment": env, "guard_prefix": "MOLT_BENCH"}
    ]
    assert startup["node"] == {"path": str(node), "version": "22.1.0"}
    assert audit._startup_ok(startup, require_runners=True) is True
    assert commands[0] == [
        str(node),
        "--no-warnings",
        "--no-wasm-tier-up",
        "--no-wasm-dynamic-tiering",
        "--wasm-num-compilation-tasks=1",
        str(audit.WASM_RUNNER),
        str(audit.wasm_runtime_manifest_path(artifact)),
    ]


def test_wasm_startup_skips_with_selector_diagnostic_when_node_unavailable(
    tmp_path: Path, monkeypatch
) -> None:
    audit = output_startup_size_audit

    def fake_resolve(**kwargs):  # type: ignore[no-untyped-def]
        raise audit.NodeRuntimeError("Node >= 18 is unavailable on PATH")

    monkeypatch.setattr(audit, "resolve_node_runtime", fake_resolve)

    startup = audit._measure_case_startup(
        audit.MatrixCase(target="wasm", build_profile="dev", backend="wasm"),
        tmp_path / "output_linked.wasm",
        samples=1,
        env=ProcessEnvironment({}),
        timeout=1.0,
    )

    assert startup == {
        "runner": "node",
        "skipped": "Node >= 18 is unavailable on PATH",
    }
    assert audit._startup_ok(startup, require_runners=False) is True
    assert audit._startup_ok(startup, require_runners=True) is False


def test_wasm_startup_fails_on_unusable_explicit_node_selection(
    tmp_path: Path, monkeypatch
) -> None:
    audit = output_startup_size_audit

    def fail_run(command, **kwargs):  # type: ignore[no-untyped-def]
        raise AssertionError(f"unexpected launch: {command}")

    monkeypatch.setattr(audit, "_run_guarded", fail_run)
    # The real selector rejects a missing explicit path before any probe runs.
    startup = audit._measure_case_startup(
        audit.MatrixCase(target="wasm", build_profile="dev", backend="wasm"),
        tmp_path / "output_linked.wasm",
        samples=1,
        env=ProcessEnvironment({"MOLT_NODE_BIN": str(tmp_path / "missing-node")}),
        timeout=1.0,
    )

    assert "skipped" not in startup
    assert "MOLT_NODE_BIN" in startup["error"]
    assert audit._startup_ok(startup, require_runners=False) is False


def test_build_molt_artifact_emits_progress_on_stderr(
    tmp_path: Path,
    monkeypatch,
    capsys,
) -> None:
    audit = output_startup_size_audit
    script = tmp_path / "probe.py"
    script.write_text("print(1)\n", encoding="utf-8")
    out_dir = tmp_path / "out"
    artifact = out_dir / "probe.mlir"

    def fake_run(command, **kwargs):  # type: ignore[no-untyped-def]
        del command, kwargs
        out_dir.mkdir(parents=True, exist_ok=True)
        artifact.write_text("print(1)\n", encoding="utf-8")
        return SimpleNamespace(
            returncode=0,
            stdout=json.dumps({"data": {"artifacts": {"mlir": str(artifact)}}}),
            stderr="",
            elapsed_s=2.5,
            child_elapsed_s=2.5,
            timed_out=False,
        )

    monkeypatch.setattr(audit, "_run_guarded", fake_run)

    result = audit._build_molt_artifact(
        case=audit.MatrixCase(target="mlir", build_profile="dev", backend="mlir"),
        script=script,
        out_dir=out_dir,
        env=ProcessEnvironment({}),
        timeout=42.0,
        extra_molt_args=[],
    )

    progress_events = [
        json.loads(line)
        for line in capsys.readouterr().err.splitlines()
        if line.strip()
    ]
    assert result.artifact == artifact
    assert [event["event"] for event in progress_events] == [
        "output_startup_size_audit.build_start",
        "output_startup_size_audit.build_done",
    ]
    assert progress_events[0]["case"] == "mlir-dev-mlir-stdlib-micro"
    assert progress_events[1]["returncode"] == 0
    assert progress_events[1]["elapsed_s"] == 2.5


def test_main_writes_json_report_without_running_real_build(
    tmp_path: Path,
    monkeypatch,
    capsys,
) -> None:
    audit = output_startup_size_audit
    script = tmp_path / "probe.py"
    script.write_text("print(1)\n", encoding="utf-8")
    artifact = tmp_path / "probe_molt"
    artifact.write_bytes(b"molt-binary")
    json_out = tmp_path / "audit.json"

    def fake_build(**kwargs):  # type: ignore[no-untyped-def]
        return audit.BuildResult(
            case=kwargs["case"],
            command=["molt", "build", str(kwargs["script"])],
            artifact=artifact,
            artifacts={"selected": artifact},
            returncode=0,
            elapsed_s=1.25,
            stdout="{}",
            stderr="",
            payload=None,
        )

    def fake_startup(case, artifact_path, **kwargs):  # type: ignore[no-untyped-def]
        del case, artifact_path, kwargs
        single_stat = {
            "count": 1,
            "min_s": 0.01,
            "median_s": 0.01,
            "mean_s": 0.01,
            "max_s": 0.01,
            "samples_s": [0.01],
        }
        return {
            "runner": "native-exec",
            "cold_first_sighting": {
                "label": "molt_cold_first_sighting",
                "mode": "cold_first_sighting",
                "ok": True,
                "stats": dict(single_stat),
                "records": [{"command": [str(artifact)], "returncode": 0}],
            },
            "same_path": {
                "label": "molt_same_path",
                "mode": "same_path_reuse",
                "ok": True,
                "stats": dict(single_stat),
                "records": [{"command": [str(artifact)], "returncode": 0}],
            },
            "page_cache_cold": {
                "label": "molt_page_cache_cold",
                "mode": "page_cache_cold_copy",
                "ok": True,
                "stats": dict(single_stat),
                "records": [{"command": [str(artifact)], "returncode": 0}],
            },
        }

    monkeypatch.setattr(audit, "_build_molt_artifact", fake_build)
    monkeypatch.setattr(audit, "_measure_case_startup", fake_startup)

    def unexpected_python(**kwargs: object) -> None:
        raise AssertionError("disabled CPython baseline resolved an interpreter")

    monkeypatch.setattr(audit, "resolve_benchmark_python", unexpected_python)
    monkeypatch.setattr(audit, "_measure_c_baseline", lambda *a, **k: None)

    rc = audit.main(
        [
            "--script",
            str(script),
            "--samples",
            "1",
            "--targets",
            "native",
            "--build-profiles",
            "release",
            "--backends",
            "auto",
            "--json-out",
            str(json_out),
            "--no-c-baseline",
            "--no-cpython-baseline",
            "--json",
        ]
    )

    captured = capsys.readouterr()
    stdout_payload = json.loads(captured.out)
    progress_events = [
        json.loads(line) for line in captured.err.splitlines() if line.strip()
    ]
    payload = json.loads(json_out.read_text(encoding="utf-8"))
    assert rc == 0
    assert stdout_payload == payload
    assert [event["event"] for event in progress_events] == [
        "output_startup_size_audit.case_start",
        "output_startup_size_audit.case_done",
        "output_startup_size_audit.baseline_start",
        "output_startup_size_audit.baseline_done",
        "output_startup_size_audit.baseline_start",
        "output_startup_size_audit.baseline_done",
    ]
    assert payload["event"] == "output_startup_size_audit"
    assert payload["workload"]["entry"] == str(script)
    assert payload["workload"]["entry_source"] == "probe.py"
    assert payload["probe_source"] == "print(1)\n"
    assert payload["summary"]["cases"] == 1
    assert payload["cases"][0]["artifact"]["bytes"] == len(b"molt-binary")
    startup = payload["cases"][0]["startup"]
    assert startup["page_cache_cold"]["mode"] == "page_cache_cold_copy"
    assert startup["cold_first_sighting"]["mode"] == "cold_first_sighting"


@pytest.mark.parametrize("stage", ["build", "publication", "closure"])
def test_source_drift_refuses_output_audit_publication(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    stage: Literal["build", "publication", "closure"],
) -> None:
    audit = output_startup_size_audit
    script = tmp_path / "probe.py"
    script.write_text("from helper import value\nprint(value)\n", encoding="utf-8")
    helper = tmp_path / "helper.py"
    helper.write_text("value = 1\n", encoding="utf-8")
    artifact = tmp_path / "probe.mlir"
    artifact.write_bytes(b"compiled fixture")
    json_out = tmp_path / "audit.json"
    json_out.write_text('{"previous": true}\n', encoding="utf-8")
    baseline_calls: list[audit.AdmittedBenchmarkWorkload] = []

    def build(**kwargs: object) -> audit.BuildResult:
        case = kwargs["case"]
        assert isinstance(case, audit.MatrixCase)
        assert kwargs["script"] == script
        if stage == "build":
            helper.write_text("value = 1000\n", encoding="utf-8")
        return audit.BuildResult(
            case, ["molt", "build", str(script)], artifact, {}, 0, 0.1, "", "", None
        )

    def measure_cpython(
        workload: audit.AdmittedBenchmarkWorkload, **kwargs: object
    ) -> dict[str, object]:
        baseline_calls.append(workload)
        assert workload.execution_script == script
        assert {item["path"] for item in workload.record["sources"]} == {
            "probe.py",
            "helper.py",
        }
        return {"ok": True}

    monkeypatch.setattr(audit, "_build_molt_artifact", build)
    monkeypatch.setattr(audit, "_measure_cpython", measure_cpython)
    monkeypatch.setattr(
        audit, "resolve_benchmark_python", lambda **_: benchmark_oracle_interpreter()
    )
    atomic_write = file_publication.atomic_write_bytes

    def change_at_replace(
        path: Path,
        data: bytes,
        *,
        replace: Callable[[Path, Path], None] | None = None,
        **kwargs: object,
    ) -> None:
        if path != json_out:
            atomic_write(path, data, replace=replace, **kwargs)
            return
        assert replace is not None

        def mutate(staged: Path, destination: Path) -> None:
            helper.write_text("value = 1000\n", encoding="utf-8")
            replace(staged, destination)

        atomic_write(path, data, replace=mutate, **kwargs)

    if stage == "publication":
        monkeypatch.setattr(file_publication, "atomic_write_bytes", change_at_replace)
    if stage == "closure":
        admit_workload = audit.admit_benchmark_workload

        @contextmanager
        def change_at_close(
            script: Path,
            *,
            project_root: Path,
            run_args: Sequence[str],
            molt_args: Sequence[str],
            cpython_command: Sequence[str] | None,
        ) -> Iterator[audit.AdmittedBenchmarkWorkload]:
            with admit_workload(
                script,
                project_root=project_root,
                run_args=run_args,
                molt_args=molt_args,
                cpython_command=cpython_command,
            ) as workload:
                yield workload
                helper.write_text("value = 1000\n", encoding="utf-8")

        monkeypatch.setattr(audit, "admit_benchmark_workload", change_at_close)

    with pytest.raises(ValueError, match="benchmark source.*changed"):
        audit.main(
            [
                "--script",
                str(script),
                "--targets",
                "mlir",
                "--samples",
                "1",
                "--build-profile",
                "dev",
                "--work-dir",
                str(tmp_path / "work"),
                "--json-out",
                str(json_out),
                "--no-c-baseline",
                "--json",
            ]
        )

    assert len(baseline_calls) == (0 if stage == "build" else 1)
    assert json_out.read_text(encoding="utf-8") == '{"previous": true}\n'


def test_startup_matrix_uses_sealed_execution_source_and_arguments(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    audit = output_startup_size_audit
    artifact = tmp_path / "program"
    artifact.write_bytes(b"compiled fixture")
    calls: list[tuple[list[str], Path]] = []
    baseline = benchmark_oracle_interpreter()
    with sealed_workload(tmp_path, molt_args=["--profile", "dev"]) as workload:
        record = workload.record
        root = workload.execution_root
        roots = str(root / "packages")

        def build(**kwargs: object) -> audit.BuildResult:
            case = kwargs["case"]
            assert isinstance(case, audit.MatrixCase)
            assert kwargs["script"] == workload.execution_script
            assert kwargs["cwd"] == root
            assert kwargs["extra_molt_args"] == record["molt_args"]
            env = kwargs["env"]
            assert isinstance(env, ProcessEnvironment)
            assert env["MOLT_HERMETIC_MODULE_ROOTS"] == "1"
            assert env["MOLT_MODULE_ROOTS"] == roots
            return audit.BuildResult(
                case,
                ["molt", "build", str(workload.execution_script)],
                artifact,
                {},
                0,
                0.1,
                "",
                "",
                None,
            )

        def launch(
            command: list[str], *, cwd: Path, **kwargs: object
        ) -> SimpleNamespace:
            calls.append((command, cwd))
            return SimpleNamespace(
                returncode=0,
                stdout="sealed:7\n",
                stderr="",
                child_elapsed_s=0.02,
            )

        monkeypatch.setattr(audit, "_build_molt_artifact", build)
        monkeypatch.setattr(audit, "_run_guarded", launch)
        args = audit._parse_args(["--samples", "1"])
        env = ProcessEnvironment({})
        row = audit._build_case_row(
            case=audit.MatrixCase("native", "release", "auto"),
            workload=workload,
            out_dir=tmp_path / "out",
            env=env,
            args=args,
        )
        reference = audit._measure_cpython(
            workload,
            baseline=baseline,
            samples=1,
            env=env,
            timeout=1,
        )

    assert row["status"] == "ok"
    assert reference["ok"] is True
    assert len(calls) == 4
    assert all(cwd == root for _, cwd in calls)
    assert all(command[1:] == record["run_args"] for command, _ in calls[:3])
    cpython_command, _ = calls[-1]
    assert cpython_command[:5] == [*baseline.command, "-c"]
    assert cpython_command[6:] == [
        str(workload.execution_script),
        "1",
        roots,
        *record["run_args"],
    ]


def test_startup_samples_exclude_descendant_cleanup_wall_time():
    audit = output_startup_size_audit
    result = SimpleNamespace(
        returncode=0, elapsed_s=12.0, child_elapsed_s=0.02, stdout="", stderr=""
    )
    assert (
        audit._sample_record(index=0, command=["probe"], result=result)["elapsed_s"]
        == 0.02
    )


def test_startup_sample_rejects_missing_execution_clock():
    import pytest

    audit = output_startup_size_audit
    with pytest.raises(ValueError, match="execution clock"):
        audit._sample_record(
            index=0,
            command=["probe"],
            result=SimpleNamespace(returncode=0, elapsed_s=12.0, stdout="", stderr=""),
        )


def test_failed_startup_keeps_diagnostics_without_execution_clock():
    audit = output_startup_size_audit
    result = SimpleNamespace(
        returncode=124,
        elapsed_s=12.0,
        timed_out=True,
        stdout="partial output",
        stderr="deadline expired",
    )
    record = audit._sample_record(index=0, command=["probe"], result=result)
    assert record["elapsed_s"] is None
    assert record["guard_wall_elapsed_s"] == 12.0
    assert record["returncode"] == 124 and record["timed_out"]
    assert record["stderr"] == "deadline expired"
    assert audit._execution_elapsed(result) is None


def test_successful_startup_rejects_invalid_child_clock():
    import pytest

    audit = output_startup_size_audit
    for value in (None, True, -0.1, float("nan"), float("inf")):
        with pytest.raises(ValueError, match="valid direct-child execution clock"):
            audit._sample_record(
                index=0,
                command=["probe"],
                result=SimpleNamespace(
                    returncode=0,
                    elapsed_s=0.01,
                    child_elapsed_s=value,
                    stdout="",
                    stderr="",
                ),
            )


def test_serial_startup_samples_propagate_operator_signal_only(monkeypatch, tmp_path):
    import signal

    calls = []
    terminal = SimpleNamespace(
        returncode=143,
        guard_signal=None,
        timed_out=False,
        elapsed_s=0.01,
        stdout="",
        stderr="child failed",
    )

    def guarded(command, **kwargs):
        calls.append(command)
        return terminal

    audit = output_startup_size_audit
    monkeypatch.setattr(
        audit.harness_memory_guard, "guarded_completed_process", guarded
    )
    script = tmp_path / "probe.py"
    script.write_text("print('probe')\n", encoding="utf-8")
    baseline = benchmark_oracle_interpreter()
    with audit.admit_benchmark_workload(
        script,
        project_root=tmp_path,
        run_args=(),
        molt_args=(),
        cpython_command=baseline.command,
    ) as workload:
        result = audit._measure_cpython(
            workload,
            baseline=baseline,
            samples=2,
            env=ProcessEnvironment({}),
            timeout=1,
        )
        assert len(calls) == 2 and not result["ok"]
        assert calls == [[*baseline.command, str(script)]] * 2
        assert result["cpython_oracle"]["cmd"] == list(baseline.command)
        assert [r["returncode"] for r in result["records"]] == [143, 143]
        calls.clear()
        terminal.guard_signal = signal.SIGTERM
        with pytest.raises(SystemExit) as caught:
            audit._measure_cpython(
                workload,
                baseline=baseline,
                samples=2,
                env=ProcessEnvironment({}),
                timeout=1,
            )
        assert caught.value.guarded_result is terminal
        assert len(calls) == 1
