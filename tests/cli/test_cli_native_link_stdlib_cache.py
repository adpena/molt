from __future__ import annotations

import os
import subprocess
from pathlib import Path

import molt.cli as cli
from molt.cli import build_pipeline as cli_build_pipeline


def _write_complete_stdlib_contract(stdlib_obj: Path, cache_key: str) -> str:
    manifest = cli._shared_stdlib_manifest(
        cache_key=cache_key,
        cache_variant="test",
        target_triple=None,
        compiler_fingerprint="test",
    )
    assert manifest is not None
    cli._stdlib_object_key_sidecar_path(stdlib_obj).write_text(
        f"{cache_key}\n", encoding="utf-8"
    )
    cli._stdlib_object_manifest_sidecar_path(stdlib_obj).write_text(
        manifest + "\n", encoding="utf-8"
    )
    cli._stdlib_object_partition_manifest_sidecar_path(stdlib_obj).write_text(
        '{"body_hash":"test","function_count":1,"functions":["molt_init_sys"],"schema":"stdlib-partition-v1"}\n',
        encoding="utf-8",
    )
    cli._stdlib_object_digest_sidecar_path(stdlib_obj).write_text(
        cli._sha256_file(stdlib_obj) + "\n", encoding="utf-8"
    )
    return manifest


def test_prepare_native_link_keeps_current_keyed_stdlib_when_runtime_is_newer(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    (project_root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")

    output_obj = tmp_path / "output.o"
    output_obj.write_bytes(b"\x7fELFobject")
    runtime_lib = tmp_path / "explicit-target" / "release" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True)
    runtime_lib.write_bytes(b"archive")
    output_binary = tmp_path / "app"
    stdlib_obj = tmp_path / "stdlib_shared.o"
    stdlib_obj.write_bytes(b"stdlib")
    stdlib_manifest = _write_complete_stdlib_contract(stdlib_obj, "stdlib-key")
    artifacts_root = tmp_path / "artifacts"
    artifacts_root.mkdir()

    monkeypatch.setenv("MOLT_SESSION_ID", "alpha/session:beta")
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "explicit-target"))

    os.utime(stdlib_obj, (2, 2))
    os.utime(runtime_lib, (3, 3))

    captured_link_cmd: list[str] = []

    def fake_run_native_link_command(
        *,
        link_cmd: list[str],
        json_output: bool,
        link_timeout: float | None,
    ) -> subprocess.CompletedProcess[str]:
        del json_output, link_timeout
        captured_link_cmd[:] = link_cmd
        return subprocess.CompletedProcess(link_cmd, 0, "", "")

    monkeypatch.setattr(cli, "_read_runtime_fingerprint", lambda path: None)
    monkeypatch.setattr(cli_build_pipeline, "_artifact_needs_rebuild", lambda *args, **kwargs: True)
    monkeypatch.setattr(cli_build_pipeline, "_run_native_link_command", fake_run_native_link_command)

    prepared, error = cli_build_pipeline._prepare_native_link(
        output_artifact=output_obj,
        trusted=False,
        capabilities_list=None,
        artifacts_root=artifacts_root,
        json_output=False,
        output_binary=output_binary,
        runtime_lib=runtime_lib,
        molt_root=project_root,
        runtime_cargo_profile="dev-fast",
        target_triple=None,
        sysroot_path=None,
        profile="dev",
        project_root=project_root,
        diagnostics_enabled=False,
        phase_starts={},
        link_timeout=None,
        warnings=[],
        stdlib_obj_path=stdlib_obj,
        stdlib_object_cache_key="stdlib-key",
        stdlib_object_manifest=stdlib_manifest,
    )

    assert error is None
    assert prepared is not None
    staged_stdlib = artifacts_root / stdlib_obj.name
    assert str(staged_stdlib) in captured_link_cmd
    assert staged_stdlib.read_bytes() == b"stdlib"


def test_prepare_native_link_uses_pre_staged_stdlib_copy(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    (project_root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")

    output_obj = tmp_path / "output.o"
    output_obj.write_bytes(b"\x7fELFobject")
    runtime_lib = tmp_path / "explicit-target" / "release" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True)
    runtime_lib.write_bytes(b"archive")
    output_binary = tmp_path / "app"
    artifacts_root = tmp_path / "artifacts"
    artifacts_root.mkdir()
    stdlib_obj = artifacts_root / "stdlib_shared.o"
    stdlib_obj.write_bytes(b"stdlib")
    stdlib_manifest = _write_complete_stdlib_contract(stdlib_obj, "stdlib-key")

    captured_link_cmd: list[str] = []

    def fake_run_native_link_command(
        *,
        link_cmd: list[str],
        json_output: bool,
        link_timeout: float | None,
    ) -> subprocess.CompletedProcess[str]:
        del json_output, link_timeout
        captured_link_cmd[:] = link_cmd
        return subprocess.CompletedProcess(link_cmd, 0, "", "")

    monkeypatch.setattr(cli, "_read_runtime_fingerprint", lambda path: None)
    monkeypatch.setattr(cli_build_pipeline, "_artifact_needs_rebuild", lambda *args, **kwargs: True)
    monkeypatch.setattr(cli_build_pipeline, "_run_native_link_command", fake_run_native_link_command)

    prepared, error = cli_build_pipeline._prepare_native_link(
        output_artifact=output_obj,
        trusted=False,
        capabilities_list=None,
        artifacts_root=artifacts_root,
        json_output=False,
        output_binary=output_binary,
        runtime_lib=runtime_lib,
        molt_root=project_root,
        runtime_cargo_profile="dev-fast",
        target_triple=None,
        sysroot_path=None,
        profile="dev",
        project_root=project_root,
        diagnostics_enabled=False,
        phase_starts={},
        link_timeout=None,
        warnings=[],
        stdlib_obj_path=stdlib_obj,
        stdlib_object_cache_key="stdlib-key",
        stdlib_object_manifest=stdlib_manifest,
    )

    assert error is None
    assert prepared is not None
    assert str(stdlib_obj) in captured_link_cmd
