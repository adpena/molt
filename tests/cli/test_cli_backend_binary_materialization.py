from __future__ import annotations

import os
import subprocess
from pathlib import Path

import molt.cli as cli
from molt.cli import backend_binary as cli_backend_binary
from molt.cli.backend_compile import _backend_environment_with_compiler_fingerprint
from molt.exact_json import canonical_json_sha256
import pytest


def _fingerprint() -> dict[str, str]:
    return {
        "hash": canonical_json_sha256("backend-source"),
        "rustc": "rustc-1",
        "inputs_digest": canonical_json_sha256("backend-inputs"),
        "meta_digest": canonical_json_sha256("backend-meta"),
    }


def test_backend_compiler_cache_fingerprint_covers_backend_fingerprint_fields() -> None:
    fingerprint = _fingerprint()
    binary_identity = {
        "entrypoint": "molt-backend",
        "content_filename": "molt-backend",
        "size": 1,
        "sha256": canonical_json_sha256("backend-executable"),
    }

    baseline = cli_backend_binary._backend_compiler_cache_fingerprint(
        fingerprint, binary_identity
    )

    assert baseline
    assert baseline == cli_backend_binary._backend_compiler_cache_fingerprint(
        dict(fingerprint), binary_identity
    )
    assert baseline != cli_backend_binary._backend_compiler_cache_fingerprint(
        {**fingerprint, "rustc": "rustc-2"}, binary_identity
    )
    assert baseline != cli_backend_binary._backend_compiler_cache_fingerprint(
        {**fingerprint, "meta_digest": canonical_json_sha256("backend-meta-2")},
        binary_identity,
    )
    assert baseline != cli_backend_binary._backend_compiler_cache_fingerprint(
        fingerprint,
        {**binary_identity, "sha256": canonical_json_sha256("other-executable")},
    )
    assert cli_backend_binary._backend_compiler_cache_fingerprint(None, binary_identity)


@pytest.mark.parametrize(
    "mutation", ["newer", "in-place", "replacement", "unattested", "wrong-source"]
)
def test_ensure_backend_binary_refreshes_feature_tagged_alias_only_from_admitted_cargo_output(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mutation: str,
) -> None:
    exe_suffix = ".exe" if os.name == "nt" else ""
    target_dir = tmp_path / "target" / "dev-fast"
    target_dir.mkdir(parents=True, exist_ok=True)
    backend_bin = target_dir / f"molt-backend.wasm_backend{exe_suffix}"
    cargo_output = target_dir / f"molt-backend{exe_suffix}"
    old_bytes = b"#!/bin/sh\nexit 0\n# old\n"
    new_bytes = b"#!/bin/sh\nexit 0\n# new\n"
    backend_bin.write_bytes(old_bytes)
    cargo_output.write_bytes(old_bytes)
    backend_bin.chmod(0o755)
    cargo_output.chmod(0o755)

    stale_mtime = 315_532_800 * 1_000_000_000
    os.utime(backend_bin, ns=(stale_mtime, stale_mtime))
    os.utime(cargo_output, ns=(stale_mtime, stale_mtime))

    fingerprint = _fingerprint()
    fingerprint_path = cli_backend_binary._backend_fingerprint_path(
        tmp_path, backend_bin, "dev-fast"
    )
    cli._write_runtime_fingerprint(fingerprint_path, fingerprint, artifact=backend_bin)
    if mutation == "replacement":
        replacement = cargo_output.with_name("replacement")
        replacement.write_bytes(new_bytes)
        replacement.chmod(0o755)
        os.replace(replacement, cargo_output)
    else:
        cargo_output.write_bytes(new_bytes)
    if mutation in {"in-place", "replacement"}:
        os.utime(cargo_output, ns=(stale_mtime, stale_mtime))
        assert cargo_output.stat().st_size == backend_bin.stat().st_size
        assert cargo_output.stat().st_mtime_ns == backend_bin.stat().st_mtime_ns
    if mutation != "unattested":
        candidate_fingerprint = dict(fingerprint)
        if mutation == "wrong-source":
            candidate_fingerprint["hash"] = canonical_json_sha256("other-source")
        cli._write_runtime_fingerprint(
            cli_backend_binary._backend_fingerprint_path(
                tmp_path, cargo_output, "dev-fast"
            ),
            candidate_fingerprint,
            artifact=cargo_output,
        )

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
    assert backend_bin.read_bytes() == (
        old_bytes if mutation in {"unattested", "wrong-source"} else new_bytes
    )


