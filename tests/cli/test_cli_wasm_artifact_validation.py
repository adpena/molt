from __future__ import annotations

import contextlib
import os
import shutil
import subprocess
from pathlib import Path

import pytest
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


@pytest.mark.slow
def test_ensure_runtime_reloc_wasm_exports_wasi_clock_ids(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo required")
    wasm_objdump = shutil.which("wasm-objdump")
    if wasm_objdump is None:
        pytest.skip("wasm-objdump required")

    project_root = Path(__file__).resolve().parents[2]
    runtime_reloc = tmp_path / "wasm" / "molt_runtime_reloc.wasm"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "target"))
    monkeypatch.setenv("MOLT_BACKEND_DAEMON", "0")

    assert cli._ensure_runtime_wasm(
        runtime_reloc,
        reloc=True,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=300.0,
        project_root=project_root,
    )

    exports = subprocess.check_output(
        [wasm_objdump, "-x", str(runtime_reloc)],
        text=True,
        cwd=project_root,
    )
    assert "D <_CLOCK_PROCESS_CPUTIME_ID> [ undefined" not in exports
    assert "D <_CLOCK_THREAD_CPUTIME_ID> [ undefined" not in exports
    assert "D <_CLOCK_PROCESS_CPUTIME_ID>" in exports
    assert "D <_CLOCK_THREAD_CPUTIME_ID>" in exports


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

    def fake_run_runtime_wasm_cargo_build(
        *,
        cmd: list[str],
        root: Path,
        env: dict[str, str],
        cargo_timeout: float | None,
        profile_dir: str,
        target_root_override: Path | None = None,
        json_output: bool,
        artifact_kind: str = "cdylib",
    ) -> tuple[subprocess.CompletedProcess[str], Path]:
        del env, cargo_timeout, json_output, artifact_kind
        target_root = target_root_override or cli._cargo_target_root(root)
        seen_target_roots.append(target_root)
        src = target_root / "wasm32-wasip1" / profile_dir / "molt_runtime.wasm"
        src.parent.mkdir(parents=True, exist_ok=True)
        if len(seen_target_roots) == 1:
            src.write_bytes(b"\x00" * 64)
        else:
            src.write_bytes(b"\x00asm\x01\x00\x00\x00ok")
        return subprocess.CompletedProcess(cmd, 0, "", ""), src

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

    def fake_run_runtime_wasm_cargo_build(
        *,
        cmd: list[str],
        root: Path,
        env: dict[str, str],
        cargo_timeout: float | None,
        profile_dir: str,
        target_root_override: Path | None = None,
        json_output: bool,
        artifact_kind: str = "cdylib",
    ) -> tuple[subprocess.CompletedProcess[str], Path]:
        del env, cargo_timeout, json_output, artifact_kind
        profile = cmd[5]
        target_root = target_root_override or cli._cargo_target_root(root)
        seen_profiles.append(profile)
        seen_targets.append(target_root)
        src = target_root / "wasm32-wasip1" / profile_dir / "molt_runtime.wasm"
        src.parent.mkdir(parents=True, exist_ok=True)
        if profile == "release-fast":
            src.write_bytes(b"\x00asm\x01\x00\x00\x00ok")
        else:
            src.write_bytes(b"\x00" * 64)
        return subprocess.CompletedProcess(cmd, 0, "", ""), src

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
        cargo_profile="release",
        cargo_timeout=5.0,
        project_root=project_root,
    )
    assert cli._is_valid_wasm_binary(runtime_wasm)
    assert seen_profiles == ["wasm-release", "wasm-release", "release-fast"]
    assert seen_targets[0] == primary_target
    assert seen_targets[1] == cli._wasm_runtime_recovery_target_root(primary_target)


