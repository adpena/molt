from __future__ import annotations

from tests.compiler_identity_helper import compiler_environment_inputs
from molt.process_environment import ProcessEnvironment

from tests.cli.backend_generation_test_support import (
    publish_backend,
    stored_backend_coordinate,
)
from tests.compiler_identity_helper import (
    compiler_build_admission,
    compiler_cargo_stdout,
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
from molt.cli.backend_generation import backend_generation_path
from molt.cli.product_store import ProductStore, ProductStoreError
from molt.backend_executable_names import backend_executable_name
from molt.host_tool_products import HostToolProduct
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


@pytest.fixture(autouse=True)
def _isolated_host_tool_store(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    """Each test owns its Cargo target and a product store it can inspect.

    Coordinates in these tests live in ``tmp_path/target``, so that is the
    target their plans and build control name.
    """
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "target"))
    store = ProductStore(tmp_path / "product-store" / "host-tool")
    monkeypatch.setattr(cli_backend_binary, "host_tool_store", lambda _root: store)
    return store


def _receipt(project_root: Path, coordinate: Path, profile: str = "dev-fast") -> Path:
    """This checkout's receipt: the identity memo, or a Cargo output's receipt."""
    return cli_backend_binary._backend_fingerprint_path(
        project_root, coordinate, profile
    )


def _stored(
    project_root: Path,
    coordinate: Path,
    fingerprint: dict[str, str] | None = None,
    environment=None,
) -> cli_backend_binary.StoredHostTool:
    """The store variant a build of these inputs publishes to and reads."""
    return cli_backend_binary.StoredHostTool(
        stored_backend_coordinate(
            project_root,
            coordinate,
            _fingerprint() if fingerprint is None else fingerprint,
            compiler_environment_inputs() if environment is None else environment,
        )
    )


def _generation(
    project_root: Path,
    coordinate: Path,
    data: bytes,
    fingerprint: dict[str, str] | None = None,
) -> Path:
    """Where the CLI must publish ``data``: its store variant's content address."""
    return backend_generation_path(
        _stored(project_root, coordinate, fingerprint).coordinate,
        hashlib.sha256(data).hexdigest(),
    )


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
        return subprocess.CompletedProcess(
            cmd, 0, compiler_cargo_stdout(tmp_path, output, cmd.environment), ""
        )

    monkeypatch.setattr(cli_backend_binary, "_backend_fingerprint", fingerprint)
    monkeypatch.setattr(cli_backend_binary, "_run_resolved_cargo_plan", build)
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
        assert result.binary.samefile(
            _generation(
                tmp_path,
                selected,
                expected,
                fingerprint(tmp_path, backend_features=features),
            )
        )
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
            project_root=None if state == "unattested" else tmp_path,
            fingerprint=stored,
            cargo_environment=compiler_environment_inputs(),
        )
        if state == "corrupt":
            generation.write_bytes(b"unexpected compiler bytes")

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: fingerprint
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
    assert not result and result.cache_compiler_fingerprint is None
    if state == "corrupt":
        # A published store entry whose bytes changed is corruption: it fails
        # loud before rebuild policy, and nothing rebuilds over it.
        assert result.phase == "product_store"
        assert "is corrupt" in result.message
        return
    if state == "probe-failure":
        # A stored compiler that fails its probe is never rebuilt over: the
        # same inputs build the same compiler (write-once variants).
        assert result.phase == "backend_feature_probe"
        assert "rebuilding the same inputs cannot change it" in result.message
        assert "requested backend feature is absent" in result.message
        return
    assert result.phase == "rebuild-policy"
    assert "MOLT_SKIP_RUNTIME_REBUILD=1" in result.message


