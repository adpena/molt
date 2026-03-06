from __future__ import annotations

import contextlib
import subprocess
from pathlib import Path

import molt.cli as cli


def test_is_valid_wasm_binary_accepts_wasm_magic(tmp_path: Path) -> None:
    artifact = tmp_path / "ok.wasm"
    artifact.write_bytes(b"\x00asm\x01\x00\x00\x00rest")
    assert cli._is_valid_wasm_binary(artifact)


def test_is_valid_wasm_binary_rejects_zero_filled_file(tmp_path: Path) -> None:
    artifact = tmp_path / "bad.wasm"
    artifact.write_bytes(b"\x00" * 32)
    assert not cli._is_valid_wasm_binary(artifact)


def test_wasm_runtime_recovery_target_root_suffix(tmp_path: Path) -> None:
    target_root = tmp_path / "cargo-target"
    assert cli._wasm_runtime_recovery_target_root(target_root) == (
        tmp_path / "cargo-target-wasm-runtime-recovery"
    )


def test_is_wasm_unsafe_volume_detects_non_native_filesystems(
    tmp_path: Path, monkeypatch
) -> None:
    path = tmp_path / "artifact-root"
    path.mkdir()

    def fake_run(
        cmd: list[str], *, capture_output: bool, text: bool, timeout: float
    ) -> subprocess.CompletedProcess[str]:
        del capture_output, text, timeout
        if cmd[0] == "df":
            return subprocess.CompletedProcess(
                cmd,
                0,
                f"Filesystem 512-blocks Used Available Capacity Mounted on\n/dev/disk9s1 1 1 1 1% {path}\n",
                "",
            )
        assert cmd[0] == "mount"
        return subprocess.CompletedProcess(
            cmd,
            0,
            "/dev/disk9s1 on /Volumes/APDataStore (exfat, local, nodev, nosuid)\n",
            "",
        )

    monkeypatch.setattr(cli.subprocess, "run", fake_run, raising=True)

    assert cli._is_wasm_unsafe_volume(path) is True


def test_ensure_runtime_wasm_redirects_unsafe_target_root_to_local(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    runtime_wasm = tmp_path / "wasm" / "molt_runtime.wasm"
    primary_target = tmp_path / "target-unsafe"
    local_target = tmp_path / "target-local"
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
    monkeypatch.setattr(
        cli,
        "_is_wasm_unsafe_volume",
        lambda path: Path(path) == primary_target,
        raising=True,
    )
    monkeypatch.setattr(
        cli, "_wasm_local_target_root", lambda target_root: local_target, raising=True
    )

    seen_target_root_override: Path | None = None

    def fake_run_runtime_wasm_cargo_build(
        *,
        cmd: list[str],
        root: Path,
        env: dict[str, str],
        cargo_timeout: float | None,
        profile_dir: str,
        target_root_override: Path | None,
        json_output: bool,
    ) -> tuple[subprocess.CompletedProcess[str], Path]:
        del root, env, cargo_timeout, json_output
        nonlocal seen_target_root_override
        seen_target_root_override = target_root_override
        src = local_target / "wasm32-wasip1" / profile_dir / "molt_runtime.wasm"
        src.parent.mkdir(parents=True, exist_ok=True)
        src.write_bytes(b"\x00asm\x01\x00\x00\x00ok")
        return subprocess.CompletedProcess(cmd, 0), src

    monkeypatch.setattr(
        cli,
        "_run_runtime_wasm_cargo_build",
        fake_run_runtime_wasm_cargo_build,
        raising=True,
    )

    assert cli._ensure_runtime_wasm(
        runtime_wasm,
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=5.0,
        project_root=project_root,
    )
    assert seen_target_root_override == local_target
    assert cli._is_valid_wasm_binary(runtime_wasm)


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
    monkeypatch.setattr(cli, "_is_wasm_unsafe_volume", lambda path: False, raising=True)

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


def test_ensure_runtime_wasm_uses_valid_deps_artifact_before_recovery(
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
    monkeypatch.setattr(cli, "_is_wasm_unsafe_volume", lambda path: False, raising=True)

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
        profile = cmd[5]
        target_root = Path(env.get("CARGO_TARGET_DIR", str(project_root / "target")))
        seen_target_roots.append(target_root)
        src = target_root / "wasm32-wasip1" / profile / "molt_runtime.wasm"
        src.parent.mkdir(parents=True, exist_ok=True)
        src.write_bytes(b"\x00" * 64)
        deps_src = src.parent / "deps" / "molt_runtime.wasm"
        deps_src.parent.mkdir(parents=True, exist_ok=True)
        deps_src.write_bytes(b"\x00asm\x01\x00\x00\x00ok")
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
    assert seen_target_roots == [primary_target]


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
    monkeypatch.setattr(cli, "_is_wasm_unsafe_volume", lambda path: False, raising=True)

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
        src = target_root / "wasm32-wasip1" / profile / "molt_runtime.wasm"
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


def test_configure_wasm_runtime_codegen_flags_uses_aggressive_defaults(
    monkeypatch,
) -> None:
    monkeypatch.delenv("MOLT_WASM_RUNTIME_TARGET_FEATURES", raising=False)
    monkeypatch.delenv("MOLT_WASM_RUNTIME_TARGET_FEATURE_MODE", raising=False)
    monkeypatch.delenv("MOLT_WASM_RUNTIME_TARGET_FEATURES_EXTRA", raising=False)
    monkeypatch.delenv("MOLT_WASM_RUNTIME_TARGET_CPU", raising=False)
    monkeypatch.delenv("MOLT_WASM_LEGACY_LINK_FLAGS", raising=False)
    env: dict[str, str] = {}

    rustflags = cli._configure_wasm_runtime_codegen_flags(env, reloc=False)

    assert env["RUSTFLAGS"] == rustflags
    target_features = cli._rustflags_codegen_values(rustflags, "target-feature")
    assert target_features
    merged = target_features[-1]
    assert "+simd128" in merged
    assert "+bulk-memory" in merged
    assert "+sign-ext" in merged
    assert "--import-memory" not in rustflags


def test_configure_wasm_runtime_codegen_flags_merges_existing_rustflags(
    monkeypatch,
) -> None:
    monkeypatch.setenv(
        "MOLT_WASM_RUNTIME_TARGET_FEATURES",
        "+simd128,+bulk-memory,+sign-ext",
    )
    monkeypatch.setenv("MOLT_WASM_RUNTIME_TARGET_FEATURES_EXTRA", "+multivalue")
    monkeypatch.setenv("MOLT_WASM_RUNTIME_TARGET_CPU", "generic")
    monkeypatch.delenv("MOLT_WASM_LEGACY_LINK_FLAGS", raising=False)
    env = {
        "RUSTFLAGS": (
            "-C target-feature=+simd128,-bulk-memory -C target-cpu=mvp -C debuginfo=1"
        )
    }

    rustflags = cli._configure_wasm_runtime_codegen_flags(env, reloc=False)

    assert env["RUSTFLAGS"] == rustflags
    target_features = cli._rustflags_codegen_values(rustflags, "target-feature")
    assert target_features
    merged = target_features[-1]
    assert "-bulk-memory" in merged
    assert "+sign-ext" in merged
    assert "+multivalue" in merged
    assert cli._rustflags_codegen_values(rustflags, "target-cpu") == ["mvp"]