def test_ensure_runtime_wasm_rebuilds_when_feature_shape_changes_even_if_artifact_is_newer(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    runtime_wasm = tmp_path / "wasm" / "molt_runtime.wasm"
    runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
    runtime_wasm.write_bytes(b"\x00asm\x01\x00\x00\x00old")

    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint",
        lambda *args, **kwargs: {"hash": "new-shape"},
        raising=True,
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
        cli, "_artifact_newer_than_sources", lambda *args, **kwargs: True, raising=True
    )
    monkeypatch.setattr(
        cli,
        "_is_valid_runtime_wasm_artifact",
        lambda *args, **kwargs: True,
        raising=True,
    )
    monkeypatch.setattr(
        cli,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )

    build_calls: list[tuple[tuple[str, ...], Path]] = []

    def fake_run_runtime_wasm_cargo_build(
        *,
        cmd: list[str],
        root: Path,
        env: dict[str, str],
        cargo_timeout: float | None,
        profile_dir: str,
        target_root_override: Path | None = None,
        json_output: bool,
        artifact_kind: str = "cdylib",
    ) -> tuple[subprocess.CompletedProcess[str], Path]:
        del cargo_timeout, profile_dir, json_output, artifact_kind
        target_root = target_root_override or cli._cargo_target_root(root)
        src = target_root / "wasm32-wasip1" / "dev-fast" / "molt_runtime.wasm"
        src.parent.mkdir(parents=True, exist_ok=True)
        src.write_bytes(b"\x00asm\x01\x00\x00\x00rebuilt")
        build_calls.append((tuple(cmd), src))
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
        stdlib_profile="micro",
        resolved_modules={"ssl"},
    )
    assert build_calls, "feature-shape changes must force a wasm runtime rebuild"
    assert runtime_wasm.read_bytes() == b"\x00asm\x01\x00\x00\x00rebuilt"


