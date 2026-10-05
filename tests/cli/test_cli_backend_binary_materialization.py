from __future__ import annotations

from tests.compiler_identity_helper import (
    compiler_build_admission,
    stub_compiler_admission,
    write_compiler_source,
)

import os
import subprocess
from pathlib import Path

import molt.cli as cli
from molt.cli import backend_binary as cli_backend_binary
from molt.cli import backend_execution as cli_backend_execution
from molt.backend_executable_names import backend_executable_name
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


@pytest.mark.parametrize("os_name,suffix", [("nt", ".exe"), ("posix", "")])
def test_every_backend_variant_is_distinct_from_cargo_output(os_name, suffix) -> None:
    raw = backend_executable_name(os_name=os_name)
    assert raw == f"molt-backend{suffix}"
    assert backend_executable_name(os_name=os_name, features=("native-backend",)) == (
        f"molt-backend.native_backend{suffix}"
    )
    variants = {
        backend_executable_name(os_name=os_name, features=features)
        for features in (
            (),
            ("native-backend",),
            ("wasm-backend",),
            ("rust-backend",),
            ("luau-backend",),
            ("native-backend", "llvm"),
        )
    }
    assert len(variants) == 6 and raw not in variants


@pytest.mark.parametrize("disable_rebuild_after_publication", [False, True])
def test_target_switch_preserves_admitted_native_and_wasm_compilers(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    disable_rebuild_after_publication: bool,
) -> None:
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "target"))
    monkeypatch.delenv("MOLT_SKIP_RUNTIME_REBUILD", raising=False)
    native = ("native-backend",)
    wasm = ("wasm-backend",)
    builds: list[tuple[str, ...]] = []
    identities: dict[tuple[str, ...], str | None] = {}

    def fingerprint(_root, *, backend_features, **_kwargs):
        return {
            **_fingerprint(),
            "hash": canonical_json_sha256(list(backend_features)),
        }

    def build(cmd, **_kwargs):
        features = tuple(cmd[cmd.index("--features") + 1].split(","))
        builds.append(features)
        selected = cli_backend_execution._backend_bin_path(
            tmp_path, "dev-fast", features
        )
        output = selected.with_name(backend_executable_name(os_name=os.name))
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(("compiled:" + ",".join(features)).encode())
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_backend_binary, "_backend_fingerprint", fingerprint)
    monkeypatch.setattr(cli_backend_binary, "_run_resolved_cargo_plan", build)
    monkeypatch.setattr(cli_backend_binary, "_codesign_binary", lambda _p: None)
    monkeypatch.setattr(
        cli_backend_binary,
        "_maybe_hydrate_artifact_from_canonical_target",
        lambda **_kwargs: False,
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **_kwargs: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )
    for index, features in enumerate((native, wasm, native, wasm)):
        if index == 2 and disable_rebuild_after_publication:
            monkeypatch.setenv("MOLT_SKIP_RUNTIME_REBUILD", "1")
        selected = cli_backend_execution._backend_bin_path(
            tmp_path, "dev-fast", features
        )
        result = cli_backend_binary._ensure_backend_binary(
            selected,
            cargo_timeout=1,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=tmp_path,
            backend_features=features,
        )
        assert result, result.message
        assert selected.read_bytes() == ("compiled:" + ",".join(features)).encode()
        previous = identities.setdefault(features, result.cache_compiler_fingerprint)
        assert previous == result.cache_compiler_fingerprint
    assert builds == [native, wasm]
    assert identities[native] != identities[wasm]


