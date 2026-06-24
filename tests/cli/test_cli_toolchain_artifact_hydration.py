from __future__ import annotations

import contextlib
import json
import os
import subprocess
from pathlib import Path
from typing import cast

from molt import cli
from molt.cli import runtime_build as RUNTIME_BUILD
from molt.cli import runtime_paths as RUNTIME_PATHS
import pytest

_FAKE_STATICLIB = b"!<arch>\nfake-staticlib"


def _cargo_runtime_artifact_stdout(path: Path) -> bytes:
    return (
        json.dumps(
            {
                "reason": "compiler-artifact",
                "package_id": "path+file:///repo/runtime/molt-runtime#0.0.1",
                "target": {"name": "molt_runtime"},
                "filenames": [str(path)],
                "fresh": True,
            }
        )
        + "\n"
    ).encode("utf-8")


def test_runtime_wasm_cargo_build_preserves_stale_candidates_and_uses_reported_artifact(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_root = tmp_path / "target"
    profile_dir = cli._cargo_profile_dir("dev-fast")
    primary = cli._wasm_runtime_artifact_path(target_root, profile_dir)
    deps_primary = (
        cli._wasm_runtime_deps_dir(target_root, profile_dir) / "molt_runtime.wasm"
    )
    stale_hashed = (
        cli._wasm_runtime_deps_dir(target_root, profile_dir)
        / "molt_runtime-deadbeef.wasm"
    )
    reported = (
        cli._wasm_runtime_deps_dir(target_root, profile_dir)
        / "molt_runtime-feedface.wasm"
    )
    for path, payload in (
        (primary, b"old-primary"),
        (deps_primary, b"old-deps"),
        (stale_hashed, b"old-hashed"),
        (reported, b"new-reported"),
    ):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(payload)

    seen: dict[str, object] = {}

    def fake_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        seen["cmd"] = cmd
        seen["env"] = kwargs["env"]
        return subprocess.CompletedProcess(
            cmd,
            0,
            _cargo_runtime_artifact_stdout(reported),
            b"",
        )

    monkeypatch.setattr(cli, "_build_slot", lambda: contextlib.nullcontext(None))
    monkeypatch.setattr(cli, "_run_subprocess_captured_to_tempfiles", fake_run)

    build, src = cli._run_runtime_wasm_cargo_build(
        cmd=[
            "cargo",
            "rustc",
            "--package",
            "molt-runtime",
            "--profile",
            "dev-fast",
            "--target",
            "wasm32-wasip1",
            "--lib",
            "--",
            "--crate-type=cdylib",
        ],
        root=tmp_path,
        env={},
        cargo_timeout=1.0,
        profile_dir=profile_dir,
        target_root_override=target_root,
        json_output=True,
        artifact_kind="cdylib",
    )

    assert build.returncode == 0
    assert src == reported
    assert primary.read_bytes() == b"old-primary"
    assert deps_primary.read_bytes() == b"old-deps"
    assert stale_hashed.read_bytes() == b"old-hashed"
    assert "--message-format=json-render-diagnostics" in cast(list[str], seen["cmd"])
    assert cast(dict[str, str], seen["env"])["CARGO_TARGET_DIR"] == str(target_root)


def test_runtime_wasm_cargo_build_does_not_fallback_to_old_artifact_without_report(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_root = tmp_path / "target"
    profile_dir = cli._cargo_profile_dir("dev-fast")
    primary = cli._wasm_runtime_artifact_path(target_root, profile_dir)
    primary.parent.mkdir(parents=True, exist_ok=True)
    primary.write_bytes(b"old-valid")

    def fake_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        del kwargs
        return subprocess.CompletedProcess(
            cmd,
            0,
            b'{"reason":"build-finished","success":true}\n',
            b"",
        )

    monkeypatch.setattr(cli, "_build_slot", lambda: contextlib.nullcontext(None))
    monkeypatch.setattr(cli, "_run_subprocess_captured_to_tempfiles", fake_run)

    build, src = cli._run_runtime_wasm_cargo_build(
        cmd=[
            "cargo",
            "rustc",
            "--package",
            "molt-runtime",
            "--profile",
            "dev-fast",
            "--target",
            "wasm32-wasip1",
            "--lib",
            "--",
            "--crate-type=cdylib",
        ],
        root=tmp_path,
        env={},
        cargo_timeout=1.0,
        profile_dir=profile_dir,
        target_root_override=target_root,
        json_output=True,
        artifact_kind="cdylib",
    )

    assert build.returncode == 0
    assert src != primary
    assert src.name == ".molt_runtime.cargo-report-missing.wasm"
    assert not src.exists()
    assert primary.read_bytes() == b"old-valid"


def test_runtime_wasm_cargo_build_accepts_cargo_fresh_primary_artifact(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_root = tmp_path / "target"
    profile_dir = cli._cargo_profile_dir("dev-fast")
    primary = cli._wasm_runtime_artifact_path(target_root, profile_dir)
    primary.parent.mkdir(parents=True, exist_ok=True)
    primary.write_bytes(b"fresh-primary")

    def fake_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        del kwargs
        return subprocess.CompletedProcess(
            cmd,
            0,
            _cargo_runtime_artifact_stdout(primary),
            b"",
        )

    monkeypatch.setattr(cli, "_build_slot", lambda: contextlib.nullcontext(None))
    monkeypatch.setattr(cli, "_run_subprocess_captured_to_tempfiles", fake_run)

    _build, src = cli._run_runtime_wasm_cargo_build(
        cmd=[
            "cargo",
            "rustc",
            "--package",
            "molt-runtime",
            "--profile",
            "dev-fast",
            "--target",
            "wasm32-wasip1",
            "--lib",
            "--",
            "--crate-type=cdylib",
        ],
        root=tmp_path,
        env={},
        cargo_timeout=1.0,
        profile_dir=profile_dir,
        target_root_override=target_root,
        json_output=True,
        artifact_kind="cdylib",
    )

    assert src == primary
    assert primary.read_bytes() == b"fresh-primary"


def test_runtime_wasm_cargo_build_preserves_staticlibs_and_uses_reported_staticlib(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_root = tmp_path / "target"
    profile_dir = cli._cargo_profile_dir("release-fast")
    primary = cli._wasm_runtime_staticlib_path(target_root, profile_dir)
    reported = (
        cli._wasm_runtime_deps_dir(target_root, profile_dir)
        / "libmolt_runtime-feedface.a"
    )
    primary.parent.mkdir(parents=True, exist_ok=True)
    primary.write_bytes(b"old-staticlib")
    reported.parent.mkdir(parents=True, exist_ok=True)
    reported.write_bytes(b"new-staticlib")

    def fake_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        del kwargs
        return subprocess.CompletedProcess(
            cmd,
            0,
            _cargo_runtime_artifact_stdout(reported),
            b"",
        )

    monkeypatch.setattr(cli, "_build_slot", lambda: contextlib.nullcontext(None))
    monkeypatch.setattr(cli, "_run_subprocess_captured_to_tempfiles", fake_run)

    _build, src = cli._run_runtime_wasm_cargo_build(
        cmd=[
            "cargo",
            "rustc",
            "--package",
            "molt-runtime",
            "--profile",
            "release-fast",
            "--target",
            "wasm32-wasip1",
            "--lib",
            "--",
            "--crate-type=staticlib",
        ],
        root=tmp_path,
        env={},
        cargo_timeout=1.0,
        profile_dir=profile_dir,
        target_root_override=target_root,
        json_output=True,
        artifact_kind="staticlib",
    )

    assert src == reported
    assert primary.read_bytes() == b"old-staticlib"
    assert reported.read_bytes() == b"new-staticlib"


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
    cli._write_runtime_fingerprint(
        canonical_fp, fingerprint, artifact=canonical_runtime
    )

    monkeypatch.setenv("CARGO_TARGET_DIR", str(isolated_target))
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint",
        lambda *args, **kwargs: dict(fingerprint),
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_run_cargo_with_sccache_retry",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("cargo should not run")
        ),
    )

    assert RUNTIME_BUILD._ensure_runtime_lib(
        isolated_runtime,
        None,
        True,
        "dev-fast",
        project_root,
        1.0,
    )
    assert isolated_runtime.read_bytes() == _FAKE_STATICLIB


def test_ensure_runtime_lib_hydration_requires_artifact_digest_match(
    monkeypatch,
    tmp_path: Path,
) -> None:
    project_root = tmp_path
    canonical_target = project_root / "target"
    isolated_target = project_root / "isolated-target"
    canonical_runtime = canonical_target / "dev-fast" / "libmolt_runtime.a"
    isolated_runtime = isolated_target / "dev-fast" / "libmolt_runtime.a"
    canonical_runtime.parent.mkdir(parents=True, exist_ok=True)
    canonical_runtime.write_bytes(_FAKE_STATICLIB + b"stale")

    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    canonical_fp = cli._artifact_state_path_for_build_state_root(
        cli._canonical_build_state_root(project_root),
        canonical_runtime,
        subdir="runtime_fingerprints",
        stem_suffix="dev-fast.native",
        extension="fingerprint",
    )
    canonical_fp.parent.mkdir(parents=True, exist_ok=True)
    cli._write_runtime_fingerprint(
        canonical_fp, fingerprint, artifact=canonical_runtime
    )
    canonical_runtime.write_bytes(_FAKE_STATICLIB + b"mutated")
    cargo_runs: list[list[str]] = []

    monkeypatch.setenv("CARGO_TARGET_DIR", str(isolated_target))
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint",
        lambda *args, **kwargs: dict(fingerprint),
    )

    def fake_run_cargo(
        cmd: list[str],
        *,
        cwd: Path,
        env: dict[str, str],
        timeout: float | None,
        json_output: bool,
        label: str,
    ) -> subprocess.CompletedProcess[str]:
        del cwd, env, timeout, json_output, label
        cargo_runs.append(list(cmd))
        scratch_lib = RUNTIME_PATHS._runtime_cargo_scratch_lib_path(
            isolated_runtime, None
        )
        scratch_lib.parent.mkdir(parents=True, exist_ok=True)
        scratch_lib.write_bytes(_FAKE_STATICLIB)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(RUNTIME_BUILD, "_run_cargo_with_sccache_retry", fake_run_cargo)

    assert RUNTIME_BUILD._ensure_runtime_lib(
        isolated_runtime,
        None,
        True,
        "dev-fast",
        project_root,
        1.0,
    )
    assert cargo_runs
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
    cli._write_runtime_fingerprint(
        canonical_fp, fingerprint, artifact=canonical_runtime
    )

    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setenv("MOLT_EXT_ROOT", str(project_root))
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint",
        lambda *args, **kwargs: dict(fingerprint),
    )
    monkeypatch.setattr(cli, "_inspect_wasm_binary", lambda _path: "valid")
    monkeypatch.setattr(
        cli, "_is_valid_shared_runtime_wasm_artifact", lambda _path: True
    )
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
    cli._write_runtime_fingerprint(
        current_staticlib_fp,
        fingerprint,
        artifact=current_staticlib,
    )

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
        export_link_args: str = "",
    ) -> bool:
        del json_output, link_timeout, export_link_args
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


