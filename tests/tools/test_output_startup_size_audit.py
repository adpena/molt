from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import sys
from types import ModuleType, SimpleNamespace


REPO_ROOT = Path(__file__).resolve().parents[2]
AUDIT_PATH = REPO_ROOT / "tools" / "output_startup_size_audit.py"


def _load_audit() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "molt_tools_output_startup_size_audit",
        AUDIT_PATH,
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_stats_are_deterministic() -> None:
    audit = _load_audit()

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
    audit = _load_audit()
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
        )

    monkeypatch.setattr(audit, "_run_guarded", fake_run)

    result = audit._measure_executable(
        binary,
        samples=2,
        env={},
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
    audit = _load_audit()
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
        )

    monkeypatch.setattr(audit, "_run_guarded", fake_run)

    result = audit._measure_cold_first_sighting(
        binary,
        env={},
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
    audit = _load_audit()

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
    audit = _load_audit()

    cases = audit._iter_matrix_cases(
        targets=("native", "wasm", "luau", "mlir"),
        build_profiles=("dev", "release"),
        backends=("all",),
        stdlib_profile="micro",
        wasm_opt_level="Oz",
    )

    ids = [case.id for case in cases]
    assert "native-dev-auto-stdlib-micro" in ids
    assert "native-dev-llvm-stdlib-micro" in ids
    assert "wasm-dev-wasm-Oz-stdlib-micro" in ids
    assert "luau-release-luau-stdlib-micro" in ids
    assert "mlir-release-mlir-stdlib-micro" in ids
    assert not any("wasm-dev-llvm" in case_id for case_id in ids)


def test_wasm_fresh_copy_preserves_linked_suffix(tmp_path: Path) -> None:
    audit = _load_audit()
    artifact = tmp_path / "output_linked.wasm"
    fresh_dir = tmp_path / ".fresh"

    copied = audit._fresh_copy_path(artifact, fresh_dir, 7)

    assert copied.name == "output.fresh-7_linked.wasm"


def test_build_molt_artifact_emits_progress_on_stderr(
    tmp_path: Path,
    monkeypatch,
    capsys,
) -> None:
    audit = _load_audit()
    script = tmp_path / "probe.py"
    script.write_text("print(1)\n", encoding="utf-8")
    out_dir = tmp_path / "out"
    artifact = out_dir / "probe.luau"

    def fake_run(command, **kwargs):  # type: ignore[no-untyped-def]
        del command, kwargs
        out_dir.mkdir(parents=True, exist_ok=True)
        artifact.write_text("print(1)\n", encoding="utf-8")
        return SimpleNamespace(
            returncode=0,
            stdout=json.dumps({"data": {"artifacts": {"luau": str(artifact)}}}),
            stderr="",
            elapsed_s=2.5,
            timed_out=False,
        )

    monkeypatch.setattr(audit, "_run_guarded", fake_run)

    result = audit._build_molt_artifact(
        case=audit.MatrixCase(target="luau", build_profile="dev", backend="luau"),
        script=script,
        out_dir=out_dir,
        env={},
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
    assert progress_events[0]["case"] == "luau-dev-luau-stdlib-micro"
    assert progress_events[1]["returncode"] == 0
    assert progress_events[1]["elapsed_s"] == 2.5


def test_main_writes_json_report_without_running_real_build(
    tmp_path: Path,
    monkeypatch,
    capsys,
) -> None:
    audit = _load_audit()
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
    monkeypatch.setattr(audit, "_measure_cpython", lambda *a, **k: None)
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
        json.loads(line)
        for line in captured.err.splitlines()
        if line.strip()
    ]
    payload = json.loads(json_out.read_text(encoding="utf-8"))
    assert rc == 0
    assert stdout_payload == payload
    assert [
        event["event"]
        for event in progress_events
    ] == [
        "output_startup_size_audit.case_start",
        "output_startup_size_audit.case_done",
        "output_startup_size_audit.baseline_start",
        "output_startup_size_audit.baseline_done",
        "output_startup_size_audit.baseline_start",
        "output_startup_size_audit.baseline_done",
    ]
    assert payload["event"] == "output_startup_size_audit"
    assert payload["summary"]["cases"] == 1
    assert payload["cases"][0]["artifact"]["bytes"] == len(b"molt-binary")
    startup = payload["cases"][0]["startup"]
    assert startup["page_cache_cold"]["mode"] == "page_cache_cold_copy"
    assert startup["cold_first_sighting"]["mode"] == "cold_first_sighting"