@pytest.mark.parametrize(
    "state", ["missing", "unattested", "wrong-source", "corrupt", "probe-failure"]
)
def test_rebuild_disabled_still_admits_backend_identity_and_features(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, state: str
) -> None:
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "target"))
    monkeypatch.setenv("MOLT_SKIP_RUNTIME_REBUILD", "1")
    features = ("native-backend",)
    selected = cli_backend_execution._backend_bin_path(tmp_path, "dev-fast", features)
    selected.parent.mkdir(parents=True, exist_ok=True)
    fingerprint = _fingerprint()
    if state != "missing":
        selected.write_bytes(b"unverified compiler bytes")
        selected.chmod(0o755)
        if state != "unattested":
            stored = dict(fingerprint)
            if state == "wrong-source":
                stored["hash"] = canonical_json_sha256("old-source")
            cli._write_runtime_fingerprint(
                cli_backend_binary._backend_fingerprint_path(
                    tmp_path, selected, "dev-fast"
                ),
                stored,
                artifact=selected,
            )
        if state == "corrupt":
            selected.write_bytes(b"unexpected compiler bytes")

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: fingerprint
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_maybe_hydrate_artifact_from_canonical_target",
        lambda **_k: False,
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_resolved_cargo_plan",
        lambda *_a, **_k: pytest.fail("rebuild-disabled policy invoked Cargo"),
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **_k: subprocess.CompletedProcess(
            cmd, 1, b"", b"requested backend feature is absent"
        ),
    )
    result = cli_backend_binary._ensure_backend_binary(
        selected,
        cargo_timeout=1,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=tmp_path,
        backend_features=features,
    )
    assert not result and result.phase == "rebuild-policy"
    assert "MOLT_SKIP_RUNTIME_REBUILD=1" in result.message
    assert result.cache_compiler_fingerprint is None
    if state == "probe-failure":
        assert "requested backend feature is absent" in result.message


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
    assert baseline == cli_backend_binary._backend_compiler_cache_fingerprint(
        {**fingerprint, "inputs_digest": canonical_json_sha256("touched-inputs")},
        binary_identity,
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


@pytest.mark.parametrize("change", ["touch", "content"])
def test_backend_refresh_distinguishes_source_metadata_from_content(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, change: str
) -> None:
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
    from molt.cli import runtime_fingerprints

    source = tmp_path / "backend.rs"
    source.write_text("fn main() {}\n", encoding="utf-8")
    monkeypatch.setattr(
        cli_backend_binary, "_backend_source_paths", lambda *args: [source]
    )
    monkeypatch.setattr(
        cli_backend_binary, "_compiler_clean_source_state", lambda *args: None
    )

    def fingerprint(stored=None):
        return cli_backend_binary._backend_fingerprint(
            tmp_path,
            cargo_profile="dev-fast",
            build_admission=compiler_build_admission(environment={"RUSTFLAGS": ""}),
            backend_features=("native-backend",),
            stored_fingerprint=stored,
        )

    original = fingerprint()
    assert original is not None and original["inputs_digest"] is not None
    artifact = tmp_path / "backend.exe"
    artifact.write_bytes(b"admitted backend bytes")
    sidecar = tmp_path / "backend.fingerprint"
    runtime_fingerprints._write_runtime_fingerprint(
        sidecar, original, artifact=artifact
    )
    before = runtime_fingerprints._read_runtime_fingerprint(sidecar)
    assert before is not None
    if change == "content":
        source.write_text("fn main() { panic!(); }\n", encoding="utf-8")
    metadata = source.stat()
    os.utime(source, ns=(metadata.st_atime_ns, metadata.st_mtime_ns + 1_000_000_000))
    current = fingerprint(before)
    assert current is not None and current["inputs_digest"] != original["inputs_digest"]
    admitted = runtime_fingerprints._runtime_artifact_fingerprint_matches(
        artifact,
        current,
        sidecar,
        require_artifact_digest=True,
    )
    if change == "content":
        assert not admitted
        with pytest.raises(ValueError, match="semantic identity"):
            runtime_fingerprints._refresh_runtime_fingerprint_metadata(sidecar, current)
        assert runtime_fingerprints._read_runtime_fingerprint(sidecar) == before
    else:
        assert admitted
        runtime_fingerprints._refresh_runtime_fingerprint_metadata(sidecar, current)
        after = runtime_fingerprints._read_runtime_fingerprint(sidecar)
        assert after is not None
        assert after["inputs_digest"] == current["inputs_digest"]
        assert after["artifact_content_identity"] == before["artifact_content_identity"]


@pytest.mark.parametrize(
    "mutation", ["newer", "in-place", "replacement", "unattested", "wrong-source"]
)
def test_ensure_backend_binary_refreshes_feature_tagged_alias_only_from_admitted_cargo_output(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mutation: str,
) -> None:
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
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
    monkeypatch.setattr(cli_backend_binary, "_run_resolved_cargo_plan", fail_run_cargo)
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
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
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
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
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
        "_run_resolved_cargo_plan",
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
    assert result.command[:7] == (
        "cargo",
        "build",
        "--locked",
        "--package",
        "molt-backend",
        "--bin",
        "molt-backend",
    )
    assert "Backend cargo build failed (exit 101)" in result.message
    assert "duplicate symbol: PyMemoryView_FromMemory" in result.message


@pytest.mark.parametrize("replace", [False, True])
def test_backend_probe_cannot_publish_receipt_for_binary_changed_during_probe(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, replace: bool
) -> None:
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
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
        "_run_resolved_cargo_plan",
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
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
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
    monkeypatch.setattr(cli_backend_binary, "_run_resolved_cargo_plan", reject_rebuild)
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
@pytest.mark.parametrize("probe_outcome", ["pass", "reject", "timeout"])
def test_backend_build_publishes_provenance_only_after_successful_probe(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    receipt: str,
    probe_outcome: str,
) -> None:
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
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
    probe_commands = []

    def fake_cargo(
        cmd: list[str], **_kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        nonlocal calls
        calls += 1
        cargo_output.write_bytes(b"backend-v3")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    def probe(cmd, **_kwargs):
        probe_commands.append(tuple(cmd))
        if probe_outcome == "timeout":
            raise subprocess.TimeoutExpired(
                cmd, 10, output=b"partial probe output", stderr=b"partial probe error"
            )
        return subprocess.CompletedProcess(
            cmd,
            0 if probe_outcome == "pass" else 17,
            b"",
            b"" if probe_outcome == "pass" else b"original feature rejection",
        )

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: fingerprint
    )
    monkeypatch.setattr(cli_backend_binary, "_run_resolved_cargo_plan", fake_cargo)
    monkeypatch.setattr(cli_backend_binary, "_codesign_binary", lambda _p: None)
    monkeypatch.setattr(
        cli_backend_binary,
        "_maybe_hydrate_artifact_from_canonical_target",
        lambda **_k: False,
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        probe,
    )
    for _ in range(2 if probe_outcome == "pass" else 1):
        result = cli_backend_binary._ensure_backend_binary(
            backend_bin,
            cargo_timeout=1.0,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=tmp_path,
            backend_features=("native-backend",),
        )
        assert bool(result) == (probe_outcome == "pass")
        if not result:
            assert result.phase == "backend_feature_probe"
            assert result.command == probe_commands[-1]
            assert result.cache_compiler_fingerprint is None
            if probe_outcome == "reject":
                assert result.returncode == 17
                assert "original feature rejection" in result.message
            else:
                assert "timed out" in result.message
                assert "partial probe output" in result.message
                assert "partial probe error" in result.message
    assert calls == 1
    assert len(probe_commands) == 1
    assert backend_bin.read_bytes() == b"backend-v3"
    for artifact in (cargo_output, backend_bin):
        assert cli_backend_binary._runtime_artifact_fingerprint_matches(
            artifact,
            fingerprint,
            cli_backend_binary._backend_fingerprint_path(
                tmp_path, artifact, "dev-fast"
            ),
            require_artifact_digest=True,
        ) == (probe_outcome == "pass")


@pytest.mark.parametrize("admitted_source", [False, True])
def test_backend_alias_replacement_during_publication_cannot_acquire_provenance(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, admitted_source: bool
) -> None:
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
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
        "_run_resolved_cargo_plan",
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
