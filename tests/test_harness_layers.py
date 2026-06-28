"""Tests for individual harness layer implementations."""

import json
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, "src")

import molt.dx as molt_dx
from molt.harness_layers import (
    LAYERS,
    get_layers_for_profile,
)
from molt.harness_report import LayerStatus


def test_run_cmd_uses_harness_memory_guard(monkeypatch, tmp_path: Path):
    import molt.harness_layers as harness_layers

    calls: list[dict[str, object]] = []

    def fake_guarded_completed_process(args, **kwargs):
        calls.append({"args": args, **kwargs})
        return harness_layers.harness_memory_guard.GuardedCompletedProcess(
            args,
            0,
            "ok\n",
            "",
            elapsed_s=0.01,
        )

    monkeypatch.setattr(
        harness_layers.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    proc = harness_layers._run_cmd(
        ["python3", "--version"],
        cwd=tmp_path,
        timeout_s=12,
        env={"MOLT_HARNESS_MAX_PROCESS_RSS_GB": "2"},
    )

    assert proc.returncode == 0
    assert proc.stdout == "ok\n"
    call = calls[0]
    assert call["args"] == ["python3", "--version"]
    assert call["prefix"] == harness_layers.HARNESS_MEMORY_PREFIX
    assert call["cwd"] == tmp_path
    assert call["capture_output"] is True
    assert call["text"] is True
    assert call["timeout"] == 12
    assert call["limits"].max_process_rss_gb == 2
    artifact_root = Path(call["env"]["MOLT_EXT_ROOT"])
    assert call["env"]["CARGO_TARGET_DIR"] == str(
        molt_dx.cargo_target_dir_for_artifact_root(
            artifact_root,
            call["env"]["MOLT_SESSION_ID"],
        )
    )
    assert call["env"]["TMPDIR"] == str(artifact_root / "tmp")


def test_harness_repo_sentinel_uses_canonical_artifact_root(
    monkeypatch, tmp_path: Path
):
    import molt.harness_layers as harness_layers

    calls: list[dict[str, object]] = []

    class FakeSentinel:
        def __enter__(self):
            calls.append({"event": "enter"})
            return self

        def __exit__(self, exc_type, exc, tb) -> None:
            calls.append({"event": "exit"})

    def fake_repo_process_sentinel(**kwargs):
        calls.append(kwargs)
        return FakeSentinel()

    monkeypatch.setattr(
        harness_layers.harness_memory_guard,
        "repo_process_sentinel",
        fake_repo_process_sentinel,
    )
    limits = harness_layers.harness_memory_limits()

    with harness_layers.harness_repo_sentinel(tmp_path, limits=limits):
        calls.append({"event": "body"})

    assert calls[0]["repo_root"] == tmp_path
    assert calls[0]["artifact_root"] == tmp_path / "tmp" / "harness"
    assert calls[0]["label"] == "molt_harness"
    assert calls[0]["limits"] is limits
    assert [call["event"] for call in calls[1:]] == ["enter", "body", "exit"]


def test_profiles_are_supersets():
    quick = {layer.name for layer in get_layers_for_profile("quick")}
    standard = {layer.name for layer in get_layers_for_profile("standard")}
    deep = {layer.name for layer in get_layers_for_profile("deep")}
    assert quick.issubset(standard)
    assert standard.issubset(deep)


def test_quick_has_four_layers():
    layers = get_layers_for_profile("quick")
    assert [layer.name for layer in layers] == [
        "compile",
        "lint",
        "unit-rust",
        "unit-python",
    ]


def test_standard_adds_four_layers():
    layers = get_layers_for_profile("standard")
    names = [layer.name for layer in layers]
    assert "wasm-compile" in names
    assert "differential" in names
    assert "resource" in names
    assert "audit" in names


def test_deep_adds_remaining_layers():
    layers = get_layers_for_profile("deep")
    names = [layer.name for layer in layers]
    for expected in [
        "fuzz",
        "conformance",
        "bench",
        "size",
        "mutation",
        "determinism",
        "miri",
        "compile-fail",
    ]:
        assert expected in names, f"missing layer: {expected}"


def test_layer_definitions_have_required_fields():
    for layer in LAYERS:
        assert layer.name, "layer must have a name"
        assert layer.profile in ("quick", "standard", "deep"), (
            f"bad profile: {layer.profile}"
        )
        assert callable(layer.run_fn), f"layer {layer.name} must have a callable run_fn"


def test_conformance_layer_uses_runner_full_suite_and_json_summary(monkeypatch):
    import molt.harness_layers as harness_layers

    calls: list[list[str]] = []

    def fake_run_cmd(args, *, cwd=None, timeout_s=300, env=None):
        calls.append(args)
        summary_path = Path(args[args.index("--json-out") + 1])
        summary_path.parent.mkdir(parents=True, exist_ok=True)
        summary_path.write_text(
            json.dumps(
                {
                    "suite": "full",
                    "manifest_path": None,
                    "corpus_root": "tests/harness/corpus/monty_compat",
                    "duration_s": 12.0,
                    "total": 20,
                    "passed": 10,
                    "failed": 2,
                    "compile_error": 3,
                    "timeout": 1,
                    "skipped": 4,
                    "failures": [{"path": "bad.py", "detail": "expected exit 0"}],
                    "compile_errors": [{"path": "cerr.py", "detail": "compile failed"}],
                    "timeouts": ["slow.py"],
                }
            ),
            encoding="utf-8",
        )
        return subprocess.CompletedProcess(
            args=args, returncode=1, stdout="", stderr=""
        )

    monkeypatch.setattr(harness_layers, "_run_cmd", fake_run_cmd)

    result = harness_layers.run_layer_conformance(
        harness_layers.HarnessConfig(project_root=Path("."))
    )

    assert Path(calls[0][1]).name == "run_molt_conformance.py"
    assert calls[0][2:4] == ["--suite", "full"]
    assert "--json-out" in calls[0]
    assert result.status == LayerStatus.FAIL
    assert result.metrics == {
        "test_count": 20,
        "pass_count": 10,
        "fail_count": 2,
        "compile_error_count": 3,
        "timeout_count": 1,
        "skip_count": 4,
        "executed_count": 12,
        "pass_rate": 10 / 12,
        "duration_s": 12.0,
    }
    assert "3 compile errors" in result.details
    assert "1 timeout" in result.details


def test_conformance_layer_passes_only_when_json_summary_is_clean(monkeypatch):
    import molt.harness_layers as harness_layers

    def fake_run_cmd(args, *, cwd=None, timeout_s=300, env=None):
        summary_path = Path(args[args.index("--json-out") + 1])
        summary_path.parent.mkdir(parents=True, exist_ok=True)
        summary_path.write_text(
            json.dumps(
                {
                    "suite": "full",
                    "manifest_path": None,
                    "corpus_root": "tests/harness/corpus/monty_compat",
                    "duration_s": 4.0,
                    "total": 29,
                    "passed": 24,
                    "failed": 0,
                    "compile_error": 0,
                    "timeout": 0,
                    "skipped": 5,
                    "failures": [],
                    "compile_errors": [],
                    "timeouts": [],
                }
            ),
            encoding="utf-8",
        )
        return subprocess.CompletedProcess(
            args=args, returncode=0, stdout="", stderr=""
        )

    monkeypatch.setattr(harness_layers, "_run_cmd", fake_run_cmd)

    result = harness_layers.run_layer_conformance(
        harness_layers.HarnessConfig(project_root=Path("."))
    )

    assert result.status == LayerStatus.PASS
    assert result.metrics["test_count"] == 29
    assert result.metrics["pass_count"] == 24
    assert result.metrics["skip_count"] == 5
    assert result.metrics["duration_s"] == 4.0


def test_conformance_layer_passes_molt_cmd_and_uses_extended_timeout(monkeypatch):
    import molt.harness_layers as harness_layers

    captured: dict[str, object] = {}

    def fake_run_cmd(args, *, cwd=None, timeout_s=300, env=None):
        captured["args"] = args
        captured["timeout_s"] = timeout_s
        captured["env"] = env
        summary_path = Path(args[args.index("--json-out") + 1])
        summary_path.parent.mkdir(parents=True, exist_ok=True)
        summary_path.write_text(
            json.dumps(
                {
                    "suite": "full",
                    "manifest_path": None,
                    "corpus_root": "tests/harness/corpus/monty_compat",
                    "duration_s": 1.0,
                    "total": 1,
                    "passed": 1,
                    "failed": 0,
                    "compile_error": 0,
                    "timeout": 0,
                    "skipped": 0,
                    "failures": [],
                    "compile_errors": [],
                    "timeouts": [],
                }
            ),
            encoding="utf-8",
        )
        return subprocess.CompletedProcess(
            args=args, returncode=0, stdout="", stderr=""
        )

    monkeypatch.setattr(harness_layers, "_run_cmd", fake_run_cmd)

    harness_layers.run_layer_conformance(
        harness_layers.HarnessConfig(project_root=Path("."), molt_cmd="custom-molt")
    )

    assert captured["env"] == {"MOLT_BIN": "custom-molt"}
    assert captured["timeout_s"] == harness_layers.CONFORMANCE_LAYER_TIMEOUT_S
