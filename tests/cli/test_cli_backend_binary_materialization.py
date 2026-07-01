from __future__ import annotations

import os
import subprocess
from pathlib import Path

import molt.cli as cli
from molt.cli import backend_binary as cli_backend_binary
import pytest


def test_ensure_backend_binary_refreshes_feature_tagged_alias_from_newer_cargo_output(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    exe_suffix = ".exe" if os.name == "nt" else ""
    target_dir = tmp_path / "target" / "dev-fast"
    target_dir.mkdir(parents=True, exist_ok=True)
    backend_bin = target_dir / f"molt-backend.wasm_backend{exe_suffix}"
    cargo_output = target_dir / f"molt-backend{exe_suffix}"
    backend_bin.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    cargo_output.write_text("#!/bin/sh\nexit 0\n# fresh\n", encoding="utf-8")
    backend_bin.chmod(0o755)
    cargo_output.chmod(0o755)

    # Ensure the feature-tagged alias is older than the canonical cargo output.
    stale_mtime = cargo_output.stat().st_mtime_ns - 1_000_000
    os.utime(backend_bin, ns=(stale_mtime, stale_mtime))

    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    fingerprint_path = cli_backend_binary._backend_fingerprint_path(
        tmp_path, backend_bin, "dev-fast"
    )
    cli._write_runtime_fingerprint(fingerprint_path, fingerprint)

    def fake_backend_fingerprint(*args: object, **kwargs: object) -> dict[str, str]:
        del args, kwargs
        return dict(fingerprint)

    def fail_run_cargo(*args: object, **kwargs: object) -> None:
        del args, kwargs
        raise AssertionError("unexpected cargo rebuild")

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", fake_backend_fingerprint
    )
    monkeypatch.setattr(cli_backend_binary, "_codesign_binary", lambda _path: None)
    monkeypatch.setattr(
        cli_backend_binary, "_run_cargo_with_sccache_retry", fail_run_cargo
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **kwargs: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )

    assert cli_backend_binary._ensure_backend_binary(
        backend_bin,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        backend_features=("wasm-backend",),
    )
    assert backend_bin.read_text(encoding="utf-8") == cargo_output.read_text(
        encoding="utf-8"
    )


def test_ensure_backend_binary_returns_cargo_failure_detail(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    backend_bin = tmp_path / "target" / "release-fast" / "molt-backend"
    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}

    def fake_run_cargo(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        return subprocess.CompletedProcess(
            cmd,
            101,
            "",
            "error: duplicate symbol: PyMemoryView_FromMemory\nnote: backend link failed",
        )

    monkeypatch.setattr(
        cli_backend_binary,
        "_backend_fingerprint",
        lambda *args, **kwargs: dict(fingerprint),
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_cargo_with_sccache_retry",
        fake_run_cargo,
    )

    result = cli_backend_binary._ensure_backend_binary(
        backend_bin,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile="release-fast",
        project_root=tmp_path,
        backend_features=("native-backend",),
    )

    assert not result
    assert result.phase == "backend_cargo_build"
    assert result.returncode == 101
    assert result.command[:4] == (
        "cargo",
        "build",
        "--package",
        "molt-backend",
    )
    assert "Backend cargo build failed (exit 101)" in result.message
    assert "duplicate symbol: PyMemoryView_FromMemory" in result.message