@pytest.mark.parametrize(
    "mutation", ["changed-size", "in-place", "replacement", "receipt"]
)
def test_ensure_backend_binary_reuses_exact_probe_validation_token(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mutation: str,
) -> None:
    exe_suffix = ".exe" if os.name == "nt" else ""
    backend_bin = tmp_path / "target" / "dev-fast" / f"molt-backend{exe_suffix}"
    backend_bin.parent.mkdir(parents=True, exist_ok=True)
    backend_bin.write_text("backend-v1", encoding="utf-8")
    backend_bin.chmod(0o755)
    fingerprint = _fingerprint()
    fingerprint_path = cli_backend_binary._backend_fingerprint_path(
        tmp_path,
        backend_bin,
        "dev-fast",
    )
    cli._write_runtime_fingerprint(fingerprint_path, fingerprint, artifact=backend_bin)
    probe_calls = 0

    def fake_backend_fingerprint(*args: object, **kwargs: object) -> dict[str, str]:
        del args, kwargs
        return dict(fingerprint)

    def fake_probe(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        nonlocal probe_calls
        del kwargs
        probe_calls += 1
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(
        cli_backend_binary,
        "_backend_fingerprint",
        fake_backend_fingerprint,
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        fake_probe,
    )

    for _ in range(2):
        before_result = cli_backend_binary._ensure_backend_binary(
            backend_bin,
            cargo_timeout=1.0,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=tmp_path,
            backend_features=("native-backend",),
        )
        assert before_result
    assert probe_calls == 1

    changed = backend_bin
    if mutation == "receipt":
        changed = cli_backend_binary._backend_probe_validation_path(
            tmp_path, backend_bin, "dev-fast"
        )
    before = changed.stat()
    if mutation == "changed-size":
        changed.write_bytes(b"backend-v2-changed")
    elif mutation == "in-place":
        changed.write_bytes(b"backend-v2")
    elif mutation == "replacement":
        replacement = changed.with_name(
            "replacement.exe" if os.name == "nt" else "replacement"
        )
        replacement.write_bytes(b"backend-v2")
        replacement.chmod(0o755)
        os.replace(replacement, changed)
    else:
        old_payload = changed.read_bytes()
        new_payload = old_payload.replace(b'"native"', b'"wasmxx"')
        assert new_payload != old_payload
        changed.write_bytes(new_payload)
    if mutation != "changed-size":
        assert changed.stat().st_size == before.st_size
        os.utime(changed, ns=(before.st_atime_ns, before.st_mtime_ns))
    if mutation != "receipt":
        # An admitted new build has current source/content provenance, but its
        # previous feature-probe receipt must not authorize the changed bytes.
        cli._write_runtime_fingerprint(
            fingerprint_path, fingerprint, artifact=backend_bin
        )
    after_result = cli_backend_binary._ensure_backend_binary(
        backend_bin,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        backend_features=("native-backend",),
    )
    assert after_result
    assert probe_calls == 2
    assert before_result.cache_compiler_fingerprint
    assert after_result.cache_compiler_fingerprint
    assert (
        after_result.cache_compiler_fingerprint
        == before_result.cache_compiler_fingerprint
    ) is (mutation == "receipt")
    propagated = _backend_environment_with_compiler_fingerprint(
        {}, after_result.cache_compiler_fingerprint
    )
    assert (
        propagated["MOLT_BACKEND_COMPILER_FINGERPRINT"]
        == after_result.cache_compiler_fingerprint
    )


def test_ensure_backend_binary_returns_cargo_failure_detail(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    backend_bin = tmp_path / "target" / "release-fast" / "molt-backend"
    fingerprint = _fingerprint()

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
    assert result.command[4:6] == ("--bin", "molt-backend")
    assert "Backend cargo build failed (exit 101)" in result.message
    assert "duplicate symbol: PyMemoryView_FromMemory" in result.message


@pytest.mark.parametrize("replace", [False, True])
def test_backend_probe_cannot_publish_receipt_for_binary_changed_during_probe(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, replace: bool
) -> None:
    name = "molt-backend.exe" if os.name == "nt" else "molt-backend"
    backend_bin = tmp_path / "target" / "dev-fast" / name
    backend_bin.parent.mkdir(parents=True)
    backend_bin.write_bytes(b"backend-v1")
    backend_bin.chmod(0o755)
    fingerprint = _fingerprint()
    fingerprint_path = cli_backend_binary._backend_fingerprint_path(
        tmp_path, backend_bin, "dev-fast"
    )
    cli._write_runtime_fingerprint(fingerprint_path, fingerprint, artifact=backend_bin)
    probe_calls = 0

    def mutate_during_probe(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        nonlocal probe_calls
        del kwargs
        probe_calls += 1
        metadata = backend_bin.stat()
        if replace:
            replacement = backend_bin.with_name("replacement")
            replacement.write_bytes(b"backend-v2")
            replacement.chmod(0o755)
            os.replace(replacement, backend_bin)
        else:
            backend_bin.write_bytes(b"backend-v2")
        os.utime(backend_bin, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: dict(fingerprint)
    )
    monkeypatch.setattr(
        cli_backend_binary, "_run_subprocess_captured_to_tempfiles", mutate_during_probe
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_maybe_hydrate_artifact_from_canonical_target",
        lambda **_k: False,
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_cargo_with_sccache_retry",
        lambda cmd, **_k: subprocess.CompletedProcess(
            cmd, 101, "", "fixture rebuild refused"
        ),
    )
    result = cli_backend_binary._ensure_backend_binary(
        backend_bin,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        backend_features=("native-backend",),
    )
    assert probe_calls
    assert not result
    assert not cli_backend_binary._backend_probe_validation_path(
        tmp_path, backend_bin, "dev-fast"
    ).exists()


def test_backend_probe_publication_failure_is_typed_without_rebuild(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    name = "molt-backend.exe" if os.name == "nt" else "molt-backend"
    backend_bin = tmp_path / "target" / "dev-fast" / name
    backend_bin.parent.mkdir(parents=True)
    backend_bin.write_bytes(b"backend")
    fingerprint = _fingerprint()
    cli._write_runtime_fingerprint(
        cli_backend_binary._backend_fingerprint_path(tmp_path, backend_bin, "dev-fast"),
        fingerprint,
        artifact=backend_bin,
    )
    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: fingerprint
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **_k: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )

    def reject_publication(*_args: object, **_kwargs: object) -> None:
        raise PermissionError("receipt directory is read-only")

    def reject_rebuild(*_args: object, **_kwargs: object) -> None:
        raise AssertionError("publication failure must not rebuild")

    monkeypatch.setattr(cli_backend_binary, "_atomic_write_json", reject_publication)
    monkeypatch.setattr(
        cli_backend_binary, "_run_cargo_with_sccache_retry", reject_rebuild
    )
    stages: dict[str, float] = {}
    result = cli_backend_binary._ensure_backend_binary(
        backend_bin,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        backend_features=("native-backend",),
        stage_timings_ms=stages,
    )
    assert not result
    assert result.phase == "backend_probe_publication"
    assert "receipt directory is read-only" in result.message
    assert stages["backend_binary_probe"] >= 0


@pytest.mark.parametrize("receipt", ["missing", "source-only", "stale-content"])
def test_unattested_prebuilt_backend_requires_cargo_and_publishes_both_receipts(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, receipt: str
) -> None:
    suffix = ".exe" if os.name == "nt" else ""
    cargo_output = tmp_path / "target" / "dev-fast" / f"molt-backend{suffix}"
    backend_bin = cargo_output.with_name(f"molt-backend.native_backend{suffix}")
    cargo_output.parent.mkdir(parents=True)
    cargo_output.write_bytes(b"backend-v1")
    fingerprint = _fingerprint()
    cargo_receipt = cli_backend_binary._backend_fingerprint_path(
        tmp_path, cargo_output, "dev-fast"
    )
    if receipt != "missing":
        cli._write_runtime_fingerprint(
            cargo_receipt,
            fingerprint,
            artifact=cargo_output if receipt == "stale-content" else None,
        )
    metadata = cargo_output.stat()
    cargo_output.write_bytes(b"backend-v2")
    os.utime(cargo_output, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
    calls = 0

    def fake_cargo(
        cmd: list[str], **_kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        nonlocal calls
        calls += 1
        cargo_output.write_bytes(b"backend-v3")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: fingerprint
    )
    monkeypatch.setattr(cli_backend_binary, "_run_cargo_with_sccache_retry", fake_cargo)
    monkeypatch.setattr(cli_backend_binary, "_codesign_binary", lambda _p: None)
    monkeypatch.setattr(
        cli_backend_binary,
        "_maybe_hydrate_artifact_from_canonical_target",
        lambda **_k: False,
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **_k: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )
    for _ in range(2):
        assert cli_backend_binary._ensure_backend_binary(
            backend_bin,
            cargo_timeout=1.0,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=tmp_path,
            backend_features=("native-backend",),
        )
    assert calls == 1
    assert backend_bin.read_bytes() == b"backend-v3"
    for artifact in (cargo_output, backend_bin):
        assert cli_backend_binary._runtime_artifact_fingerprint_matches(
            artifact,
            fingerprint,
            cli_backend_binary._backend_fingerprint_path(
                tmp_path, artifact, "dev-fast"
            ),
            require_artifact_digest=True,
        )


@pytest.mark.parametrize("admitted_source", [False, True])
def test_backend_alias_replacement_during_publication_cannot_acquire_provenance(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, admitted_source: bool
) -> None:
    suffix = ".exe" if os.name == "nt" else ""
    cargo_output = tmp_path / "target" / "dev-fast" / f"molt-backend{suffix}"
    backend_bin = cargo_output.with_name(f"molt-backend.native_backend{suffix}")
    cargo_output.parent.mkdir(parents=True)
    cargo_output.write_bytes(b"backend-v1")
    fingerprint = _fingerprint()
    alias_receipt = cli_backend_binary._backend_fingerprint_path(
        tmp_path, backend_bin, "dev-fast"
    )
    if admitted_source:
        cli._write_runtime_fingerprint(
            cli_backend_binary._backend_fingerprint_path(
                tmp_path, cargo_output, "dev-fast"
            ),
            fingerprint,
            artifact=cargo_output,
        )
    write_fingerprint = cli_backend_binary._write_runtime_fingerprint

    def replace_before_publication(
        path: Path, payload: dict[str, object], **kwargs: object
    ) -> None:
        if path == alias_receipt:
            metadata = backend_bin.stat()
            replacement = backend_bin.with_name("replacement")
            replacement.write_bytes(b"backend-v2")
            os.replace(replacement, backend_bin)
            os.utime(backend_bin, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
        write_fingerprint(path, payload, **kwargs)

    monkeypatch.setattr(
        cli_backend_binary, "_write_runtime_fingerprint", replace_before_publication
    )
    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: fingerprint
    )
    monkeypatch.setattr(cli_backend_binary, "_codesign_binary", lambda _p: None)
    monkeypatch.setattr(
        cli_backend_binary,
        "_maybe_hydrate_artifact_from_canonical_target",
        lambda **_k: False,
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_cargo_with_sccache_retry",
        lambda cmd, **_k: subprocess.CompletedProcess(cmd, 0, "", ""),
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **_k: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )
    result = cli_backend_binary._ensure_backend_binary(
        backend_bin,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        backend_features=("native-backend",),
    )
    assert not result
    assert result.phase == (
        "backend_alias_publication"
        if admitted_source
        else "backend_artifact_publication"
    )
    assert not cli_backend_binary._runtime_artifact_fingerprint_matches(
        backend_bin, fingerprint, alias_receipt, require_artifact_digest=True
    )
