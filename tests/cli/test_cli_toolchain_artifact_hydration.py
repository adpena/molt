from __future__ import annotations

import os
import subprocess
from pathlib import Path

from molt import cli

_FAKE_STATICLIB = b"!<arch>\nfake-staticlib"


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
    canonical_backend.write_text(
        "#!/bin/sh\n"
        'out=""\n'
        "while [ $# -gt 0 ]; do\n"
        '  if [ "$1" = "--output" ]; then\n'
        "    shift\n"
        '    out="$1"\n'
        "  fi\n"
        "  shift\n"
        "done\n"
        "printf 'ok' > \"$out\"\n"
    )
    canonical_backend.chmod(0o755)

    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    canonical_fp = cli._artifact_state_path_for_build_state_root(
        cli._canonical_build_state_root(project_root),
        canonical_backend,
        subdir="backend_fingerprints",
        stem_suffix="dev-fast",
        extension="fingerprint",
    )
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
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("cargo should not run")
        ),
    )

    assert cli._ensure_backend_binary(
        isolated_backend,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=project_root,
        backend_features=("native-backend",),
    )
    assert isolated_backend.read_text() == canonical_backend.read_text()
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
    canonical_runtime.write_bytes(_FAKE_STATICLIB)

    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    canonical_fp = cli._artifact_state_path_for_build_state_root(
        cli._canonical_build_state_root(project_root),
        canonical_runtime,
        subdir="runtime_fingerprints",
        stem_suffix="dev-fast.native",
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
        "_run_cargo_with_sccache_retry",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("cargo should not run")
        ),
    )

    assert cli._ensure_runtime_lib(
        isolated_runtime,
        None,
        True,
        "dev-fast",
        project_root,
        1.0,
    )
    assert isolated_runtime.read_bytes() == _FAKE_STATICLIB


def test_ensure_runtime_wasm_hydrates_from_current_target_artifact(
    monkeypatch,
    tmp_path: Path,
) -> None:
    project_root = tmp_path
    target_root = project_root / "shared-target"
    profile_dir = cli._cargo_profile_dir("dev-fast")
    canonical_runtime = (
        target_root / "wasm32-wasip1" / profile_dir / "deps" / "molt_runtime.wasm"
    )
    isolated_runtime = project_root / "wasm" / "molt_runtime.wasm"
    canonical_runtime.parent.mkdir(parents=True, exist_ok=True)
    canonical_runtime.write_bytes(b"\x00asm\x01\x00\x00\x00runtime")

    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    canonical_fp = cli._artifact_state_path_for_build_state_root(
        target_root / ".molt_state",
        canonical_runtime,
        subdir="runtime_fingerprints",
        stem_suffix="dev-fast.wasm32-wasip1",
        extension="fingerprint",
    )
    canonical_fp.parent.mkdir(parents=True, exist_ok=True)
    cli._write_runtime_fingerprint(canonical_fp, fingerprint)

    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setenv("MOLT_EXT_ROOT", str(project_root))
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint",
        lambda *args, **kwargs: dict(fingerprint),
    )
    monkeypatch.setattr(cli, "_inspect_wasm_binary", lambda _path: "valid")
    monkeypatch.setattr(
        cli,
        "_runtime_wasm_exports_satisfy",
        lambda *_args, **_kwargs: True,
    )
    monkeypatch.setattr(
        cli,
        "_write_runtime_wasm_integrity_sidecar",
        lambda _path: None,
    )
    monkeypatch.setattr(
        cli,
        "_run_runtime_wasm_cargo_build",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("cargo should not run")
        ),
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


