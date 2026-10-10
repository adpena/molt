from __future__ import annotations

import inspect
import os
from pathlib import Path

import pytest

import molt.cli as cli
from molt.cli import mlir_backend
import shutil
from tests.process_guard_common import install_module_view

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


def _place_backend(path: Path) -> Path:
    path.parent.mkdir(parents=True)
    path.write_text("", encoding="utf-8")
    return path


def _no_backend_on_path(monkeypatch) -> None:
    install_module_view(
        monkeypatch, "shutil", shutil, mlir_backend, which=lambda _: None
    )


@pytest.mark.usefixtures("developer_host_context")
def test_find_mlir_backend_binary_searches_only_the_project_target(
    tmp_path: Path,
    monkeypatch,
) -> None:
    _no_backend_on_path(monkeypatch)
    # Cargo's own default for the standalone workspace is not where Molt builds.
    _place_backend(
        tmp_path
        / "runtime"
        / "molt-backend-mlir"
        / "target"
        / "release"
        / _backend_name()
    )
    assert mlir_backend._find_mlir_backend_binary(tmp_path) is None

    backend = _place_backend(tmp_path / "target" / "release" / _backend_name())
    assert mlir_backend._find_mlir_backend_binary(tmp_path) == backend


@pytest.mark.usefixtures("developer_host_context")
def test_find_mlir_backend_binary_follows_a_pinned_session_target(
    tmp_path: Path,
    monkeypatch,
) -> None:
    _no_backend_on_path(monkeypatch)
    monkeypatch.setenv("MOLT_SESSION_ID", "agent-a")
    _place_backend(tmp_path / "target" / "release" / _backend_name())
    _place_backend(tmp_path / "target-agent-a" / "release" / _backend_name())
    session_backend = _place_backend(
        tmp_path / "target" / "sessions" / "agent-a" / "debug" / _backend_name()
    )

    assert mlir_backend._find_mlir_backend_binary(tmp_path) == session_backend


@pytest.mark.usefixtures("developer_host_context")
def test_ensure_mlir_backend_builds_once_with_canonical_environment(
    tmp_path: Path,
    monkeypatch,
) -> None:
    manifest = tmp_path / "runtime" / "molt-backend-mlir" / "Cargo.toml"
    manifest.parent.mkdir(parents=True)
    manifest.write_text(
        "[workspace]\n[package]\nname='m'\nversion='0.0.0'\n", encoding="utf-8"
    )
    cargo_target = tmp_path / "cargo-target"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(cargo_target))
    backend = cargo_target / "release" / _backend_name()
    captured: dict[str, object] = {}

    install_module_view(
        monkeypatch,
        "shutil",
        shutil,
        mlir_backend,
        which=lambda name: "C:/bin/cargo.exe" if name == "cargo" else None,
    )
    monkeypatch.setattr(
        mlir_backend,
        "mlir_toolchain_environment",
        lambda root, *, environ: {**environ, "MOLT_LLVM_PREFIX": "C:/LLVM"},
    )
    monkeypatch.setattr(
        mlir_backend,
        "_cargo_build_env",
        lambda: {
            "RUSTC_WRAPPER": "/usr/bin/sccache",
            "CARGO_INCREMENTAL": "0",
        },
    )

    def fake_run(command: list[str], **kwargs: object):
        captured["command"] = command
        captured["kwargs"] = kwargs
        backend.parent.mkdir(parents=True)
        backend.write_text("", encoding="utf-8")
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
    assert kwargs["env"] == {
        "RUSTC_WRAPPER": "/usr/bin/sccache",
        "CARGO_INCREMENTAL": "0",
        "MOLT_LLVM_PREFIX": "C:/LLVM",
        "CARGO_TARGET_DIR": str(cargo_target),
    }
    assert kwargs["timeout"] == 1800


def test_mlir_backend_executable_name_is_host_specific() -> None:
    assert mlir_backend._mlir_backend_executable_name(os_name="nt").endswith(".exe")
    assert (
        mlir_backend._mlir_backend_executable_name(os_name="posix")
        == "molt-backend-mlir"
    )
