from __future__ import annotations

from tests.cli.backend_generation_test_support import publish_backend, selected_backend
from tests.compiler_identity_helper import (
    compiler_build_admission,
    stub_compiler_admission,
    write_compiler_source,
)

import hashlib
import json
import os
import subprocess
from pathlib import Path

from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.cli import backend_binary as cli_backend_binary
from molt.cli import backend_execution as cli_backend_execution
from molt.cli import generation_store
from molt.cli.backend_generation import (
    backend_generation_path,
    backend_generation_store_root,
)
from molt.backend_executable_names import backend_executable_name
from molt.cli.backend_execution import _backend_environment_with_compiler_fingerprint
from molt.exact_json import canonical_json_sha256
from molt.toolchain_identity import (
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
import pytest
from molt.cli import runtime_fingerprints as cli_runtime_fingerprints


def _fingerprint() -> dict[str, str]:
    return {
        "hash": canonical_json_sha256("backend-source"),
        "rustc": "rustc-1",
        "inputs_digest": canonical_json_sha256("backend-inputs"),
        "meta_digest": canonical_json_sha256("backend-meta"),
    }


def _receipt(project_root: Path, coordinate: Path, profile: str = "dev-fast") -> Path:
    return cli_backend_binary._backend_fingerprint_path(
        project_root, coordinate, profile
    )


def _generation(coordinate: Path, data: bytes) -> Path:
    """Where the CLI must publish ``data``: an independent content address."""
    return backend_generation_path(coordinate, hashlib.sha256(data).hexdigest())


def _ensure(
    coordinate: Path,
    project_root: Path,
    features: tuple[str, ...],
    *,
    profile: str = "dev-fast",
    **kwargs,
):
    return cli_backend_binary._ensure_backend_binary(
        coordinate,
        cargo_timeout=kwargs.pop("cargo_timeout", 1.0),
        json_output=True,
        cargo_profile=profile,
        project_root=project_root,
        backend_features=features,
        rust_cpu=RustCpuPolicy.BASELINE,
        **kwargs,
    )


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


def test_cargo_output_is_never_a_backend_coordinate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
    cargo_output = (
        tmp_path / "target" / "dev-fast" / backend_executable_name(os_name=os.name)
    )
    result = _ensure(cargo_output, tmp_path, ("native-backend",))
    assert not result and result.phase == "backend_artifact"
    assert "Cargo's shared output" in result.message


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
    admitted: dict[tuple[str, ...], tuple[str | None, Path | None]] = {}

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
    monkeypatch.setattr(
        cli_backend_binary,
        "_hydrate_canonical_backend_generation",
        lambda *_a, **_k: None,
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
        result = _ensure(selected, tmp_path, features)
        assert result, result.message
        assert result.binary is not None
        expected = ("compiled:" + ",".join(features)).encode()
        assert result.binary.read_bytes() == expected
        assert result.binary.samefile(_generation(selected, expected))
        previous = admitted.setdefault(
            features, (result.cache_compiler_fingerprint, result.binary)
        )
        assert previous == (result.cache_compiler_fingerprint, result.binary)
    assert builds == [native, wasm]
    assert admitted[native][0] != admitted[wasm][0]


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
    fingerprint = _fingerprint()
    if state != "missing":
        stored = dict(fingerprint)
        if state == "wrong-source":
            stored["hash"] = canonical_json_sha256("old-source")
        generation = publish_backend(
            selected,
            b"unverified compiler bytes",
            receipt=None if state == "unattested" else _receipt(tmp_path, selected),
            fingerprint=stored,
        )
        if state == "corrupt":
            generation.write_bytes(b"unexpected compiler bytes")

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: fingerprint
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_hydrate_canonical_backend_generation",
        lambda *_a, **_k: None,
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
    result = _ensure(selected, tmp_path, features)
    assert not result and result.phase == "rebuild-policy"
    assert "MOLT_SKIP_RUNTIME_REBUILD=1" in result.message
    assert result.cache_compiler_fingerprint is None
    if state == "probe-failure":
        assert "requested backend feature is absent" in result.message


# A never-seen backend binary's first run measured 11.4 s on macOS at load 7
# (the operating system checks a new executable once), then 0.2 s (HF-233).
_SLOW_FIRST_RUN_S = 11.4


@pytest.mark.parametrize("cargo_timeout", [None, 600.0, 5.0])
def test_slow_first_probe_run_shares_the_build_budget_and_never_rebuilds(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, cargo_timeout: float | None
) -> None:
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "target"))
    features = ("native-backend",)
    selected = cli_backend_execution._backend_bin_path(tmp_path, "dev-fast", features)
    fingerprint = _fingerprint()
    publish_backend(
        selected,
        b"attested compiler bytes",
        receipt=_receipt(tmp_path, selected),
        fingerprint=fingerprint,
    )
    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: fingerprint
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_hydrate_canonical_backend_generation",
        lambda *_a, **_k: None,
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_resolved_cargo_plan",
        lambda *_a, **_k: pytest.fail("a slow probe run rebuilt the backend"),
    )

    def first_run(cmd: list[str], **kwargs: object):
        limit = kwargs["timeout"]
        if limit is not None and float(limit) < _SLOW_FIRST_RUN_S:
            raise subprocess.TimeoutExpired(cmd, limit)
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(
        cli_backend_binary, "_run_subprocess_captured_to_tempfiles", first_run
    )
    result = _ensure(selected, tmp_path, features, cargo_timeout=cargo_timeout)
    if cargo_timeout is None or cargo_timeout >= _SLOW_FIRST_RUN_S:
        assert result, result.message
    else:
        # A timeout is its own failure class, not a capability refusal.
        assert not result and result.phase == "backend_feature_probe_timeout"
        assert "within 5.0s (MOLT_CARGO_TIMEOUT)" in result.message


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
def test_ensure_backend_binary_selects_only_an_admitted_cargo_output(
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
    fingerprint = _fingerprint()
    old_generation = publish_backend(
        backend_bin,
        old_bytes,
        receipt=_receipt(tmp_path, backend_bin),
        fingerprint=fingerprint,
    )
    held = stable_regular_file_identity(old_generation, label="held backend")
    cargo_output.write_bytes(old_bytes)
    cargo_output.chmod(0o755)
    stale_mtime = 315_532_800 * 1_000_000_000
    os.utime(cargo_output, ns=(stale_mtime, stale_mtime))
    if mutation == "replacement":
        replacement = cargo_output.with_name("replacement")
        replacement.write_bytes(new_bytes)
        replacement.chmod(0o755)
        os.replace(replacement, cargo_output)
    else:
        cargo_output.write_bytes(new_bytes)
    if mutation in {"in-place", "replacement"}:
        os.utime(cargo_output, ns=(stale_mtime, stale_mtime))
    if mutation != "unattested":
        candidate_fingerprint = dict(fingerprint)
        if mutation == "wrong-source":
            candidate_fingerprint["hash"] = canonical_json_sha256("other-source")
        cli_runtime_fingerprints._write_runtime_fingerprint(
            _receipt(tmp_path, cargo_output),
            candidate_fingerprint,
            artifact=cargo_output,
        )

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: dict(fingerprint)
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_resolved_cargo_plan",
        lambda *_a, **_k: pytest.fail("unexpected cargo rebuild"),
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **kwargs: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )

    result = _ensure(backend_bin, tmp_path, ("wasm-backend",))
    assert result, result.message
    assert result.binary is not None
    expected = old_bytes if mutation in {"unattested", "wrong-source"} else new_bytes
    assert result.binary.read_bytes() == expected
    assert result.binary.samefile(_generation(backend_bin, expected))
    # Selecting the Cargo output published a new generation; the old one, and
    # what a build captured of it, did not change.
    verify_stable_regular_file_identity(held, label="held backend")
    assert old_generation.read_bytes() == old_bytes


