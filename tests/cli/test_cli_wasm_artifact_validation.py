from __future__ import annotations

import contextlib
import subprocess
from pathlib import Path

import molt.cli as cli


def test_is_valid_wasm_binary_accepts_wasm_magic(tmp_path: Path) -> None:
    artifact = tmp_path / "ok.wasm"
    artifact.write_bytes(b"\x00asm\x01\x00\x00\x00rest")
    assert cli._inspect_wasm_binary(artifact) == "valid"
    assert cli._is_valid_wasm_binary(artifact)


def test_is_valid_wasm_binary_rejects_zero_filled_file(tmp_path: Path) -> None:
    artifact = tmp_path / "bad.wasm"
    artifact.write_bytes(b"\x00" * 32)
    assert cli._inspect_wasm_binary(artifact) == "invalid"
    assert not cli._is_valid_wasm_binary(artifact)


def test_inspect_wasm_binary_reports_missing(tmp_path: Path) -> None:
    artifact = tmp_path / "missing.wasm"
    assert cli._inspect_wasm_binary(artifact) == "missing"


def test_wasm_runtime_recovery_target_root_suffix(tmp_path: Path) -> None:
    target_root = tmp_path / "cargo-target"
    assert cli._wasm_runtime_recovery_target_root(target_root) == (
        tmp_path / "cargo-target-wasm-runtime-recovery"
    )


def test_ensure_runtime_wasm_recovers_from_invalid_primary_artifact(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    runtime_wasm = tmp_path / "wasm" / "molt_runtime.wasm"
    primary_target = tmp_path / "target-primary"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(primary_target))
    monkeypatch.setattr(
        cli, "_runtime_fingerprint", lambda *args, **kwargs: None, raising=True
    )
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: tmp_path / "fingerprint.json",
        raising=True,
    )
    monkeypatch.setattr(
        cli, "_artifact_needs_rebuild", lambda *args, **kwargs: True, raising=True
    )
    monkeypatch.setattr(
        cli,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )

    seen_target_roots: list[Path] = []

    def fake_run(
        cmd: list[str],
        *,
        cwd: Path,
        env: dict[str, str],
        timeout: float | None,
        check: bool,
        text: bool,
    ) -> subprocess.CompletedProcess[str]:
        del cwd, timeout, check, text
        target_root = Path(env.get("CARGO_TARGET_DIR", str(project_root / "target")))
        seen_target_roots.append(target_root)
        src = target_root / "wasm32-wasip1" / "dev-fast" / "molt_runtime.wasm"
        src.parent.mkdir(parents=True, exist_ok=True)
        if len(seen_target_roots) == 1:
            src.write_bytes(b"\x00" * 64)
        else:
            src.write_bytes(b"\x00asm\x01\x00\x00\x00ok")
        return subprocess.CompletedProcess(cmd, 0)

    monkeypatch.setattr(cli.subprocess, "run", fake_run, raising=True)

    assert cli._ensure_runtime_wasm(
        runtime_wasm,
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=5.0,
        project_root=project_root,
    )
    assert cli._is_valid_wasm_binary(runtime_wasm)
    assert len(seen_target_roots) == 2
    assert seen_target_roots[0] == primary_target
    assert seen_target_roots[1] == cli._wasm_runtime_recovery_target_root(
        primary_target
    )


def test_ensure_runtime_wasm_uses_fallback_profile_when_release_artifacts_invalid(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    runtime_wasm = tmp_path / "wasm" / "molt_runtime.wasm"
    primary_target = tmp_path / "target-release"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(primary_target))
    monkeypatch.setenv("MOLT_WASM_RUNTIME_FALLBACK_PROFILE", "release-fast")
    monkeypatch.setattr(
        cli, "_runtime_fingerprint", lambda *args, **kwargs: None, raising=True
    )
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: tmp_path / "fingerprint.json",
        raising=True,
    )
    monkeypatch.setattr(
        cli, "_artifact_needs_rebuild", lambda *args, **kwargs: True, raising=True
    )
    monkeypatch.setattr(
        cli,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )

    seen_profiles: list[str] = []
    seen_targets: list[Path] = []

    def fake_run(
        cmd: list[str],
        *,
        cwd: Path,
        env: dict[str, str],
        timeout: float | None,
        check: bool,
        text: bool,
    ) -> subprocess.CompletedProcess[str]:
        del cwd, timeout, check, text
        profile = cmd[5]
        target_root = Path(env.get("CARGO_TARGET_DIR", str(project_root / "target")))
        seen_profiles.append(profile)
        seen_targets.append(target_root)
        output_profile_dir = (
            "release-fast"
            if profile == "release-fast"
            else cli._cargo_profile_dir(profile)
        )
        src = (
            target_root
            / "wasm32-wasip1"
            / output_profile_dir
            / "molt_runtime.wasm"
        )
        src.parent.mkdir(parents=True, exist_ok=True)
        if profile == "release-fast":
            src.write_bytes(b"\x00asm\x01\x00\x00\x00ok")
        else:
            src.write_bytes(b"\x00" * 64)
        return subprocess.CompletedProcess(cmd, 0)

    monkeypatch.setattr(cli.subprocess, "run", fake_run, raising=True)

    assert cli._ensure_runtime_wasm(
        runtime_wasm,
        reloc=False,
        json_output=True,
        cargo_profile="release",
        cargo_timeout=5.0,
        project_root=project_root,
    )
    assert cli._is_valid_wasm_binary(runtime_wasm)
    assert seen_profiles == ["wasm-release", "wasm-release", "release-fast"]
    assert seen_targets[0] == primary_target
    assert seen_targets[1] == cli._wasm_runtime_recovery_target_root(primary_target)