def test_ensure_runtime_wasm_reloc_relinks_from_current_target_staticlib(
    monkeypatch,
    tmp_path: Path,
) -> None:
    project_root = tmp_path
    target_root = project_root / "shared-target"
    profile_dir = cli._cargo_profile_dir("release-fast")
    current_staticlib = (
        target_root / "wasm32-wasip1" / profile_dir / "libmolt_runtime.a"
    )
    runtime_reloc = project_root / "wasm" / "molt_runtime_reloc.wasm"
    current_staticlib.parent.mkdir(parents=True, exist_ok=True)
    current_staticlib.write_bytes(_FAKE_STATICLIB)

    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    current_staticlib_fp = cli._artifact_state_path_for_build_state_root(
        target_root / ".molt_state",
        current_staticlib,
        subdir="runtime_fingerprints",
        stem_suffix="release-fast.wasm32-wasip1",
        extension="fingerprint",
    )
    current_staticlib_fp.parent.mkdir(parents=True, exist_ok=True)
    cli._write_runtime_fingerprint(current_staticlib_fp, fingerprint)

    linked: dict[str, Path] = {}

    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setenv("MOLT_EXT_ROOT", str(project_root))
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint",
        lambda *args, **kwargs: dict(fingerprint),
    )
    monkeypatch.setattr(
        cli,
        "_run_runtime_wasm_cargo_build",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("cargo should not run")
        ),
    )

    def fake_link_runtime_staticlib_to_reloc_wasm(
        *,
        staticlib_path: Path,
        output_path: Path,
        json_output: bool,
        link_timeout: float | None,
    ) -> bool:
        del json_output, link_timeout
        linked["staticlib_path"] = staticlib_path
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_bytes(b"\x00asm\x01\x00\x00\x00reloc")
        return True

    monkeypatch.setattr(
        cli,
        "_link_runtime_staticlib_to_reloc_wasm",
        fake_link_runtime_staticlib_to_reloc_wasm,
    )
    monkeypatch.setattr(
        cli,
        "_write_runtime_wasm_integrity_sidecar",
        lambda _path: None,
    )

    assert cli._ensure_runtime_wasm(
        runtime_reloc,
        reloc=True,
        json_output=True,
        cargo_profile="release-fast",
        cargo_timeout=1.0,
        project_root=project_root,
    )
    assert linked["staticlib_path"] == current_staticlib
    assert runtime_reloc.read_bytes() == b"\x00asm\x01\x00\x00\x00reloc"


def test_ensure_runtime_wasm_reloc_builds_when_only_hashed_current_target_staticlib_exists(
    monkeypatch,
    tmp_path: Path,
) -> None:
    project_root = tmp_path
    target_root = project_root / "shared-target"
    profile_dir = cli._cargo_profile_dir("release-fast")
    current_staticlib = (
        target_root
        / "wasm32-wasip1"
        / profile_dir
        / "deps"
        / "libmolt_runtime-deadbeefdeadbeef.a"
    )
    runtime_reloc = project_root / "wasm" / "molt_runtime_reloc.wasm"
    current_staticlib.parent.mkdir(parents=True, exist_ok=True)
    current_staticlib.write_bytes(_FAKE_STATICLIB)

    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    current_staticlib_fp = cli._artifact_state_path_for_build_state_root(
        target_root / ".molt_state",
        current_staticlib,
        subdir="runtime_fingerprints",
        stem_suffix="release-fast.wasm32-wasip1",
        extension="fingerprint",
    )
    current_staticlib_fp.parent.mkdir(parents=True, exist_ok=True)
    cli._write_runtime_fingerprint(current_staticlib_fp, fingerprint)

    linked: dict[str, Path] = {}

    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setenv("MOLT_EXT_ROOT", str(project_root))
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint",
        lambda *args, **kwargs: dict(fingerprint),
    )
    cargo_calls: list[tuple[object, object]] = []

    def fake_run_runtime_wasm_cargo_build(*args: object, **kwargs: object):
        cargo_calls.append((args, kwargs))
        return subprocess.CompletedProcess(["cargo"], 0, "", ""), current_staticlib

    monkeypatch.setattr(
        cli, "_run_runtime_wasm_cargo_build", fake_run_runtime_wasm_cargo_build
    )

    def fake_link_runtime_staticlib_to_reloc_wasm(
        *,
        staticlib_path: Path,
        output_path: Path,
        json_output: bool,
        link_timeout: float | None,
    ) -> bool:
        del json_output, link_timeout
        linked["staticlib_path"] = staticlib_path
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_bytes(b"\x00asm\x01\x00\x00\x00reloc")
        return True

    monkeypatch.setattr(
        cli,
        "_link_runtime_staticlib_to_reloc_wasm",
        fake_link_runtime_staticlib_to_reloc_wasm,
    )
    monkeypatch.setattr(
        cli,
        "_write_runtime_wasm_integrity_sidecar",
        lambda _path: None,
    )

    assert cli._ensure_runtime_wasm(
        runtime_reloc,
        reloc=True,
        json_output=True,
        cargo_profile="release-fast",
        cargo_timeout=1.0,
        project_root=project_root,
    )
    assert cargo_calls
    assert linked["staticlib_path"] == current_staticlib
    assert runtime_reloc.read_bytes() == b"\x00asm\x01\x00\x00\x00reloc"
