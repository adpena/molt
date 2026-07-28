from __future__ import annotations

import os
import json
import shutil
import sys
import threading
import time
from pathlib import Path

import pytest

from molt.cli import runtime_build_identity as identity
from molt.cli.runtime_artifact_selection import (
    RUNTIME_STATICLIB_ARTIFACTS,
    RUNTIME_WASM_COMBINED_ARTIFACTS,
    RuntimeArtifactSelection,
)


def _provision_toolchain(root: Path) -> identity.RuntimeToolchainContentManifest:
    archive_root = root / "archives"
    env = dict(os.environ)
    env.update(
        {
            "RUSTC": sys.executable,
            "CARGO": sys.executable,
            "MOLT_BUILD_PYTHON": sys.executable,
            "CC_wasm32-wasip1": sys.executable,
            "CXX_wasm32-wasip1": sys.executable,
            "AR_wasm32-wasip1": sys.executable,
        }
    )
    return identity.provision_runtime_toolchain_content_manifest(
        env=env,
        target_triple="wasm32-wasip1",
        wasi_sysroot=root / "wasi-sysroot",
        wasm_linker=Path(sys.executable),
        long_double_archive=archive_root / "libc-printscan-long-double.a",
        builtins_archive=archive_root / "libclang_rt.builtins-wasm32.a",
        wasi_libc_archive=archive_root / "libc.a",
        rust_builtins_archive=archive_root / "libcompiler_builtins.rlib",
    )


def _resolve(
    root: Path,
    *,
    kind: str,
    publication: str,
    profile: str = "release-output",
    base_rustflags: str = "-C panic=abort",
    response_path: Path | None = None,
    toolchain_manifest: identity.RuntimeToolchainContentManifest | None = None,
    extra_env: dict[str, str] | None = None,
    artifact_selection: RuntimeArtifactSelection = RUNTIME_WASM_COMBINED_ARTIFACTS,
) -> identity.RuntimeBuildIdentity:
    archive_root = root / "archives"
    sysroot = root / "wasi-sysroot"
    archives = [
        archive_root / "libc.a",
        archive_root / "libcompiler_builtins.rlib",
        archive_root / "libc-printscan-long-double.a",
        archive_root / "libclang_rt.builtins-wasm32.a",
    ]
    env = dict(os.environ)
    env.update(
        {
            "RUSTC": sys.executable,
            "CARGO": sys.executable,
            "MOLT_BUILD_PYTHON": sys.executable,
            "CC_wasm32-wasip1": sys.executable,
            "CXX_wasm32-wasip1": sys.executable,
            "AR_wasm32-wasip1": sys.executable,
        }
    )
    env.update(extra_env or {})
    shared, reloc = identity.resolve_runtime_build_pair_identities(
        root,
        env=env,
        cargo_profile=profile,
        target_triple="wasm32-wasip1",
        runtime_features=("stdlib_micro",),
        base_rustflags=base_rustflags,
        producer_artifact_selection=artifact_selection,
        shared=identity.RuntimePairMemberPlan(
            kind="shared",
            resolved_rustflags=(
                "-C panic=abort --cfg shared"
                + (f" -C link-arg=@{response_path}" if response_path else "")
            ),
            publication_transform=(
                publication if kind == "shared" else "strip-final-link-metadata-v1"
            ),
            preserve_debug=False,
            link_args=("--export=shared",),
        ),
        reloc=identity.RuntimePairMemberPlan(
            kind="reloc",
            resolved_rustflags="-C panic=abort --cfg reloc",
            publication_transform=(
                publication if kind == "reloc" else "relocatable-wasm-byte-identity-v1"
            ),
            preserve_debug=False,
            link_args=("--export=reloc",),
        ),
        wasi_sysroot=sysroot,
        wasm_linker=Path(sys.executable),
        long_double_archive=archives[2],
        builtins_archive=archives[3],
        wasi_libc_archive=archives[0],
        rust_builtins_archive=archives[1],
        toolchain_manifest=toolchain_manifest,
    )
    return shared if kind == "shared" else reloc


