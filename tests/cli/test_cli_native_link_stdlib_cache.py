from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

import molt.cli as cli
from molt.capability_manifest import CapabilityManifest
from molt.cli import link_pipeline as cli_link_pipeline
from molt.cli import native_link_command
from molt.cli.native_link_plan import (
    NativeArtifactKind,
    native_artifact_link_arguments,
    resolve_native_target_spec,
)
from tests.cli.native_link_test_support import (
    static_archive_bytes,
    write_test_native_link_manifest,
    write_test_static_archive,
)
from tests.native_artifact_fixtures import native_relocatable_object


@pytest.fixture(
    params=(
        "x86_64-pc-windows-msvc",
        "x86_64-pc-windows-gnu",
        "x86_64-unknown-linux-gnu",
        "aarch64-apple-darwin",
    )
)
def link_target(request: pytest.FixtureRequest, monkeypatch: pytest.MonkeyPatch) -> str:
    target_triple: str = request.param
    # Retain production link planning; these snapshot tests do not discover or
    # execute a compiler, nor content-identify installed linker tools.
    monkeypatch.setattr(
        native_link_command,
        "_build_native_link_driver_command",
        lambda **kwargs: (["clang", "-target", target_triple], None, target_triple),
    )
    monkeypatch.setattr(
        cli_link_pipeline, "native_link_cache_tool_facts", lambda plan: []
    )
    return target_triple


@pytest.fixture
def stdlib_archive(link_target: str) -> bytes:
    return static_archive_bytes(
        native_relocatable_object(target_triple=link_target, symbols=("molt_init_sys",))
    )


def _assert_snapshot_link_operand(
    command: list[str], *, staged: Path, source: Path, target_triple: str
) -> None:
    target = resolve_native_target_spec(target_triple)
    for path, expected in ((staged, True), (source, False)):
        operand = native_artifact_link_arguments(
            path, kind=NativeArtifactKind.ARCHIVE, target=target
        )
        occurrences = sum(
            tuple(command[index : index + len(operand)]) == operand
            for index in range(len(command) - len(operand) + 1)
        )
        assert occurrences == int(expected), (path, operand, command)


def _write_complete_stdlib_contract(
    stdlib_obj: Path, cache_key: str, target_triple: str
) -> str:
    manifest = cli._shared_stdlib_manifest(
        cache_key=cache_key,
        cache_variant="test",
        target_triple=target_triple,
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
        '{"body_hash":"test","function_count":1,"functions":["molt_init_sys"],"schema":"stdlib-partition-v2-exact-linkage-abi"}\n',
        encoding="utf-8",
    )
    cli._stdlib_object_digest_sidecar_path(stdlib_obj).write_text(
        cli._sha256_file(stdlib_obj) + "\n", encoding="utf-8"
    )
    return manifest


def test_prepare_native_link_keeps_current_keyed_stdlib_when_runtime_is_newer(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    link_target: str,
    stdlib_archive: bytes,
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    (project_root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")

    output_obj = tmp_path / "output.a"
    write_test_static_archive(output_obj)
    runtime_lib = tmp_path / "explicit-target" / "release" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True)
    write_test_static_archive(runtime_lib)
    runtime_build_identity = write_test_native_link_manifest(
        runtime_lib, target_triple=link_target
    )
    output_binary = tmp_path / "app"
    stdlib_obj = tmp_path / "stdlib_shared.a"
    stdlib_obj.write_bytes(stdlib_archive)
    stdlib_manifest = _write_complete_stdlib_contract(
        stdlib_obj, "stdlib-key", link_target
    )
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
    monkeypatch.setattr(
        cli_link_pipeline, "_artifact_needs_rebuild", lambda *args, **kwargs: True
    )
    monkeypatch.setattr(
        cli_link_pipeline, "_run_native_link_command", fake_run_native_link_command
    )

    prepared, error = cli_link_pipeline._prepare_native_link(
        output_artifact=output_obj,
        resolved_capability_policy=CapabilityManifest().resolve(),
        artifacts_root=artifacts_root,
        json_output=False,
        output_binary=output_binary,
        runtime_lib=runtime_lib,
        runtime_build_identity=runtime_build_identity,
        molt_root=project_root,
        runtime_cargo_profile="dev-fast",
        target_triple=link_target,
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
    staged_stdlib = artifacts_root / "shared-stdlib-link" / stdlib_obj.name
    _assert_snapshot_link_operand(
        captured_link_cmd,
        staged=staged_stdlib,
        source=stdlib_obj,
        target_triple=link_target,
    )
    assert staged_stdlib.read_bytes() == stdlib_obj.read_bytes() == stdlib_archive


def test_prepare_native_link_snapshots_same_root_stdlib_input(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    link_target: str,
    stdlib_archive: bytes,
) -> None:
    project_root = tmp_path / "project"
    project_root.mkdir()
    (project_root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")

    output_obj = tmp_path / "output.a"
    write_test_static_archive(output_obj)
    runtime_lib = tmp_path / "explicit-target" / "release" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True)
    write_test_static_archive(runtime_lib)
    runtime_build_identity = write_test_native_link_manifest(
        runtime_lib, target_triple=link_target
    )
    output_binary = tmp_path / "app"
    artifacts_root = tmp_path / "artifacts"
    artifacts_root.mkdir()
    stdlib_obj = artifacts_root / "stdlib_shared.a"
    stdlib_obj.write_bytes(stdlib_archive)
    stdlib_manifest = _write_complete_stdlib_contract(
        stdlib_obj, "stdlib-key", link_target
    )

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
    monkeypatch.setattr(
        cli_link_pipeline, "_artifact_needs_rebuild", lambda *args, **kwargs: True
    )
    monkeypatch.setattr(
        cli_link_pipeline, "_run_native_link_command", fake_run_native_link_command
    )

    prepared, error = cli_link_pipeline._prepare_native_link(
        output_artifact=output_obj,
        resolved_capability_policy=CapabilityManifest().resolve(),
        artifacts_root=artifacts_root,
        json_output=False,
        output_binary=output_binary,
        runtime_lib=runtime_lib,
        runtime_build_identity=runtime_build_identity,
        molt_root=project_root,
        runtime_cargo_profile="dev-fast",
        target_triple=link_target,
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
    staged_stdlib = artifacts_root / "shared-stdlib-link" / stdlib_obj.name
    _assert_snapshot_link_operand(
        captured_link_cmd,
        staged=staged_stdlib,
        source=stdlib_obj,
        target_triple=link_target,
    )
    assert staged_stdlib.read_bytes() == stdlib_obj.read_bytes() == stdlib_archive