@pytest.mark.parametrize("mutation", ["same-size", "other-size", "receipt"])
def test_ensure_backend_binary_reuses_exact_probe_validation_token(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mutation: str,
) -> None:
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
    exe_suffix = ".exe" if os.name == "nt" else ""
    backend_bin = (
        tmp_path / "target" / "dev-fast" / f"molt-backend.native_backend{exe_suffix}"
    )
    fingerprint = _fingerprint()
    fingerprint_path = _receipt(tmp_path, backend_bin)
    publish_backend(
        backend_bin, b"backend-v1", receipt=fingerprint_path, fingerprint=fingerprint
    )
    probe_calls = 0

    def fake_probe(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        nonlocal probe_calls
        del kwargs
        probe_calls += 1
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: dict(fingerprint)
    )
    monkeypatch.setattr(
        cli_backend_binary, "_run_subprocess_captured_to_tempfiles", fake_probe
    )

    for _ in range(2):
        before_result = _ensure(backend_bin, tmp_path, ("native-backend",))
        assert before_result
    assert probe_calls == 1

    if mutation == "receipt":
        changed = cli_backend_binary._backend_probe_validation_path(
            tmp_path, backend_bin, "dev-fast"
        )
        old_payload = changed.read_bytes()
        new_payload = old_payload.replace(b'"native"', b'"wasmxx"')
        assert new_payload != old_payload
        changed.write_bytes(new_payload)
    else:
        # An admitted new build selects new bytes with current source/content
        # provenance; the previous feature-probe receipt must not authorize it.
        publish_backend(
            backend_bin,
            b"backend-v2" if mutation == "same-size" else b"backend-v2-changed",
            receipt=fingerprint_path,
            fingerprint=fingerprint,
        )
    after_result = _ensure(backend_bin, tmp_path, ("native-backend",))
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