def test_ensure_runtime_wasm_reloc_relinks_from_hashed_current_target_staticlib(
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
    cli._write_runtime_fingerprint(
        current_staticlib_fp,
        fingerprint,
        artifact=current_staticlib,
    )

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
        export_link_args: str = "",
    ) -> bool:
        del json_output, link_timeout, export_link_args
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


def test_ensure_runtime_wasm_uses_reported_hashed_artifact_not_stale_primary(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    project_root = tmp_path
    target_root = project_root / "shared-target"
    profile_dir = cli._cargo_profile_dir("dev-fast")
    primary = cli._wasm_runtime_artifact_path(target_root, profile_dir)
    reported = (
        cli._wasm_runtime_deps_dir(target_root, profile_dir)
        / "molt_runtime-feedface.wasm"
    )
    runtime_wasm = project_root / "wasm" / "molt_runtime.wasm"
    primary.parent.mkdir(parents=True, exist_ok=True)
    reported.parent.mkdir(parents=True, exist_ok=True)
    primary.write_bytes(b"\x00asm\x01\x00\x00\x00stale-primary")
    reported.write_bytes(b"\x00asm\x01\x00\x00\x00reported-hashed")

    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    primary_fp = cli._runtime_target_fingerprint_path(
        target_root / ".molt_state",
        primary,
        cargo_profile="dev-fast",
        target_label="wasm32-wasip1",
    )
    primary_fp.parent.mkdir(parents=True, exist_ok=True)
    cli._write_runtime_fingerprint(primary_fp, fingerprint)

    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setenv("MOLT_EXT_ROOT", str(project_root))
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint",
        lambda *args, **kwargs: dict(fingerprint),
    )
    monkeypatch.setattr(cli, "_inspect_wasm_binary", lambda _path: "valid")
    monkeypatch.setattr(
        cli, "_is_valid_shared_runtime_wasm_artifact", lambda _path: True
    )
    monkeypatch.setattr(
        cli,
        "_runtime_wasm_exports_satisfy",
        lambda *_args, **_kwargs: True,
    )
    monkeypatch.setattr(
        cli,
        "_runtime_wasm_missing_exports",
        lambda *_args, **_kwargs: set(),
    )
    monkeypatch.setattr(
        cli,
        "_write_runtime_wasm_integrity_sidecar",
        lambda _path: None,
    )

    cargo_calls: list[tuple[object, object]] = []

    def fake_run_runtime_wasm_cargo_build(*args: object, **kwargs: object):
        cargo_calls.append((args, kwargs))
        return subprocess.CompletedProcess(["cargo"], 0, "", ""), reported

    monkeypatch.setattr(
        cli, "_run_runtime_wasm_cargo_build", fake_run_runtime_wasm_cargo_build
    )

    assert cli._ensure_runtime_wasm(
        runtime_wasm,
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=1.0,
        project_root=project_root,
    )
    assert cargo_calls
    assert runtime_wasm.read_bytes() == reported.read_bytes()
    assert primary.read_bytes() == b"\x00asm\x01\x00\x00\x00stale-primary"

    reported_fp = cli._runtime_target_fingerprint_path(
        target_root / ".molt_state",
        reported,
        cargo_profile="dev-fast",
        target_label="wasm32-wasip1",
    )
    assert cli._read_runtime_fingerprint(reported_fp)["artifact_sha256"] == (
        cli._sha256_file(reported)
    )
    assert cli._read_runtime_fingerprint(primary_fp).get("artifact_sha256") is None

    runtime_wasm.unlink()
    cargo_calls.clear()
    monkeypatch.setattr(
        cli,
        "_run_runtime_wasm_cargo_build",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("cargo should not run")
        ),
    )

    assert cli._ensure_runtime_wasm(
        runtime_wasm,
        reloc=False,
        json_output=True,
        cargo_profile="dev-fast",
        cargo_timeout=1.0,
        project_root=project_root,
    )
    assert not cargo_calls
    assert runtime_wasm.read_bytes() == reported.read_bytes()