@pytest.fixture
def identity_root(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    source = tmp_path / "runtime" / "src"
    source.mkdir(parents=True)
    (source / "lib.rs").write_text("pub fn runtime() {}\n", encoding="utf-8")
    sysroot = tmp_path / "wasi-sysroot"
    (sysroot / "include").mkdir(parents=True)
    (sysroot / "include" / "stddef.h").write_text("typedef int size_t;\n")
    (sysroot / "lib" / "wasm32-wasip1").mkdir(parents=True)
    (sysroot / "lib" / "wasm32-wasip1" / "libwasi-emulated-signal.a").write_bytes(
        b"signal"
    )
    (sysroot / "VERSION").write_text("33\n", encoding="utf-8")
    archive_root = tmp_path / "archives"
    archive_root.mkdir()
    for name in (
        "libc.a",
        "libcompiler_builtins.rlib",
        "libc-printscan-long-double.a",
        "libclang_rt.builtins-wasm32.a",
    ):
        (archive_root / name).write_bytes(name.encode("ascii"))
    monkeypatch.setattr(
        identity,
        "runtime_source_paths",
        lambda root, _features=(): (root / "runtime" / "src",),
        raising=True,
    )
    return tmp_path


def test_identity_roundtrips_and_shared_reloc_form_one_pair(
    identity_root: Path,
) -> None:
    shared = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    )
    reloc = _resolve(
        identity_root,
        kind="reloc",
        publication="relocatable-wasm-byte-identity-v1",
    )

    assert shared.digest != reloc.digest
    assert shared.pair_digest == reloc.pair_digest
    assert identity.RuntimeBuildIdentity.from_dict(shared.to_dict()) == shared


def test_identity_observes_source_and_sysroot_mutation_without_cache(
    identity_root: Path,
) -> None:
    before = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    )
    (identity_root / "runtime" / "src" / "lib.rs").write_text(
        "pub fn runtime_changed() {}\n", encoding="utf-8"
    )
    after_source = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    )
    assert after_source.digest != before.digest
    assert after_source.pair_digest != before.pair_digest

    (identity_root / "wasi-sysroot" / "include" / "stddef.h").write_text(
        "typedef unsigned size_t;\n", encoding="utf-8"
    )
    after_header = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    )
    assert after_header.digest != after_source.digest
    assert after_header.pair_digest != after_source.pair_digest


def test_canonical_profile_and_publication_transform_are_identity_inputs(
    identity_root: Path,
) -> None:
    release = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    )
    dev = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        profile="dev-fast",
    )
    unstripped = _resolve(
        identity_root,
        kind="shared",
        publication="unstripped-debug-v1",
    )

    assert len({release.digest, dev.digest, unstripped.digest}) == 3
    assert release.pair_digest != dev.pair_digest
    assert release.pair_digest != unstripped.pair_digest


def test_exact_producer_artifact_selection_is_pair_identity_input(
    identity_root: Path,
) -> None:
    combined = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    )
    staticlib_only = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        artifact_selection=RUNTIME_STATICLIB_ARTIFACTS,
    )

    assert combined.pair_digest != staticlib_only.pair_digest
    assert (
        combined.payload["pair"]["common_config"]["producer_artifact_selection"]
        == RUNTIME_WASM_COMBINED_ARTIFACTS.source_identity
    )


def test_deserializer_rejects_self_asserted_digest(identity_root: Path) -> None:
    value = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    ).to_dict()
    value["digest"] = "0" * 64
    with pytest.raises(ValueError, match="digest"):
        identity.RuntimeBuildIdentity.from_dict(value)


def test_identity_rejects_digest_valid_wrong_pair_schema(identity_root: Path) -> None:
    value = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    ).to_dict()
    pair = value["payload"]["pair"]
    pair["schema"] = "molt.runtime-build-pair.v1"
    value["pair_digest"] = identity._digest(pair)
    value["digest"] = identity._digest(value["payload"])

    with pytest.raises(ValueError, match="digest"):
        identity.RuntimeBuildIdentity.from_dict(value)