def test_ensure_runtime_wasm_skip_rebuild_still_requires_requested_exports(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    runtime_wasm = tmp_path / "wasm" / "molt_runtime.wasm"
    runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
    runtime_wasm.write_bytes(b"\x00asm\x01\x00\x00\x00runtime")

    monkeypatch.setenv("MOLT_SKIP_RUNTIME_REBUILD", "1")
    monkeypatch.setattr(
        cli,
        "_runtime_wasm_exports_satisfy",
        lambda path, required: False,
        raising=True,
    )
    monkeypatch.setattr(
        cli,
        "_is_valid_runtime_wasm_artifact",
        lambda path: True,
        raising=True,
    )

    assert not cli._ensure_runtime_wasm(
        runtime_wasm,
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=5.0,
        project_root=project_root,
        required_exports={"molt_fast_list_append"},
    )


def test_run_subprocess_captured_to_tempfiles_respects_cwd(tmp_path: Path) -> None:
    workdir = tmp_path / "work"
    workdir.mkdir()
    result = cli._run_subprocess_captured_to_tempfiles(
        [
            "python3",
            "-c",
            "import os,sys; sys.stdout.write(os.getcwd())",
        ],
        cwd=workdir,
    )
    assert result.returncode == 0
    assert os.path.samefile(result.stdout.decode("utf-8"), workdir)


def test_runtime_fingerprint_recomputes_when_rustflags_change(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()

    monkeypatch.setattr(cli, "_runtime_source_paths", lambda _root: (), raising=True)
    monkeypatch.setattr(
        cli,
        "_hash_source_tree_metadata",
        lambda *args, **kwargs: ("same-inputs", 0),
        raising=True,
    )
    monkeypatch.setattr(cli, "_rustc_version", lambda: "rustc test", raising=True)

    first = cli._runtime_fingerprint(
        project_root,
        cargo_profile="dev-fast",
        target_triple="wasm32-wasip1",
        rustflags="-C link-arg=--export-if-defined=molt_a",
        runtime_features=("stdlib_micro",),
        stored_fingerprint=None,
    )
    assert first is not None

    second = cli._runtime_fingerprint(
        project_root,
        cargo_profile="dev-fast",
        target_triple="wasm32-wasip1",
        rustflags="-C link-arg=--export-if-defined=molt_b",
        runtime_features=("stdlib_micro",),
        stored_fingerprint=first,
    )
    assert second is not None
    assert second["hash"] != first["hash"]


def test_backend_fingerprint_recomputes_when_rustflags_change(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()

    monkeypatch.setattr(cli, "_backend_source_paths", lambda *_args: (), raising=True)
    monkeypatch.setattr(
        cli,
        "_hash_source_tree_metadata",
        lambda *args, **kwargs: ("same-inputs", 0),
        raising=True,
    )
    monkeypatch.setattr(cli, "_rustc_version", lambda: "rustc test", raising=True)

    first = cli._backend_fingerprint(
        project_root,
        cargo_profile="dev-fast",
        rustflags="-C link-arg=--export-if-defined=molt_a",
        backend_features=("wasm-backend",),
        stored_fingerprint=None,
    )
    assert first is not None

    second = cli._backend_fingerprint(
        project_root,
        cargo_profile="dev-fast",
        rustflags="-C link-arg=--export-if-defined=molt_b",
        backend_features=("wasm-backend",),
        stored_fingerprint=first,
    )
    assert second is not None
    assert second["hash"] != first["hash"]


def test_ensure_runtime_wasm_reloc_requests_staticlib_build(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    runtime_wasm = tmp_path / "wasm" / "molt_runtime_reloc.wasm"
    target_root = tmp_path / "target"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
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
        cli, "_write_runtime_fingerprint", lambda *args, **kwargs: None, raising=True
    )
    monkeypatch.setattr(
        cli,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )

    captured: dict[str, object] = {}

    def fake_run_runtime_wasm_cargo_build(
        *,
        cmd: list[str],
        root: Path,
        env: dict[str, str],
        cargo_timeout: float | None,
        profile_dir: str,
        target_root_override: Path | None = None,
        json_output: bool,
        artifact_kind: str = "cdylib",
    ) -> tuple[subprocess.CompletedProcess[str], Path]:
        del cargo_timeout, json_output
        captured["cmd"] = list(cmd)
        captured["root"] = root
        captured["env"] = dict(env)
        captured["profile_dir"] = profile_dir
        captured["artifact_kind"] = artifact_kind
        captured["target_root_override"] = target_root_override
        effective_target_root = target_root_override or cli._cargo_target_root(root)
        staticlib_path = (
            effective_target_root / "wasm32-wasip1" / profile_dir / "libmolt_runtime.a"
        )
        staticlib_path.parent.mkdir(parents=True, exist_ok=True)
        staticlib_path.write_bytes(b"archive")
        return subprocess.CompletedProcess(cmd, 0, "", ""), staticlib_path

    def fake_link_runtime_staticlib_to_reloc_wasm(
        *,
        staticlib_path: Path,
        output_path: Path,
        json_output: bool,
        link_timeout: float | None,
        export_link_args: str = "",
    ) -> bool:
        del json_output, link_timeout, export_link_args
        captured["linked_staticlib_path"] = staticlib_path
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_bytes(b"\x00asm\x01\x00\x00\x00reloc")
        return True

    monkeypatch.setattr(
        cli,
        "_run_runtime_wasm_cargo_build",
        fake_run_runtime_wasm_cargo_build,
        raising=True,
    )
    monkeypatch.setattr(
        cli,
        "_link_runtime_staticlib_to_reloc_wasm",
        fake_link_runtime_staticlib_to_reloc_wasm,
        raising=True,
    )

    assert cli._ensure_runtime_wasm(
        runtime_wasm,
        reloc=True,
        json_output=True,
        cargo_profile="release-fast",
        cargo_timeout=5.0,
        project_root=project_root,
        stdlib_profile="micro",
        resolved_modules={"__main__", "math", "sys", "builtins"},
    )
    assert captured["artifact_kind"] == "staticlib"
    assert captured["profile_dir"] == "release-fast"
    cmd = captured["cmd"]
    assert cmd[:2] == ["cargo", "rustc"]
    assert "--lib" in cmd
    assert "--crate-type=staticlib" in cmd
    assert captured["linked_staticlib_path"] == (
        target_root / "wasm32-wasip1" / "release-fast" / "libmolt_runtime.a"
    )
    assert runtime_wasm.read_bytes() == b"\x00asm\x01\x00\x00\x00reloc"


def test_link_runtime_staticlib_to_reloc_wasm_does_not_whole_archive_libc(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    staticlib = tmp_path / "libmolt_runtime.a"
    staticlib.write_bytes(b"archive")
    runtime_wasm = tmp_path / "molt_runtime_reloc.wasm"
    libc_archive = tmp_path / "libc.a"
    libc_archive.write_bytes(b"libc")
    captured: dict[str, object] = {}

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = list(cmd)
        runtime_wasm.write_bytes(b"\0asm\x01\0\0\0reloc")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli.shutil, "which", lambda name: "/usr/bin/wasm-ld")
    monkeypatch.setattr(
        cli, "_wasm_wasi_libc_archive", lambda: libc_archive, raising=True
    )
    monkeypatch.setattr(cli.subprocess, "run", fake_run, raising=True)
    monkeypatch.setattr(
        cli, "_is_valid_runtime_wasm_artifact", lambda path: True, raising=True
    )

    assert cli._link_runtime_staticlib_to_reloc_wasm(
        staticlib_path=staticlib,
        output_path=runtime_wasm,
        json_output=True,
        link_timeout=5.0,
    )

    cmd = captured["cmd"]
    assert cmd[:4] == ["/usr/bin/wasm-ld", "-r", "--whole-archive", str(staticlib)]
    assert "--no-whole-archive" in cmd
    no_whole_index = cmd.index("--no-whole-archive")
    assert cmd[no_whole_index + 1] == str(libc_archive)