def test_ensure_runtime_wasm_reloc_uses_reported_staticlib_not_stale_primary(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    project_root = tmp_path
    target_root = project_root / "shared-target"
    profile_dir = cli._cargo_profile_dir("release-fast")
    primary = cli._wasm_runtime_staticlib_path(target_root, profile_dir)
    reported = (
        cli._wasm_runtime_deps_dir(target_root, profile_dir)
        / "libmolt_runtime-feedface.a"
    )
    runtime_reloc = project_root / "wasm" / "molt_runtime_reloc.wasm"
    primary.parent.mkdir(parents=True, exist_ok=True)
    reported.parent.mkdir(parents=True, exist_ok=True)
    primary.write_bytes(_FAKE_STATICLIB + b"stale-primary")
    reported.write_bytes(_FAKE_STATICLIB + b"reported-hashed")

    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    primary_fp = cli._runtime_target_fingerprint_path(
        target_root / ".molt_state",
        primary,
        cargo_profile="release-fast",
        target_label="wasm32-wasip1",
    )
    primary_fp.parent.mkdir(parents=True, exist_ok=True)
    cli._write_runtime_fingerprint(primary_fp, fingerprint)

    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    monkeypatch.setenv("MOLT_EXT_ROOT", str(project_root))
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint",
        lambda *args, **kwargs: dict(fingerprint),
    )
    monkeypatch.setattr(
        cli,
        "_write_runtime_wasm_integrity_sidecar",
        lambda _path: None,
    )

    cargo_calls: list[tuple[object, object]] = []
    linked: list[Path] = []

    def fake_run_runtime_wasm_cargo_build(*args: object, **kwargs: object):
        cargo_calls.append((args, kwargs))
        return subprocess.CompletedProcess(["cargo"], 0, "", ""), reported

    def fake_link_runtime_staticlib_to_reloc_wasm(
        *,
        staticlib_path: Path,
        output_path: Path,
        json_output: bool,
        link_timeout: float | None,
        export_link_args: str = "",
    ) -> bool:
        del json_output, link_timeout, export_link_args
        linked.append(staticlib_path)
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_bytes(
            b"\x00asm\x01\x00\x00\x00" + staticlib_path.name.encode()
        )
        return True

    monkeypatch.setattr(
        cli, "_run_runtime_wasm_cargo_build", fake_run_runtime_wasm_cargo_build
    )
    monkeypatch.setattr(
        cli,
        "_link_runtime_staticlib_to_reloc_wasm",
        fake_link_runtime_staticlib_to_reloc_wasm,
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
    assert linked == [reported]

    reported_fp = cli._runtime_target_fingerprint_path(
        target_root / ".molt_state",
        reported,
        cargo_profile="release-fast",
        target_label="wasm32-wasip1",
    )
    assert cli._read_runtime_fingerprint(reported_fp)["artifact_sha256"] == (
        cli._sha256_file(reported)
    )
    assert cli._read_runtime_fingerprint(primary_fp).get("artifact_sha256") is None

    runtime_reloc.unlink()
    cargo_calls.clear()
    linked.clear()
    monkeypatch.setattr(
        cli,
        "_run_runtime_wasm_cargo_build",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("cargo should not run")
        ),
    )

    assert cli._ensure_runtime_wasm(
        runtime_reloc,
        reloc=True,
        json_output=True,
        cargo_profile="release-fast",
        cargo_timeout=1.0,
        project_root=project_root,
    )
    assert not cargo_calls
    assert linked == [reported]