def test_corrupted_generation_is_never_admitted(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Bytes that no longer hash to their address are not the selected compiler."""
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
    suffix = ".exe" if os.name == "nt" else ""
    backend_bin = (
        tmp_path / "target" / "dev-fast" / f"molt-backend.native_backend{suffix}"
    )
    cargo_output = backend_bin.with_name(f"molt-backend{suffix}")
    fingerprint = _fingerprint()
    generation = publish_backend(
        backend_bin,
        b"backend-v1",
        receipt=_receipt(tmp_path, backend_bin),
        fingerprint=fingerprint,
    )
    generation.write_bytes(b"backend-XX")
    builds = 0

    def build(cmd, **_kwargs):
        nonlocal builds
        builds += 1
        cargo_output.write_bytes(b"backend-v1")
        cargo_output.chmod(0o755)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: fingerprint
    )
    monkeypatch.setattr(cli_backend_binary, "_run_resolved_cargo_plan", build)
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **_k: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )
    result = _ensure(backend_bin, tmp_path, ("native-backend",))
    assert result, result.message
    assert builds == 1
    assert result.binary is not None and result.binary.read_bytes() == b"backend-v1"


def test_ensure_backend_binary_returns_cargo_failure_detail(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
    backend_bin = tmp_path / "target" / "release-fast" / "molt-backend.native_backend"
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
        rust_cpu=RustCpuPolicy.BASELINE,
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
    suffix = ".exe" if os.name == "nt" else ""
    backend_bin = (
        tmp_path / "target" / "dev-fast" / f"molt-backend.native_backend{suffix}"
    )
    fingerprint = _fingerprint()
    generation = publish_backend(
        backend_bin,
        b"backend-v1",
        receipt=_receipt(tmp_path, backend_bin),
        fingerprint=fingerprint,
    )
    probe_calls = 0

    def mutate_during_probe(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        nonlocal probe_calls
        del kwargs
        probe_calls += 1
        metadata = generation.stat()
        if replace:
            replacement = generation.with_name("replacement")
            replacement.write_bytes(b"backend-v2")
            replacement.chmod(0o755)
            os.replace(replacement, generation)
        else:
            generation.write_bytes(b"backend-v2")
        os.utime(generation, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: dict(fingerprint)
    )
    monkeypatch.setattr(
        cli_backend_binary, "_run_subprocess_captured_to_tempfiles", mutate_during_probe
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_hydrate_canonical_backend_generation",
        lambda *_a, **_k: None,
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_resolved_cargo_plan",
        lambda cmd, **_k: subprocess.CompletedProcess(
            cmd, 101, "", "fixture rebuild refused"
        ),
    )
    result = _ensure(backend_bin, tmp_path, ("native-backend",))
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
    suffix = ".exe" if os.name == "nt" else ""
    backend_bin = (
        tmp_path / "target" / "dev-fast" / f"molt-backend.native_backend{suffix}"
    )
    fingerprint = _fingerprint()
    publish_backend(
        backend_bin,
        b"backend",
        receipt=_receipt(tmp_path, backend_bin),
        fingerprint=fingerprint,
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
    result = _ensure(
        backend_bin, tmp_path, ("native-backend",), stage_timings_ms=stages
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
    cargo_receipt = _receipt(tmp_path, cargo_output)
    if receipt != "missing":
        cli_runtime_fingerprints._write_runtime_fingerprint(
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
    monkeypatch.setattr(
        cli_backend_binary,
        "_hydrate_canonical_backend_generation",
        lambda *_a, **_k: None,
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        probe,
    )
    for _ in range(2 if probe_outcome == "pass" else 1):
        result = _ensure(backend_bin, tmp_path, ("native-backend",))
        assert bool(result) == (probe_outcome == "pass")
        if not result:
            # A timeout is its own failure class, distinct from a refusal.
            assert result.phase == (
                "backend_feature_probe_timeout"
                if probe_outcome == "timeout"
                else "backend_feature_probe"
            )
            assert result.command == probe_commands[-1]
            assert result.cache_compiler_fingerprint is None
            if probe_outcome == "reject":
                assert result.returncode == 17
                assert "original feature rejection" in result.message
            else:
                assert "did not finish within" in result.message
                assert "partial probe output" in result.message
                assert "partial probe error" in result.message
    assert calls == 1
    assert len(probe_commands) == 1
    # The probe ran the published generation: the file a build would run.
    generation = _generation(backend_bin, b"backend-v3")
    assert Path(probe_commands[0][0]).samefile(generation)
    assert generation.read_bytes() == b"backend-v3"
    assert cli_backend_binary._runtime_artifact_fingerprint_matches(
        cargo_output, fingerprint, cargo_receipt, require_artifact_digest=True
    ) == (probe_outcome == "pass")
    selection = cli_backend_binary._selected_backend_generation_matches(
        backend_bin, fingerprint, _receipt(tmp_path, backend_bin)
    )
    assert (selection is not None) == (probe_outcome == "pass")
    if selection is not None:
        assert selection.samefile(generation)


@pytest.mark.parametrize("admitted_source", [False, True])
def test_generation_replaced_during_selection_cannot_acquire_provenance(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, admitted_source: bool
) -> None:
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
    suffix = ".exe" if os.name == "nt" else ""
    cargo_output = tmp_path / "target" / "dev-fast" / f"molt-backend{suffix}"
    backend_bin = cargo_output.with_name(f"molt-backend.native_backend{suffix}")
    cargo_output.parent.mkdir(parents=True)
    cargo_output.write_bytes(b"backend-v1")
    cargo_output.chmod(0o755)
    fingerprint = _fingerprint()
    selection_receipt = _receipt(tmp_path, backend_bin)
    generation = _generation(backend_bin, b"backend-v1")
    if admitted_source:
        cli_runtime_fingerprints._write_runtime_fingerprint(
            _receipt(tmp_path, cargo_output),
            fingerprint,
            artifact=cargo_output,
        )
    write_fingerprint = cli_backend_binary._write_runtime_fingerprint

    def replace_before_selection(
        path: Path, payload: dict[str, object], **kwargs: object
    ) -> None:
        if path == selection_receipt:
            metadata = generation.stat()
            replacement = generation.with_name("replacement")
            replacement.write_bytes(b"backend-v2")
            os.replace(replacement, generation)
            os.utime(generation, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
        write_fingerprint(path, payload, **kwargs)

    monkeypatch.setattr(
        cli_backend_binary, "_write_runtime_fingerprint", replace_before_selection
    )
    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: fingerprint
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_hydrate_canonical_backend_generation",
        lambda *_a, **_k: None,
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
    result = _ensure(backend_bin, tmp_path, ("native-backend",))
    assert not result
    assert result.phase == (
        "backend_selection_publication"
        if admitted_source
        else "backend_artifact_publication"
    )
    assert (
        cli_backend_binary._selected_backend_generation_matches(
            backend_bin, fingerprint, selection_receipt
        )
        is None
    )


def test_warm_build_reuses_the_exact_cargo_bytes_without_republication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """An unchanged Cargo output must leave the admitted generation alone.

    Re-signing a published copy on macOS rewrote its bytes, so the "copy
    equals the Cargo output" freshness test never held: every build re-copied,
    re-probed and published a new compiler identity, which re-keyed every
    backend and link cache. Signing hooks below mutate bytes the way codesign
    does.
    """
    from molt.cli import atomic_io

    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "target"))
    monkeypatch.delenv("MOLT_SKIP_RUNTIME_REBUILD", raising=False)
    features = ("wasm-backend",)
    selected = cli_backend_execution._backend_bin_path(tmp_path, "dev-fast", features)
    cargo_output = selected.with_name(backend_executable_name(os_name=os.name))
    builds: list[tuple[str, ...]] = []
    probes: list[list[str]] = []

    def build(cmd, **_kwargs):  # type: ignore[no-untyped-def]
        builds.append(tuple(cmd))
        cargo_output.parent.mkdir(parents=True, exist_ok=True)
        cargo_output.write_bytes(b"compiled:wasm-backend")
        cargo_output.chmod(0o755)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    def probe(cmd, **_kwargs):  # type: ignore[no-untyped-def]
        probes.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    def rewriting_signature(stage: Path, *_destination: Path) -> None:
        stage.write_bytes(stage.read_bytes() + os.urandom(8))

    monkeypatch.setattr(
        cli_backend_binary,
        "_backend_fingerprint",
        lambda _root, **_kwargs: _fingerprint(),
    )
    monkeypatch.setattr(cli_backend_binary, "_run_resolved_cargo_plan", build)
    monkeypatch.setattr(
        cli_backend_binary,
        "_hydrate_canonical_backend_generation",
        lambda *_a, **_k: None,
    )
    monkeypatch.setattr(
        cli_backend_binary, "_run_subprocess_captured_to_tempfiles", probe
    )
    monkeypatch.setattr(atomic_io, "_codesign_atomic_copy_temp", rewriting_signature)

    def ensure():
        result = _ensure(selected, tmp_path, features, cargo_timeout=1)
        assert result, result.message
        return result

    first = ensure()
    assert len(builds) == 1
    assert first.binary is not None
    assert first.binary.read_bytes() == cargo_output.read_bytes()
    generation_stat = first.binary.stat()
    probes.clear()

    second = ensure()
    assert second.cache_compiler_fingerprint == first.cache_compiler_fingerprint
    assert second.binary == first.binary or second.binary.samefile(first.binary)
    assert builds and len(builds) == 1
    assert probes == []
    assert (first.binary.stat().st_ino, first.binary.stat().st_ctime_ns) == (
        generation_stat.st_ino,
        generation_stat.st_ctime_ns,
    )


def _two_generation_fixture(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    """A lane whose sources change between builds; Cargo writes known bytes."""
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "target"))
    monkeypatch.delenv("MOLT_SKIP_RUNTIME_REBUILD", raising=False)
    features = ("wasm-backend",)
    coordinate = cli_backend_execution._backend_bin_path(tmp_path, "release", features)
    cargo_output = coordinate.with_name(backend_executable_name(os_name=os.name))
    state = {"source": "v1"}

    def fingerprint(_root, **_kwargs):
        return {**_fingerprint(), "hash": canonical_json_sha256(state["source"])}

    def build(cmd, **_kwargs):
        cargo_output.parent.mkdir(parents=True, exist_ok=True)
        cargo_output.write_bytes(f"#!/bin/sh\n# {state['source']}\n".encode())
        cargo_output.chmod(0o755)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(cli_backend_binary, "_backend_fingerprint", fingerprint)
    monkeypatch.setattr(cli_backend_binary, "_run_resolved_cargo_plan", build)
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **_k: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )

    def ensure(source: str):
        state["source"] = source
        result = _ensure(coordinate, tmp_path, features, profile="release")
        assert result, result.message
        assert result.binary is not None
        return result.binary

    return coordinate, ensure


def test_a_later_publication_never_changes_the_generation_a_build_holds(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """HF-244: a build that captured its backend keeps it through republication.

    Build A resolves its backend and captures the identity the WASM link
    later verifies. Build B then rebuilds from changed sources and publishes,
    and build C republishes A's bytes. A's capture still verifies under the
    same check that failed ("WASM link input changed since identity
    capture"), and its path still holds the bytes Cargo wrote for A.
    """
    coordinate, ensure = _two_generation_fixture(tmp_path, monkeypatch)
    a_bytes = b"#!/bin/sh\n# v1\n"
    held_path = ensure("v1")
    held = stable_regular_file_identity(held_path, label="WASM link input")

    b_path = ensure("v2")
    assert b_path.read_bytes() == b"#!/bin/sh\n# v2\n"
    assert not b_path.samefile(held_path)
    verify_stable_regular_file_identity(held, label="WASM link input")
    assert hashlib.sha256(held_path.read_bytes()).hexdigest() == (
        hashlib.sha256(a_bytes).hexdigest()
    )
    # The coordinate itself is never materialized: no build runs it.
    assert not coordinate.exists()

    # Identical bytes reuse the generation A holds without any write.
    c_path = ensure("v1")
    assert c_path.samefile(held_path)
    verify_stable_regular_file_identity(held, label="WASM link input")
    assert selected_backend(tmp_path, coordinate, "release").samefile(held_path)


def test_a_concurrent_build_runs_its_own_generation_while_another_publishes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The held generation stays executable while the selection moves on."""
    coordinate, ensure = _two_generation_fixture(tmp_path, monkeypatch)
    held_path = ensure("v1")
    with held_path.open("rb") as running:
        ensure("v2")
        ensure("v3")
        assert running.read() == b"#!/bin/sh\n# v1\n"
    assert os.access(held_path, os.X_OK)
    names = {path.name for path in backend_generation_store_root(coordinate).iterdir()}
    assert {
        hashlib.sha256(f"#!/bin/sh\n# {seed}\n".encode()).hexdigest()
        for seed in ("v1", "v2", "v3")
    } <= names


class _Clock:
    def __init__(self, now: float) -> None:
        self.now = now

    def time(self) -> float:
        return self.now


_HOUR = 60 * 60


def test_publication_evicts_backend_generations_no_build_used_for_a_day(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Retention keeps every generation a build used within a day.

    Two build-state roots share one Cargo target, so the store's own selection
    names only the last publisher's generation. The other root's build admits
    its generation by reads alone and records that use, which keeps it.
    """
    clock = _Clock(1_000_000.0)
    monkeypatch.setattr(generation_store, "time", clock)
    coordinate, ensure = _two_generation_fixture(tmp_path, monkeypatch)
    store = backend_generation_store_root(coordinate)

    def in_state(name: str) -> None:
        monkeypatch.setenv("MOLT_BUILD_STATE_DIR", str(tmp_path / name))

    in_state("state-a")
    held = ensure("v1")
    clock.now += _HOUR
    in_state("state-b")
    idle = ensure("v2")
    clock.now += _HOUR
    replaced = ensure("v3")
    clock.now += 23 * _HOUR
    in_state("state-a")
    assert ensure("v1").samefile(held)
    clock.now += 2 * _HOUR
    in_state("state-b")
    current = ensure("v4")

    names = {path.name for path in store.iterdir()}
    assert idle.parent.name not in names
    assert {held.parent.name, replaced.parent.name, current.parent.name} <= names
    assert held.read_bytes() == b"#!/bin/sh\n# v1\n"
    ledger = json.loads((store / ".backend-store.json").read_text(encoding="utf-8"))
    assert ledger["schema"] == "molt.backend-generation-store.v1"
    assert ledger["selected"] == [current.parent.name]
    assert idle.parent.name not in ledger["last_used"]


@pytest.mark.parametrize("link_kind", ["symlink", "junction"])
@pytest.mark.parametrize("placement", ["root", "nested"])
def test_backend_generation_eviction_preserves_directory_link_referent(
    tmp_path: Path, link_kind: str, placement: str
) -> None:
    from molt.cli import backend_generation

    if link_kind == "junction" and os.name != "nt":
        pytest.skip("junctions are Windows reparse points")
    root = tmp_path / "generations"
    root.mkdir()
    outside = tmp_path / "outside"
    outside.mkdir()
    sentinel = outside / "sentinel"
    sentinel.write_bytes(b"outside bytes must survive backend eviction")
    before = sentinel.stat()
    entry = root / ("a" * 64)
    if placement == "root":
        alias = entry
    else:
        (entry / "nested").mkdir(parents=True)
        (entry / "compiler").write_bytes(b"owned compiler")
        alias = entry / "nested" / "borrowed"
    if link_kind == "junction":
        import _winapi

        _winapi.CreateJunction(str(outside), str(alias))
    else:
        try:
            alias.symlink_to(outside, target_is_directory=True)
        except OSError:
            pytest.skip("host does not permit symbolic links")
    store = generation_store.GenerationStore(
        layout=backend_generation._BACKEND_STORE,
        root=root,
        now=2 * generation_store.GENERATION_STORE_IDLE_SECONDS,
        last_used={entry.name: 0.0},
    )

    evicted = store.evict_idle(generation_store.GENERATION_STORE_IDLE_SECONDS)

    assert sentinel.read_bytes() == b"outside bytes must survive backend eviction"
    after = sentinel.stat()
    assert (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns) == (
        before.st_dev,
        before.st_ino,
        before.st_size,
        before.st_mtime_ns,
    )
    assert evicted == (entry.name,)
    assert not os.path.lexists(entry)
    assert entry.name not in store.last_used


def test_backend_generation_eviction_retains_a_refused_removal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli import backend_generation

    root = tmp_path / "generations"
    entry = root / ("a" * 64)
    entry.mkdir(parents=True)
    compiler = entry / "compiler"
    compiler.write_bytes(b"held compiler")
    store = generation_store.GenerationStore(
        layout=backend_generation._BACKEND_STORE,
        root=root,
        now=2 * generation_store.GENERATION_STORE_IDLE_SECONDS,
        last_used={entry.name: 0.0},
    )
    monkeypatch.setattr(
        backend_generation, "delete_path", lambda path: (False, "held by a peer")
    )

    assert store.evict_idle(generation_store.GENERATION_STORE_IDLE_SECONDS) == ()
    assert compiler.read_bytes() == b"held compiler"
    assert store.last_used == {entry.name: 0.0}
