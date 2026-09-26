from __future__ import annotations

import inspect
import json
from pathlib import Path

import pytest

import molt.cli as cli
from molt.cli import build_pipeline
from molt.cli import link_fingerprints, link_pipeline

_LINK_PIPELINE_NAMES = (
    "_darwin_link_validation_failure",
    "_prepare_native_link",
    "_prepare_native_object_artifact",
    "_run_native_link_command",
    "_validate_darwin_link_output",
)


def test_cli_link_pipeline_authority_is_single_home() -> None:
    for name in ("_link_fingerprint", "_link_fingerprint_path"):
        assert hasattr(link_fingerprints, name)
        assert not hasattr(link_pipeline, name)
    assert not hasattr(link_pipeline, "_run_native_partial_link_command")
    for name in _LINK_PIPELINE_NAMES:
        assert hasattr(link_pipeline, name), name
        assert not hasattr(cli, name), name
        assert not hasattr(build_pipeline, name), name

    cli_source = inspect.getsource(cli)
    build_pipeline_source = inspect.getsource(build_pipeline)
    for name in _LINK_PIPELINE_NAMES:
        assert f"def {name}(" not in cli_source
        assert f"def {name}(" not in build_pipeline_source


@pytest.mark.parametrize("location", ["external", "same-root", "missing"])
@pytest.mark.parametrize("json_output", [False, True])
def test_every_configured_stdlib_uses_locked_snapshot_admission(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys,
    location: str,
    json_output: bool,
) -> None:
    artifacts = tmp_path / "build"
    source = (tmp_path / "cache" if location == "external" else artifacts) / "stdlib.a"
    source.parent.mkdir(parents=True)
    if location != "missing":
        source.write_bytes(b"generation")
    admitted: list[Path] = []

    def stage(path: Path, **kwargs):
        admitted.append(path)
        assert kwargs["artifacts_root"] == artifacts
        assert kwargs["stdlib_object_cache_key"] == "expected"
        error = OSError("snapshot admission rejected the generation")
        error.add_note(
            "Shared stdlib staging cleanup also failed: owned archive denied"
        )
        raise error

    monkeypatch.setattr(link_pipeline, "_stage_shared_stdlib_object_for_link", stage)
    prepared, failure = link_pipeline._prepare_native_link(
        output_artifact=artifacts / "app.a",
        resolved_capability_policy=None,
        artifacts_root=artifacts,
        json_output=json_output,
        output_binary=artifacts / "app.exe",
        runtime_lib=None,
        runtime_build_identity=None,
        molt_root=tmp_path,
        runtime_cargo_profile="dev-fast",
        target_triple=None,
        sysroot_path=None,
        profile=None,
        project_root=tmp_path,
        diagnostics_enabled=False,
        phase_starts={},
        link_timeout=None,
        warnings=[],
        stdlib_obj_path=source,
        stdlib_object_cache_key="expected",
    )
    assert admitted == [source]
    assert prepared is None and failure == 2
    captured = capsys.readouterr()
    if json_output:
        payload = json.loads(captured.out)
        assert payload["status"] == "error"
        assert payload["data"]["returncode"] == 2
        assert len(payload["errors"]) == 1
        message = payload["errors"][0]
        assert not captured.err
    else:
        message = captured.err
        assert not captured.out
    assert "snapshot admission rejected the generation" in message
    assert "staging cleanup also failed: owned archive denied" in message
    assert "Traceback (most recent call last)" not in message
