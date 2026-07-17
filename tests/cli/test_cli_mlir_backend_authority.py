from __future__ import annotations

import inspect
import os
from pathlib import Path

import molt.cli as cli
from molt.cli import mlir_backend

_MLIR_BACKEND_NAMES = (
    "_ensure_mlir_backend_binary",
    "_find_mlir_backend_binary",
    "_mlir_backend_executable_name",
    "_run_mlir_backend_pipeline",
)


def _backend_name() -> str:
    return "molt-backend-mlir.exe" if os.name == "nt" else "molt-backend-mlir"


def test_cli_mlir_backend_authority_is_single_home() -> None:
    for name in _MLIR_BACKEND_NAMES:
        assert getattr(cli, name) is getattr(mlir_backend, name)

    cli_source = inspect.getsource(cli)
    for name in _MLIR_BACKEND_NAMES:
        assert f"def {name}(" not in cli_source


def test_find_mlir_backend_binary_prefers_crate_release_build(tmp_path: Path) -> None:
    backend = (
        tmp_path
        / "runtime"
        / "molt-backend-mlir"
        / "target"
        / "release"
        / _backend_name()
    )
    backend.parent.mkdir(parents=True)
    backend.write_text("")

    assert mlir_backend._find_mlir_backend_binary(tmp_path) == backend


def test_find_mlir_backend_binary_uses_session_target_before_default(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setenv("MOLT_SESSION_ID", "agent-a")
    session_backend = tmp_path / "target-agent-a" / "debug" / _backend_name()
    default_backend = tmp_path / "target" / "release" / _backend_name()
    session_backend.parent.mkdir(parents=True)
    default_backend.parent.mkdir(parents=True)
    session_backend.write_text("")
    default_backend.write_text("")

    assert mlir_backend._find_mlir_backend_binary(tmp_path) == session_backend


def test_ensure_mlir_backend_builds_once_with_canonical_environment(
    tmp_path: Path,
    monkeypatch,
) -> None:
    manifest = tmp_path / "runtime" / "molt-backend-mlir" / "Cargo.toml"
    manifest.parent.mkdir(parents=True)
    manifest.write_text("[workspace]\n[package]\nname='m'\nversion='0.0.0'\n")
    backend = manifest.parent / "target" / "release" / _backend_name()
    captured: dict[str, object] = {}

    monkeypatch.setattr(
        mlir_backend.shutil,
        "which",
        lambda name: "C:/bin/cargo.exe" if name == "cargo" else None,
    )
    monkeypatch.setattr(
        mlir_backend,
        "mlir_toolchain_environment",
        lambda root: {"MOLT_LLVM_PREFIX": "C:/LLVM"},
    )

    def fake_run(command: list[str], **kwargs: object):
        captured["command"] = command
        captured["kwargs"] = kwargs
        backend.parent.mkdir(parents=True)
        backend.write_text("")
        return mlir_backend.subprocess.CompletedProcess(command, 0, b"", b"")

    monkeypatch.setattr(
        mlir_backend,
        "_run_subprocess_captured_to_tempfiles",
        fake_run,
    )

    resolved, error = mlir_backend._ensure_mlir_backend_binary(tmp_path)

    assert error is None
    assert resolved == backend
    assert captured["command"] == [
        "C:/bin/cargo.exe",
        "build",
        "--locked",
        "--release",
        "--manifest-path",
        str(manifest),
    ]
    kwargs = captured["kwargs"]
    assert isinstance(kwargs, dict)
    assert kwargs["env"] == {"MOLT_LLVM_PREFIX": "C:/LLVM"}
    assert kwargs["timeout"] == 1800


def test_mlir_backend_executable_name_is_host_specific() -> None:
    assert mlir_backend._mlir_backend_executable_name(os_name="nt").endswith(".exe")
    assert mlir_backend._mlir_backend_executable_name(os_name="posix") == "molt-backend-mlir"