def test_ambient_c_and_cxx_flags_are_pair_identity_inputs(
    identity_root: Path,
) -> None:
    baseline = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        extra_env={"CFLAGS_wasm32-wasip1": "-O1", "CXXFLAGS": "-fno-rtti"},
    )
    changed = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        extra_env={"CFLAGS_wasm32-wasip1": "-O2", "CXXFLAGS": "-fno-rtti"},
    )

    assert baseline.pair_digest != changed.pair_digest
    assert baseline.payload["pair"]["common_config"]["ambient_c_build_environment"] == {
        "CFLAGS_wasm32-wasip1": ("-O1",),
        "CXXFLAGS": ("-fno-rtti",),
    }


def test_identity_serialization_is_detached_and_rejects_nested_mutation(
    identity_root: Path,
) -> None:
    resolved = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
    )
    detached = resolved.to_dict()
    detached["payload"]["pair"]["common_config"]["cargo_profile"] = "poison"
    assert (
        resolved.to_dict()["payload"]["pair"]["common_config"]["cargo_profile"]
        == "release-output"
    )

    with pytest.raises(TypeError):
        resolved.payload["pair"]["common_config"]["cargo_profile"] = "poison"


def test_identity_is_location_independent_including_response_files(
    identity_root: Path, tmp_path: Path
) -> None:
    relocated = tmp_path / "relocated" / "tree"
    shutil.copytree(identity_root, relocated)
    first_response = identity_root / "runtime-link.rsp"
    second_response = relocated / "runtime-link.rsp"
    first_response.write_text("--export=PyLong_Type\n", encoding="utf-8")
    second_response.write_text("--export=PyLong_Type\n", encoding="utf-8")

    first = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        response_path=first_response,
        base_rustflags=f"-C panic=abort --sysroot={identity_root / 'wasi-sysroot'}",
    )
    second = _resolve(
        relocated,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        response_path=second_response,
        base_rustflags=f"-C panic=abort --sysroot={relocated / 'wasi-sysroot'}",
    )
    serialized = json.dumps(first.to_dict(), sort_keys=True)

    assert first == second
    assert str(identity_root) not in serialized
    assert str(relocated) not in serialized
    assert r"C:\\" not in serialized
    assert "D:/" not in serialized


def test_identity_rejects_unknown_absolute_flag_path(identity_root: Path) -> None:
    with pytest.raises(ValueError, match="absolute host path"):
        _resolve(
            identity_root,
            kind="shared",
            publication="strip-final-link-metadata-v1",
            base_rustflags=r"-L D:\poison\runtime",
        )


def test_flag_canonicalization_enforces_path_boundaries(identity_root: Path) -> None:
    sysroot = identity_root / "wasi-sysroot"
    logical = (("wasi-sysroot", sysroot),)
    assert (
        identity._canonical_flag_token(
            f"-I{sysroot / 'include'}", logical_paths=logical
        )
        == "-I${wasi-sysroot}/include"
    )
    with pytest.raises(ValueError, match="absolute host path"):
        identity._canonical_flag_token(f"--sysroot={sysroot}bar", logical_paths=logical)
    with pytest.raises(ValueError, match="absolute host path"):
        identity._canonical_flag_token(f"embedded{sysroot}", logical_paths=logical)
    with pytest.raises(ValueError, match="absolute host path"):
        identity._canonical_flag_token("-I/opt/poison", logical_paths=logical)


