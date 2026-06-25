from __future__ import annotations

import os
import subprocess
from pathlib import Path

import molt.cli as cli
from molt.cli import build_pipeline as cli_build_pipeline
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
    fingerprint_path = cli_build_pipeline._backend_fingerprint_path(
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
        cli_build_pipeline, "_backend_fingerprint", fake_backend_fingerprint
    )
    monkeypatch.setattr(cli_build_pipeline, "_codesign_binary", lambda _path: None)
    monkeypatch.setattr(
        cli_build_pipeline, "_run_cargo_with_sccache_retry", fail_run_cargo
    )
    monkeypatch.setattr(
        cli_build_pipeline,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **kwargs: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )

    assert cli_build_pipeline._ensure_backend_binary(
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