def test_a_stored_compiler_that_fails_its_probe_is_never_rebuilt_over(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A published variant is written once, even when a rebuild is allowed.

    Other builds may hold the stored compiler, and the same inputs cannot build
    another one, so a failed probe fails the build: no Cargo run, no second
    generation and no receipt rewrite, even with this checkout's Cargo output
    on disk.
    """
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
    monkeypatch.delenv("MOLT_SKIP_RUNTIME_REBUILD", raising=False)
    features = ("native-backend",)
    selected = cli_backend_execution._backend_bin_path(tmp_path, "dev-fast", features)
    fingerprint = _fingerprint()
    generation = publish_backend(
        selected,
        b"stored compiler bytes",
        project_root=tmp_path,
        fingerprint=fingerprint,
        cargo_environment=compiler_environment_inputs(),
    )
    stored = _stored(tmp_path, selected)
    receipt = stored.receipt.read_bytes()
    cargo_output = selected.with_name(backend_executable_name(os_name=os.name))
    cargo_output.write_bytes(b"#!/bin/sh\nexit 0\n# other bytes\n")
    cargo_output.chmod(0o755)
    cli_runtime_fingerprints._write_runtime_fingerprint(
        _receipt(tmp_path, cargo_output),
        fingerprint,
        artifact=cargo_output,
        cargo_environment=compiler_environment_inputs(),
    )
    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: fingerprint
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_resolved_cargo_plan",
        lambda *_a, **_k: pytest.fail("a stored compiler was rebuilt over"),
    )
    probes: list[list[str]] = []

    def failing_probe(cmd, **_kwargs):
        probes.append(list(cmd))
        return subprocess.CompletedProcess(cmd, 1, b"", b"probe refused")

    monkeypatch.setattr(
        cli_backend_binary, "_run_subprocess_captured_to_tempfiles", failing_probe
    )
    result = _ensure(selected, tmp_path, features)
    assert not result and result.phase == "backend_feature_probe"
    assert "probe refused" in result.message
    assert [Path(probe[0]) for probe in probes] == [generation]
    assert stored.receipt.read_bytes() == receipt
    assert not stored.probe_validation.exists()
    assert generation.read_bytes() == b"stored compiler bytes"
    assert [
        path.name for path in generation.parent.parent.iterdir() if path.is_dir()
    ] == [generation.parent.name]


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
        project_root=tmp_path,
        fingerprint=fingerprint,
        cargo_environment=compiler_environment_inputs(),
    )
    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: fingerprint
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

    def source_paths(_root, _features, *, product):
        assert product is HostToolProduct.BACKEND
        return [source]

    monkeypatch.setattr(cli_backend_binary, "_backend_source_paths", source_paths)
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
        sidecar,
        original,
        artifact=artifact,
        cargo_environment=compiler_environment_inputs(),
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
        environment=ProcessEnvironment(os.environ),
    )
    if change == "content":
        assert not admitted
        current["cargo_environment"] = before["cargo_environment"]
        with pytest.raises(ValueError, match="semantic identity"):
            runtime_fingerprints._refresh_runtime_fingerprint_metadata(sidecar, current)
        assert runtime_fingerprints._read_runtime_fingerprint(sidecar) == before
    else:
        assert admitted
        current = runtime_fingerprints._admitted_runtime_fingerprint(current, before)
        runtime_fingerprints._refresh_runtime_fingerprint_metadata(sidecar, current)
        after = runtime_fingerprints._read_runtime_fingerprint(sidecar)
        assert after is not None
        assert after["inputs_digest"] == current["inputs_digest"]
        assert after["artifact_content_identity"] == before["artifact_content_identity"]
        assert after["cargo_environment"] == before["cargo_environment"]


@pytest.mark.parametrize("stored", [True, False])
@pytest.mark.parametrize(
    "mutation", ["newer", "in-place", "replacement", "unattested", "wrong-source"]
)
def test_ensure_backend_binary_publishes_only_an_admitted_cargo_output(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mutation: str,
    stored: bool,
) -> None:
    """A stored product wins; else only an attested Cargo output is published."""
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
    old_generation = None
    if stored:
        old_generation = publish_backend(
            backend_bin,
            old_bytes,
            project_root=tmp_path,
            fingerprint=fingerprint,
            cargo_environment=compiler_environment_inputs(),
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
            cargo_environment=compiler_environment_inputs(),
        )
    builds = []

    def refused_build(cmd, **_kwargs):
        builds.append(cmd)
        return subprocess.CompletedProcess(cmd, 101, "", "fixture rebuild refused")

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: dict(fingerprint)
    )
    monkeypatch.setattr(cli_backend_binary, "_run_resolved_cargo_plan", refused_build)
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **kwargs: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )

    result = _ensure(backend_bin, tmp_path, ("wasm-backend",))
    if not stored and mutation in {"unattested", "wrong-source"}:
        # Nothing admits these bytes, so the build runs Cargo.
        assert not result and result.phase == "backend_cargo_build"
        assert len(builds) == 1
        return
    assert result, result.message
    assert not builds
    assert result.binary is not None
    expected = old_bytes if stored else new_bytes
    assert result.binary.read_bytes() == expected
    assert result.binary.samefile(_generation(tmp_path, backend_bin, expected))
    if old_generation is not None:
        # The stored product and what a build captured of it did not change.
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
    publish_backend(
        backend_bin,
        b"backend-v1",
        project_root=tmp_path,
        fingerprint=fingerprint,
        cargo_environment=compiler_environment_inputs(),
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
        changed = _stored(tmp_path, backend_bin).probe_validation
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
            project_root=tmp_path,
            fingerprint=fingerprint,
            cargo_environment=compiler_environment_inputs(),
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


def test_corrupted_stored_generation_fails_loud_without_rebuild(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Bytes that no longer hash to their address are corruption, never reuse."""
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
        project_root=tmp_path,
        fingerprint=fingerprint,
        cargo_environment=compiler_environment_inputs(),
    )
    generation.write_bytes(b"backend-XX")
    builds = 0

    def build(cmd, **_kwargs):
        nonlocal builds
        builds += 1
        cargo_output.write_bytes(b"backend-v1")
        cargo_output.chmod(0o755)
        return subprocess.CompletedProcess(
            cmd, 0, compiler_cargo_stdout(tmp_path, cargo_output, cmd.environment), ""
        )

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
    assert not result and result.phase == "product_store"
    assert "is corrupt" in result.message and str(generation.parents[3]) in (
        result.message
    )
    assert builds == 0
    assert generation.read_bytes() == b"backend-XX"