def test_tool_version_banner_never_serializes_installed_directory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    tool = tmp_path / "clang.exe"
    tool.write_bytes(b"tool-bytes")
    monkeypatch.setattr(identity, "_command_path", lambda *_args, **_kwargs: tool)
    monkeypatch.setattr(
        identity.process_guard,
        "run_completed_command",
        lambda *_args, **_kwargs: type(
            "Completed",
            (),
            {
                "returncode": 0,
                "stdout": "clang version 22.1.7\nInstalledDir: D:\\poison\\LLVM\\bin\n",
                "stderr": "",
            },
        )(),
    )

    result = identity._executable_identity("cc", str(tool), env={})

    assert result["version"] == "22.1.7"
    assert "poison" not in json.dumps(result)


def test_tree_identity_rejects_logical_label_collision(tmp_path: Path) -> None:
    first = tmp_path / "first"
    second = tmp_path / "second"
    first.write_bytes(b"first")
    second.write_bytes(b"second")
    with pytest.raises(ValueError, match="root label collision"):
        identity._tree_identity(
            (("runtime/input", first), ("runtime/input", second)),
            require_all=True,
        )


def test_tree_identity_is_deterministic_across_parallel_completion_order(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source"
    source.mkdir()
    for index in range(12):
        (source / f"{index:02}.rs").write_bytes(bytes([index]) * (index + 1))
    original = identity._hash_tree_input_file
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 1)
    serial = identity._tree_identity((("runtime/source", source),), require_all=True)
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 4)

    def forward(file: identity._TreeInputFile) -> str:
        time.sleep((11 - int(file.path.stem)) * 0.0005)
        return original(file)

    monkeypatch.setattr(identity, "_hash_tree_input_file", forward)
    first = identity._tree_identity((("runtime/source", source),), require_all=True)

    def reverse(file: identity._TreeInputFile) -> str:
        time.sleep(int(file.path.stem) * 0.0005)
        return original(file)

    monkeypatch.setattr(identity, "_hash_tree_input_file", reverse)
    second = identity._tree_identity((("runtime/source", source),), require_all=True)

    assert serial == first == second
    assert first["file_count"] == 12


