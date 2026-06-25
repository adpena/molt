from __future__ import annotations

import inspect
from pathlib import Path

import molt.cli as cli
from molt.cli import mlir_backend

_MLIR_BACKEND_NAMES = (
    "_find_mlir_backend_binary",
    "_run_mlir_backend_pipeline",
)


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
        / "molt-backend-mlir"
    )
    backend.parent.mkdir(parents=True)
    backend.write_text("")

    assert mlir_backend._find_mlir_backend_binary(tmp_path) == backend


def test_find_mlir_backend_binary_uses_session_target_before_default(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setenv("MOLT_SESSION_ID", "agent-a")
    session_backend = tmp_path / "target-agent-a" / "debug" / "molt-backend-mlir"
    default_backend = tmp_path / "target" / "release" / "molt-backend-mlir"
    session_backend.parent.mkdir(parents=True)
    default_backend.parent.mkdir(parents=True)
    session_backend.write_text("")
    default_backend.write_text("")

    assert mlir_backend._find_mlir_backend_binary(tmp_path) == session_backend