def test_a_store_miss_hashes_this_checkouts_sources_once(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """The identity that keyed the store lookup is what the locked path rechecks.

    A cold checkout hashes its compiler sources to key the store. On a miss,
    the locked path rechecks that identity's metadata instead of hashing the
    sources again; nothing is written before the build.
    """
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
    backend_bin = tmp_path / "target" / "release-fast" / "molt-backend.native_backend"
    fingerprint = _fingerprint()
    memos: list[object] = []

    def computed(*_args, stored_fingerprint=None, **_kwargs):
        memos.append(stored_fingerprint)
        return dict(fingerprint)

    monkeypatch.setattr(cli_backend_binary, "_backend_fingerprint", computed)
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_resolved_cargo_plan",
        lambda cmd, **_kwargs: subprocess.CompletedProcess(cmd, 101, "", "refused"),
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
    assert not result and result.phase == "backend_cargo_build"
    assert memos[0] is None
    assert isinstance(memos[1], dict) and memos[1]["hash"] == fingerprint["hash"]
    assert memos[1]["inputs_digest"] == fingerprint["inputs_digest"]


@pytest.mark.parametrize("cargo_timeout", [None, 7.0])
def test_a_waiter_waits_as_long_as_the_compiler_build_it_waits_for(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, cargo_timeout: float | None
) -> None:
    """The identity lock is bounded by the build's own Cargo timeout, if any."""
    write_compiler_source(tmp_path)
    stub_compiler_admission(monkeypatch)
    monkeypatch.delenv("MOLT_BUILD_LOCK_TIMEOUT", raising=False)
    backend_bin = tmp_path / "target" / "release-fast" / "molt-backend.native_backend"
    original = ProductStore.single_flight
    waits: list[float | None] = []

    def single_flight(self, identity, *, default_timeout_s):
        waits.append(default_timeout_s)
        return original(self, identity, default_timeout_s=default_timeout_s)

    monkeypatch.setattr(ProductStore, "single_flight", single_flight)
    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: _fingerprint()
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_resolved_cargo_plan",
        lambda cmd, **_kwargs: subprocess.CompletedProcess(cmd, 101, "", "refused"),
    )
    result = cli_backend_binary._ensure_backend_binary(
        backend_bin,
        cargo_timeout=cargo_timeout,
        json_output=True,
        cargo_profile="release-fast",
        project_root=tmp_path,
        backend_features=("native-backend",),
        rust_cpu=RustCpuPolicy.BASELINE,
    )
    assert not result and result.phase == "backend_cargo_build"
    assert waits == [cargo_timeout]


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
    assert result.command == (
        "cargo",
        "build",
        "--locked",
        "--message-format=json-render-diagnostics",
        "--package",
        "molt-backend",
        "--bin",
        "molt-backend",
        "--profile",
        "release-fast",
        "--no-default-features",
        "--features",
        "native-backend",
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
        project_root=tmp_path,
        fingerprint=fingerprint,
        cargo_environment=compiler_environment_inputs(),
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
        "_run_resolved_cargo_plan",
        lambda cmd, **_k: subprocess.CompletedProcess(
            cmd, 101, "", "fixture rebuild refused"
        ),
    )
    result = _ensure(backend_bin, tmp_path, ("native-backend",))
    assert probe_calls
    assert not result
    assert not _stored(tmp_path, backend_bin).probe_validation.exists()


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
        project_root=tmp_path,
        fingerprint=fingerprint,
        cargo_environment=compiler_environment_inputs(),
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
            cargo_environment=compiler_environment_inputs(),
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
        return subprocess.CompletedProcess(
            cmd, 0, compiler_cargo_stdout(tmp_path, cargo_output, cmd.environment), ""
        )

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
    generation = _generation(tmp_path, backend_bin, b"backend-v3")
    assert Path(probe_commands[0][0]).samefile(generation)
    assert generation.read_bytes() == b"backend-v3"
    assert cli_backend_binary._runtime_artifact_fingerprint_matches(
        cargo_output,
        fingerprint,
        cargo_receipt,
        require_artifact_digest=True,
        environment=ProcessEnvironment(os.environ),
    ) == (probe_outcome == "pass")
    # Only a probed compiler acquires the store receipt that selects it.
    selection = cli_backend_binary.stored_host_tools(
        cli_backend_binary.host_tool_store(tmp_path),
        cli_backend_binary.host_tool_identity(backend_bin, fingerprint),
        backend_bin,
        fingerprint,
        environment=ProcessEnvironment(os.environ),
    )
    assert bool(selection) == (probe_outcome == "pass")
    if selection:
        assert selection[0].generation.samefile(generation)


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
    selection_receipt = _stored(tmp_path, backend_bin).receipt
    generation = _generation(tmp_path, backend_bin, b"backend-v1")
    if admitted_source:
        cli_runtime_fingerprints._write_runtime_fingerprint(
            _receipt(tmp_path, cargo_output),
            fingerprint,
            artifact=cargo_output,
            cargo_environment=compiler_environment_inputs(),
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
        "_run_resolved_cargo_plan",
        lambda cmd, **_k: subprocess.CompletedProcess(
            cmd, 0, compiler_cargo_stdout(tmp_path, cargo_output, cmd.environment), ""
        ),
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
    # The receipt names the bytes it saw, which were then replaced: the next
    # reader finds a corrupt entry and fails loud, never admitting them.
    with pytest.raises(ProductStoreError, match="differs from its receipt"):
        cli_backend_binary.stored_host_tools(
            cli_backend_binary.host_tool_store(tmp_path),
            cli_backend_binary.host_tool_identity(backend_bin, fingerprint),
            backend_bin,
            fingerprint,
            environment=ProcessEnvironment(os.environ),
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
        return subprocess.CompletedProcess(
            cmd, 0, compiler_cargo_stdout(tmp_path, cargo_output, cmd.environment), ""
        )

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
        return subprocess.CompletedProcess(
            cmd, 0, compiler_cargo_stdout(tmp_path, cargo_output, cmd.environment), ""
        )

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

    ensure.fingerprint = lambda source: {  # type: ignore[attr-defined]
        **_fingerprint(),
        "hash": canonical_json_sha256(source),
    }
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

    # The inputs A was built from reuse its stored generation without a write.
    c_path = ensure("v1")
    assert c_path.samefile(held_path)
    verify_stable_regular_file_identity(held, label="WASM link input")
    assert held_path.samefile(
        _generation(tmp_path, coordinate, a_bytes, ensure.fingerprint("v1"))
    )


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
    for seed in ("v1", "v2", "v3"):
        assert _generation(
            tmp_path,
            coordinate,
            f"#!/bin/sh\n# {seed}\n".encode(),
            ensure.fingerprint(seed),
        ).is_file()


class _Clock:
    def __init__(self, now: float) -> None:
        self.now = now

    def time(self) -> float:
        return self.now


_HOUR = 60 * 60


def test_publication_evicts_stored_backends_no_build_used_for_five_days(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, _isolated_host_tool_store
) -> None:
    """The product store keeps every entry a build used within its window.

    Each source version is its own store entry. A build that reuses an entry
    renews it, and a later publication evicts only entries idle past the
    window (Go trims its build cache the same way).
    """
    from molt.cli import product_store

    clock = _Clock(1_000_000.0)
    monkeypatch.setattr(generation_store, "time", clock)
    coordinate, ensure = _two_generation_fixture(tmp_path, monkeypatch)
    window = product_store.PRODUCT_STORE_IDLE_SECONDS

    held = ensure("v1")
    clock.now += _HOUR
    idle = ensure("v2")
    clock.now += _HOUR
    replaced = ensure("v3")
    clock.now += window - _HOUR
    assert ensure("v1").samefile(held)
    clock.now += 2 * _HOUR
    current = ensure("v4")

    def entry(source: str) -> str:
        return cli_backend_binary.host_tool_identity(
            coordinate, ensure.fingerprint(source)
        )

    store = _isolated_host_tool_store
    names = {path.name for path in store.root.iterdir()}
    assert {entry("v1"), entry("v4")} <= names
    assert not {entry("v2"), entry("v3")} & names
    assert not idle.exists() and not replaced.exists()
    assert held.read_bytes() == b"#!/bin/sh\n# v1\n"
    assert current.read_bytes() == b"#!/bin/sh\n# v4\n"
    ledger = json.loads((store.root / ".product-store.json").read_text("utf-8"))
    assert ledger["schema"] == "molt.product-store.v1"
    assert set(ledger["last_used"]) == {entry("v1"), entry("v4")}


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


@pytest.mark.parametrize("before,after", [("first", "second"), (None, ""), ("", None)])
@pytest.mark.parametrize(
    "reuse",
    [
        "published",
        "selected-probe",
        "cargo-output",
        "other-target",
        "transfer-read",
        "transfer-import",
    ],
)
def test_observed_environment_change_refuses_every_backend_reuse(
    tmp_path, monkeypatch, before, after, reuse
):
    from tests.compiler_identity_helper import BACKEND_FIXTURE_INPUT

    def select(value):
        if value is None:
            monkeypatch.delenv(BACKEND_FIXTURE_INPUT, raising=False)
        else:
            monkeypatch.setenv(BACKEND_FIXTURE_INPUT, value)

    select(before)
    coordinate, ensure = _two_generation_fixture(tmp_path, monkeypatch)
    generation = ensure("v1")
    stored = _stored(tmp_path, coordinate, ensure.fingerprint("v1"))
    receipt = stored.receipt
    original_receipt = receipt.read_bytes()
    observed = cli_runtime_fingerprints._read_runtime_fingerprint(receipt)[
        "cargo_environment"
    ]
    assert observed == compiler_environment_inputs().to_dict()
    select(after)
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_resolved_cargo_plan",
        lambda *_a, **_k: pytest.fail(
            "reuse refusal must not invoke Cargo with no-build policy"
        ),
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda *_a, **_k: pytest.fail(
            "a stale compiler must not reach its capability probe"
        ),
    )
    features = ("wasm-backend",)
    if reuse == "transfer-read":
        with pytest.raises(ValueError, match="no backend compiler is stored"):
            cli_backend_binary.admitted_backend_binary(
                tmp_path,
                binary=coordinate,
                cargo_profile="release",
                backend_features=features,
                rust_cpu=RustCpuPolicy.BASELINE,
            )
    elif reuse == "transfer-import":
        with pytest.raises(ValueError, match=BACKEND_FIXTURE_INPUT):
            cli_backend_binary.import_backend_binary(
                tmp_path,
                binary=coordinate,
                cargo_profile="release",
                backend_features=features,
                rust_cpu=RustCpuPolicy.BASELINE,
                executable=generation,
                receipt=receipt,
            )
    else:
        target = coordinate
        if reuse == "selected-probe":
            stored.probe_validation.unlink()
        elif reuse == "cargo-output":
            # Without the store's selection only this checkout's Cargo output
            # could still admit the bytes; its observation is stale too.
            receipt.unlink()
        elif reuse == "other-target":
            # Another checkout's target reads the same store by identity.
            target = tmp_path / "other-target" / "release" / coordinate.name
        monkeypatch.setenv("MOLT_SKIP_RUNTIME_REBUILD", "1")
        result = _ensure(target, tmp_path, features, profile="release")
        assert not result and result.phase == "rebuild-policy"
    assert generation.read_bytes() == b"#!/bin/sh\n# v1\n"
    if receipt.exists():
        assert receipt.read_bytes() == original_receipt


def test_backend_observation_rebuild_keeps_identical_byte_cache_key_and_warm_receipts(
    tmp_path, monkeypatch
):
    from tests.compiler_identity_helper import BACKEND_FIXTURE_INPUT

    monkeypatch.setenv(BACKEND_FIXTURE_INPUT, "first")
    coordinate, _ensure_generation = _two_generation_fixture(tmp_path, monkeypatch)
    original_cargo = cli_backend_binary._run_resolved_cargo_plan
    builds = []

    def cargo(plan, **kwargs):
        builds.append(plan.environment[BACKEND_FIXTURE_INPUT])
        return original_cargo(plan, **kwargs)

    monkeypatch.setattr(cli_backend_binary, "_run_resolved_cargo_plan", cargo)
    features = ("wasm-backend",)
    fingerprint = _ensure_generation.fingerprint("v1")
    first = _ensure(coordinate, tmp_path, features, profile="release")
    assert first, first.message
    assert first.binary is not None
    stored = _stored(tmp_path, coordinate, fingerprint)
    cargo_output = coordinate.with_name(backend_executable_name(os_name=os.name))
    paths = (
        stored.receipt,
        _receipt(tmp_path, cargo_output, "release"),
        stored.probe_validation,
        first.binary,
    )
    before = [(path.read_bytes(), path.stat().st_mtime_ns) for path in paths]
    for _ in range(2):
        warm = _ensure(coordinate, tmp_path, features, profile="release")
        assert warm, warm.message
        assert warm.binary == first.binary
        assert warm.cache_compiler_fingerprint == first.cache_compiler_fingerprint
    assert builds == ["first"]
    assert [(path.read_bytes(), path.stat().st_mtime_ns) for path in paths] == before

    monkeypatch.setenv(BACKEND_FIXTURE_INPUT, "second")
    rebuilt = _ensure(coordinate, tmp_path, features, profile="release")
    assert rebuilt, rebuilt.message
    assert builds == ["first", "second"]
    # Another observation is another store variant of the same identity: the
    # same bytes at another path, so the compiler cache key is unchanged.
    assert rebuilt.binary is not None and rebuilt.binary != first.binary
    assert rebuilt.binary.read_bytes() == first.binary.read_bytes()
    assert rebuilt.cache_compiler_fingerprint == first.cache_compiler_fingerprint
    second = _stored(tmp_path, coordinate, fingerprint)
    assert second.receipt != stored.receipt
    current = cli_runtime_fingerprints._read_runtime_fingerprint(second.receipt)
    cargo_current = cli_runtime_fingerprints._read_runtime_fingerprint(paths[1])
    assert (
        current["cargo_environment"]
        != (
            cli_runtime_fingerprints._read_runtime_fingerprint(stored.receipt)[
                "cargo_environment"
            ]
        )
    )
    assert current["cargo_environment"] == cargo_current["cargo_environment"]
    assert (
        current["artifact_content_identity"]
        == cargo_current["artifact_content_identity"]
    )
    assert (
        current["artifact_content_identity"]["sha256"]
        == hashlib.sha256(first.binary.read_bytes()).hexdigest()
    )


def test_a_store_receipt_whose_observation_changed_is_corrupt(tmp_path, monkeypatch):
    """A receipt that no longer names its variant's observation fails loud."""
    from tests.compiler_identity_helper import BACKEND_FIXTURE_INPUT

    monkeypatch.setenv(BACKEND_FIXTURE_INPUT, "current")
    coordinate, ensure = _two_generation_fixture(tmp_path, monkeypatch)
    generation = ensure("v1")
    stored = _stored(tmp_path, coordinate, ensure.fingerprint("v1"))
    source = cli_runtime_fingerprints._read_runtime_fingerprint(stored.receipt)
    replacement = compiler_environment_inputs(
        ProcessEnvironment(os.environ).updated({BACKEND_FIXTURE_INPUT: "other-build"})
    )
    cli_runtime_fingerprints._write_runtime_fingerprint(
        stored.receipt, source, cargo_environment=replacement
    )
    monkeypatch.setenv("MOLT_SKIP_RUNTIME_REBUILD", "1")
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_resolved_cargo_plan",
        lambda *_a, **_k: pytest.fail("a corrupt entry must not invoke Cargo"),
    )
    result = _ensure(coordinate, tmp_path, ("wasm-backend",), profile="release")
    assert not result and result.phase == "product_store"
    assert "Cargo environment differs from its name" in result.message
    assert generation.read_bytes() == b"#!/bin/sh\n# v1\n"