def test_tree_identity_parallel_scheduler_bounds_in_flight_futures(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source"
    source.mkdir()
    for index in range(20):
        (source / f"{index:02}.rs").write_text(
            f"pub const VALUE_{index}: usize = {index};\n", encoding="utf-8"
        )
    pending_sizes: list[int] = []
    original_wait = identity.wait

    def observed_wait(futures: object, **kwargs: object) -> object:
        pending_sizes.append(len(futures))  # type: ignore[arg-type]
        return original_wait(futures, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 3)
    monkeypatch.setattr(identity, "wait", observed_wait)

    result = identity._tree_identity((("runtime/source", source),), require_all=True)

    assert result["file_count"] == 20
    assert pending_sizes
    assert max(pending_sizes) == 6


def test_tree_identity_workers_are_cpu_memory_file_and_policy_bounded(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[dict[str, int]] = []

    def resource_ceiling(**kwargs: int) -> int:
        calls.append(kwargs)
        return 7

    monkeypatch.setattr(identity, "_memory_bounded_worker_count", resource_ceiling)

    assert identity._tree_hash_worker_count(3) == 3
    assert identity._tree_hash_worker_count(100) == 7
    assert calls == [
        {
            "bytes_per_worker": identity._TREE_HASH_BYTES_PER_WORKER,
            "headroom_bytes": identity._TREE_HASH_MEMORY_HEADROOM_BYTES,
        },
        {
            "bytes_per_worker": identity._TREE_HASH_BYTES_PER_WORKER,
            "headroom_bytes": identity._TREE_HASH_MEMORY_HEADROOM_BYTES,
        },
    ]
    monkeypatch.setattr(
        identity, "_memory_bounded_worker_count", lambda **_kwargs: 10_000
    )
    assert identity._tree_hash_worker_count(100) == identity._TREE_HASH_MAX_WORKERS


def test_tree_identity_rejects_same_size_mutation_during_hash(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source.bin"
    source.write_bytes(b"a" * (2 * 1024 * 1024))
    before = source.stat()
    original = identity._sha256_open_file

    def mutate_after_read(handle: object) -> str:
        digest = original(handle)
        with source.open("r+b", buffering=0) as writer:
            writer.write(b"b" * before.st_size)
            os.fsync(writer.fileno())
        os.utime(
            source,
            ns=(before.st_atime_ns, before.st_mtime_ns),
        )
        return digest

    monkeypatch.setattr(identity, "_sha256_open_file", mutate_after_read)
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 1)

    with pytest.raises(ValueError, match="changed while hashing"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_tree_identity_rejects_mutation_after_enumeration_before_open(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source.bin"
    source.write_bytes(b"before")
    original = identity._hash_tree_input_file

    def mutate_before_open(file: identity._TreeInputFile) -> str:
        before = file.path.stat()
        file.path.write_bytes(b"after!")
        os.utime(
            file.path,
            ns=(before.st_atime_ns, before.st_mtime_ns + 1_000_000_000),
        )
        return original(file)

    monkeypatch.setattr(identity, "_hash_tree_input_file", mutate_before_open)
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 1)

    with pytest.raises(ValueError, match="changed while hashing"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_tree_identity_rejects_mutation_after_open_before_read(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source.bin"
    source.write_bytes(b"before")
    original = identity._sha256_open_file

    def mutate_after_open(handle: object) -> str:
        before = source.stat()
        source.write_bytes(b"after!")
        os.utime(
            source,
            ns=(before.st_atime_ns, before.st_mtime_ns + 1_000_000_000),
        )
        return original(handle)

    monkeypatch.setattr(identity, "_sha256_open_file", mutate_after_open)
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 1)

    with pytest.raises(ValueError, match="changed while hashing"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_tree_identity_fails_closed_on_hash_io_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source"
    source.mkdir()
    victim = source / "victim.rs"
    victim.write_text("pub fn victim() {}\n", encoding="utf-8")
    original = identity._hash_tree_input_file

    def delete_before_open(file: identity._TreeInputFile) -> str:
        file.path.unlink()
        return original(file)

    monkeypatch.setattr(identity, "_hash_tree_input_file", delete_before_open)
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 1)

    with pytest.raises(OSError, match="runtime input hashing failed.*victim.rs"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_tree_identity_fails_closed_on_read_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "source.bin"
    source.write_bytes(b"content")

    def fail_read(_handle: object) -> str:
        raise OSError("synthetic read fault")

    monkeypatch.setattr(identity, "_sha256_open_file", fail_read)
    monkeypatch.setattr(identity, "_tree_hash_worker_count", lambda _count: 1)

    with pytest.raises(OSError, match="runtime input hashing failed.*read fault"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_tree_identity_rejects_internal_file_alias(tmp_path: Path) -> None:
    source = tmp_path / "source"
    source.mkdir()
    target = source / "target.rs"
    target.write_text("pub fn target() {}\n", encoding="utf-8")
    linked = source / "linked.rs"
    try:
        linked.symlink_to(target)
    except OSError:
        pytest.skip("file symlinks are unavailable")

    with pytest.raises(ValueError, match="path alias"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_tree_identity_rejects_root_alias(tmp_path: Path) -> None:
    source = tmp_path / "source"
    source.mkdir()
    (source / "target.rs").write_text("pub fn target() {}\n", encoding="utf-8")
    linked = tmp_path / "linked"
    try:
        linked.symlink_to(source, target_is_directory=True)
    except OSError:
        pytest.skip("directory symlinks are unavailable")

    with pytest.raises(ValueError, match="root alias"):
        identity._tree_identity((("runtime/source", linked),), require_all=True)


def test_tree_identity_rejects_file_symlink_escape(tmp_path: Path) -> None:
    source = tmp_path / "source"
    source.mkdir()
    outside = tmp_path / "outside.rs"
    outside.write_text("pub fn poison() {}\n", encoding="utf-8")
    linked = source / "linked.rs"
    try:
        linked.symlink_to(outside)
    except OSError:
        pytest.skip("file symlinks are unavailable")

    with pytest.raises(ValueError, match="escaped logical root"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_tree_identity_rejects_directory_symlink_escape(tmp_path: Path) -> None:
    source = tmp_path / "source"
    source.mkdir()
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "poison.rs").write_text("pub fn poison() {}\n", encoding="utf-8")
    linked = source / "linked"
    try:
        linked.symlink_to(outside, target_is_directory=True)
    except OSError:
        pytest.skip("directory symlinks are unavailable")

    with pytest.raises(ValueError, match="escaped logical root"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_tree_identity_rejects_broken_symlink_escape(tmp_path: Path) -> None:
    source = tmp_path / "source"
    source.mkdir()
    linked = source / "broken.rs"
    try:
        linked.symlink_to(tmp_path.parent / "missing" / "poison.rs")
    except OSError:
        pytest.skip("file symlinks are unavailable")

    with pytest.raises(ValueError, match="escaped logical root"):
        identity._tree_identity((("runtime/source", source),), require_all=True)


def test_normal_identity_consumes_only_provisioned_toolchain_manifest(
    identity_root: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manifest = _provision_toolchain(identity_root)
    manifest_path = identity_root / "toolchain.json"
    manifest.write(manifest_path)
    restored = identity.RuntimeToolchainContentManifest.read(manifest_path)
    monkeypatch.setattr(
        identity,
        "_runtime_toolchain_content",
        lambda **_kwargs: (_ for _ in ()).throw(
            AssertionError("normal identity rescanned provisioned toolchain")
        ),
    )

    resolved = _resolve(
        identity_root,
        kind="shared",
        publication="strip-final-link-metadata-v1",
        toolchain_manifest=restored,
    )

    assert resolved.payload["pair"]["toolchain"] == manifest.payload["toolchain"]


def test_exact_toolchain_manifest_observes_changed_archive_byte(
    identity_root: Path,
) -> None:
    before = _provision_toolchain(identity_root)
    archive = identity_root / "archives" / "libc.a"
    original = archive.read_bytes()
    archive.write_bytes(original[:-1] + bytes([original[-1] ^ 1]))

    after = _provision_toolchain(identity_root)

    assert after.digest != before.digest


def test_toolchain_manifest_is_relocatable_and_rejects_tampering(
    identity_root: Path, tmp_path: Path
) -> None:
    relocated = tmp_path / "relocated-toolchain"
    shutil.copytree(identity_root, relocated)
    first = _provision_toolchain(identity_root)
    second = _provision_toolchain(relocated)
    assert first == second

    value = first.to_dict()
    value["payload"]["target_triple"] = "wasm32-poison"
    with pytest.raises(ValueError, match="digest"):
        identity.RuntimeToolchainContentManifest.from_dict(value)


def test_toolchain_manifest_rejects_nested_mutation_at_consumption(
    identity_root: Path,
) -> None:
    manifest = _provision_toolchain(identity_root)
    with pytest.raises(TypeError):
        manifest.payload["target_triple"] = "wasm32-poison"


def test_toolchain_manifest_concurrent_publication_is_atomic(
    identity_root: Path, tmp_path: Path
) -> None:
    first = _provision_toolchain(identity_root)
    archive = identity_root / "archives" / "libc.a"
    archive.write_bytes(b"different-libc")
    second = _provision_toolchain(identity_root)
    path = tmp_path / "runtime-toolchain.json"
    barrier = threading.Barrier(2)
    errors: list[BaseException] = []

    def publish(manifest: identity.RuntimeToolchainContentManifest) -> None:
        try:
            barrier.wait()
            for _ in range(32):
                manifest.write(path)
        except BaseException as exc:
            errors.append(exc)

    threads = [
        threading.Thread(target=publish, args=(manifest,))
        for manifest in (first, second)
    ]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()

    assert errors == []
    final = identity.RuntimeToolchainContentManifest.read(path)
    assert final.digest in {first.digest, second.digest}
