from __future__ import annotations

import os
from pathlib import Path

from molt import cli


def test_ensure_backend_binary_hydrates_from_canonical_target(
    monkeypatch,
    tmp_path: Path,
) -> None:
    project_root = tmp_path
    canonical_target = project_root / "target"
    isolated_target = project_root / "isolated-target"
    canonical_backend = canonical_target / "dev-fast" / "molt-backend"
    isolated_backend = isolated_target / "dev-fast" / "molt-backend"
    canonical_backend.parent.mkdir(parents=True, exist_ok=True)
    canonical_backend.write_text("backend-binary")
    canonical_backend.chmod(0o755)

    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    canonical_fp = cli._backend_fingerprint_path(project_root, canonical_backend, "dev-fast")
    canonical_fp.parent.mkdir(parents=True, exist_ok=True)
    cli._write_runtime_fingerprint(canonical_fp, fingerprint)

    monkeypatch.setenv("CARGO_TARGET_DIR", str(isolated_target))
    monkeypatch.setattr(
        cli,
        "_backend_fingerprint",
        lambda *args, **kwargs: dict(fingerprint),
    )
    monkeypatch.setattr(
        cli,
        "_run_cargo_with_sccache_retry",
        lambda *args, **kwargs: (_ for _ in ()).throw(AssertionError("cargo should not run")),
    )

    assert cli._ensure_backend_binary(
        isolated_backend,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=project_root,
        backend_features=("native-backend",),
    )
    assert isolated_backend.read_text() == "backend-binary"
    assert os.access(isolated_backend, os.X_OK)


def test_ensure_runtime_lib_hydrates_from_canonical_target(
    monkeypatch,
    tmp_path: Path,
) -> None:
    project_root = tmp_path
    canonical_target = project_root / "target"
    isolated_target = project_root / "isolated-target"
    canonical_runtime = canonical_target / "dev-fast" / "libmolt_runtime.a"
    isolated_runtime = isolated_target / "dev-fast" / "libmolt_runtime.a"
    canonical_runtime.parent.mkdir(parents=True, exist_ok=True)
    canonical_runtime.write_text("runtime-lib")

    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    canonical_fp = cli._runtime_fingerprint_path(
        project_root,
        canonical_runtime,
        "dev-fast",
        None,
    )
    canonical_fp.parent.mkdir(parents=True, exist_ok=True)
    cli._write_runtime_fingerprint(canonical_fp, fingerprint)

    monkeypatch.setenv("CARGO_TARGET_DIR", str(isolated_target))
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint",
        lambda *args, **kwargs: dict(fingerprint),
    )
    monkeypatch.setattr(
        cli,
        "_run_cargo_with_sccache_retry",
        lambda *args, **kwargs: (_ for _ in ()).throw(AssertionError("cargo should not run")),
    )

    assert cli._ensure_runtime_lib(
        isolated_runtime,
        None,
        True,
        "dev-fast",
        project_root,
        1.0,
    )
    assert isolated_runtime.read_text() == "runtime-lib"


def test_ensure_runtime_wasm_hydrates_from_canonical_target(
    monkeypatch,
    tmp_path: Path,
) -> None:
    project_root = tmp_path
    canonical_target = project_root / "target"
    isolated_target = project_root / "isolated-target"
    profile_dir = cli._cargo_profile_dir("dev-fast")
    canonical_runtime = cli._wasm_runtime_artifact_path(canonical_target, profile_dir)
    isolated_runtime = project_root / "wasm" / "molt_runtime.wasm"
    canonical_runtime.parent.mkdir(parents=True, exist_ok=True)
    canonical_runtime.write_bytes(b"\x00asm\x01\x00\x00\x00runtime")

    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    canonical_fp = cli._artifact_state_path_for_build_state_root(
        cli._canonical_build_state_root(project_root),
        canonical_runtime,
        subdir="runtime_fingerprints",
        stem_suffix=f"dev-fast.wasm32-wasip1",
        extension="fingerprint",
    )
    canonical_fp.parent.mkdir(parents=True, exist_ok=True)
    cli._write_runtime_fingerprint(canonical_fp, fingerprint)

    monkeypatch.setenv("CARGO_TARGET_DIR", str(isolated_target))
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint",
        lambda *args, **kwargs: dict(fingerprint),
    )
    monkeypatch.setattr(
        cli,
        "_run_runtime_wasm_cargo_build",
        lambda *args, **kwargs: (_ for _ in ()).throw(AssertionError("cargo should not run")),
    )

    assert cli._ensure_runtime_wasm(
        isolated_runtime,
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=1.0,
        project_root=project_root,
    )
    assert isolated_runtime.read_bytes() == canonical_runtime.read_bytes()
