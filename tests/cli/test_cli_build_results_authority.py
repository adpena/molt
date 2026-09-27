from __future__ import annotations

import inspect
import json
import subprocess
from pathlib import Path

import pytest

import molt.cli as cli
from molt.capability_manifest import CapabilityManifest
from molt.cli import build_results, link_fingerprints, progress

_BUILD_RESULTS_NAMES = (
    "_attach_build_metadata",
    "_attach_process_output",
    "_build_cache_info",
    "_build_common_build_json_data",
    "_build_native_link_error_data",
    "_build_native_link_success_data",
    "_emit_build_success_json",
    "_emit_native_link_result",
    "_emit_non_native_build_result",
    "_post_link_strip",
)

_BUILD_RESULTS_DEFINITIONS = (
    "def _attach_build_metadata(",
    "def _attach_process_output(",
    "def _build_cache_info(",
    "def _build_common_build_json_data(",
    "def _build_native_link_error_data(",
    "def _build_native_link_success_data(",
    "def _emit_build_success_json(",
    "def _emit_native_link_result(",
    "def _emit_non_native_build_result(",
    "def _post_link_strip(",
)


def test_cli_build_results_authority_is_single_home() -> None:
    assert hasattr(link_fingerprints, "publish_link_outputs")
    assert not hasattr(link_fingerprints, "_write_link_fingerprint_if_needed")
    assert not hasattr(build_results, "_write_link_fingerprint_if_needed")
    assert not hasattr(cli, "_write_link_fingerprint_if_needed")
    for name in _BUILD_RESULTS_NAMES:
        assert hasattr(build_results, name)
        assert not hasattr(cli, name)

    cli_source = inspect.getsource(cli)
    for marker in _BUILD_RESULTS_DEFINITIONS:
        assert marker not in cli_source


def _emit_link_result(
    tmp_path: Path,
    process: subprocess.CompletedProcess[str],
    *,
    json_output: bool,
) -> int:
    return build_results._emit_native_link_result(
        link_process=process,
        link_skipped=False,
        link_fingerprint=None,
        link_fingerprint_path=tmp_path / "link.json",
        cache=False,
        cache_hit=False,
        cache_key=None,
        function_cache_key=None,
        cache_path=None,
        function_cache_path=None,
        cache_hit_tier=None,
        backend_daemon_cached=None,
        backend_daemon_cache_tier=None,
        backend_daemon_config_digest=None,
        target="native",
        target_triple=None,
        source_path=tmp_path / "app.py",
        output_binary=tmp_path / "app.exe",
        deterministic=False,
        trusted=False,
        resolved_capability_policy=CapabilityManifest().resolve(),
        capabilities_source=None,
        sysroot_path=None,
        emit_mode="binary",
        profile="dev",
        native_arch_perf_enabled=False,
        output_obj=tmp_path / "app.lib",
        stub_path=tmp_path / "main_stub.c",
        runtime_lib=tmp_path / "runtime.lib",
        external_native_artifacts=(),
        diagnostics_payload=None,
        diagnostics_path=None,
        pgo_profile_payload=None,
        runtime_feedback_payload=None,
        emit_ir_path=None,
        stdlib_obj_path=None,
        warnings=[],
        json_output=json_output,
        resolved_diagnostics_verbosity="brief",
    )


@pytest.mark.parametrize(
    ("stdout", "stderr", "expected"),
    [
        (
            "driver detail",
            "lld-link: unresolved symbol\n",
            "driver detail\nlld-link: unresolved symbol\nLinking failed\n",
        ),
        ("driver-only diagnostic\n", "", "driver-only diagnostic\nLinking failed\n"),
    ],
)
def test_native_link_failure_text_preserves_captured_streams(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    stdout: str,
    stderr: str,
    expected: str,
) -> None:
    process = subprocess.CompletedProcess(["clang"], 1, stdout, stderr)
    assert _emit_link_result(tmp_path, process, json_output=False) == 1
    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err == expected


def test_native_link_failure_json_keeps_structured_streams(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    process = subprocess.CompletedProcess(
        ["clang"], 1, "driver detail", "lld-link: unresolved symbol\n"
    )
    assert _emit_link_result(tmp_path, process, json_output=True) == 1
    captured = capsys.readouterr()
    assert captured.err == ""
    payload = json.loads(captured.out)
    assert payload["errors"] == ["Linking failed"]
    assert payload["data"]["stdout"] == process.stdout
    assert payload["data"]["stderr"] == process.stderr


def test_native_link_failure_keeps_diagnostics_in_quiet_mode(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    process = subprocess.CompletedProcess(
        ["clang"], 1, "", "lld-link: missing import\n"
    )
    with progress.BuildProgress(quiet=True):
        assert _emit_link_result(tmp_path, process, json_output=False) == 1
    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err == "lld-link: missing import\nLinking failed\n"


def test_native_link_success_text_preserves_linker_warnings(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(build_results, "_assert_native_binary_valid", lambda *_: None)
    monkeypatch.setattr(
        build_results,
        "_native_artifact_binary_image_analysis_payload",
        lambda **_: {},
    )
    process = subprocess.CompletedProcess(
        ["clang"], 0, "driver warning\n", "lld-link: warning: weak symbol"
    )
    assert _emit_link_result(tmp_path, process, json_output=False) == 0
    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err == (
        "driver warning\nlld-link: warning: weak symbol\n"
        f"Successfully built {tmp_path / 'app.exe'}\n"
    )