@pytest.mark.parametrize("product", [HostToolProduct.BACKEND, HostToolProduct.JIT])
@pytest.mark.parametrize("read", ["declared", "undeclared"])
def test_a_build_that_reads_outside_its_source_closure_is_never_published(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    product: HostToolProduct,
    read: str,
) -> None:
    # The generation is keyed by the product's source closure. A file the
    # build read outside it (here an include_str! of README.md) would let a
    # later run adopt this build after that file changed (HF-371 review v3).
    # Declared inputs (package.metadata.molt.build-inputs) are covered.
    write_compiler_source(tmp_path, product=product)
    stub_compiler_admission(monkeypatch)
    suffix = ".exe" if os.name == "nt" else ""
    cargo_output = tmp_path / "target" / "dev-fast" / f"{product.value}{suffix}"
    coordinate = cargo_output.with_name(f"{product.value}.variant{suffix}")
    cargo_output.parent.mkdir(parents=True)
    readme = tmp_path / "README.md"
    readme.write_text("not a build input\n", encoding="utf-8")
    declared = tmp_path / "src" / "molt" / "declared_input.txt"
    declared.parent.mkdir(parents=True, exist_ok=True)
    declared.write_text("1\n", encoding="utf-8")
    manifest = tmp_path / "runtime" / product.value / "Cargo.toml"
    manifest.write_text(
        f'[package]\nname = "{product.value}"\nversion = "0.1.0"\n'
        "[package.metadata.molt]\n"
        'build-inputs = ["../../src/molt/declared_input.txt"]\n',
        encoding="utf-8",
    )
    extra = declared if read == "declared" else readme
    probes: list[tuple[str, ...]] = []

    def fake_cargo(cmd, **_kwargs):
        cargo_output.write_bytes(b"built")
        return subprocess.CompletedProcess(
            cmd,
            0,
            compiler_cargo_stdout(
                tmp_path,
                cargo_output,
                cmd.environment,
                product=product,
                extra_reads=(extra,),
            ),
            "",
        )

    def probe(cmd, **_kwargs):
        probes.append(tuple(cmd))
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", lambda *_a, **_k: _fingerprint()
    )
    monkeypatch.setattr(cli_backend_binary, "_run_resolved_cargo_plan", fake_cargo)
    monkeypatch.setattr(
        cli_backend_binary,
        "_hydrate_canonical_backend_generation",
        lambda *_a, **_k: None,
    )
    monkeypatch.setattr(
        cli_backend_binary, "_run_subprocess_captured_to_tempfiles", probe
    )

    result = _ensure(coordinate, tmp_path, product.required_features, product=product)

    generation = _generation(coordinate, b"built")
    if read == "declared":
        assert result, result.message
        assert generation.read_bytes() == b"built"
        if product is HostToolProduct.JIT:
            # The admission probe is the runner's declared probe command
            # (runtime/native_launch_abi.toml [runner]), nothing spelled here.
            import tomllib

            declared_runner = tomllib.loads(
                (
                    Path(__file__).resolve().parents[2]
                    / "runtime"
                    / "native_launch_abi.toml"
                ).read_text(encoding="utf-8")
            )["runner"]
            assert probes and probes[-1][1:] == (declared_runner["probe_flag"],)
        return
    assert not result
    assert result.phase == "backend_build_input_coverage"
    assert "README.md" in result.message
    assert "build-inputs" in result.message
    assert not probes
    assert not generation.exists()
