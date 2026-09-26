from __future__ import annotations

import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import stat
import tarfile

import pytest

from molt.cli.native_link_manifest import (
    native_link_flags_from_manifest,
    read_native_link_dependency_manifest,
    write_native_link_dependency_manifest,
)
from molt.cli.runtime_build_identity import RuntimeBuildIdentity
from tests.cli.native_link_test_support import (
    RUNTIME_BUILD_IDENTITY,
    write_test_native_link_manifest,
    write_test_static_archive,
)
from tests.runtime_build_identity_helper import native_runtime_staticlib_identity
from tools import nightly_runtime_bundle as bundle
from molt.exact_json import encode_exact
from molt import artifact_publication


IDENTITY = bundle.BundleIdentity(
    source_commit="1" * 40,
    target_triple="x86_64-unknown-linux-gnu",
)


def test_extraction_destination_rejects_junction_components(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    destination = tmp_path / "destination"
    redirected = destination / "redirected"
    redirected.mkdir(parents=True)
    original = Path.is_junction
    monkeypatch.setattr(
        Path,
        "is_junction",
        lambda path: path == redirected or original(path),
    )

    with pytest.raises(bundle.NightlyRuntimeBundleError, match="link-like"):
        bundle._ensure_destination_has_no_link(
            destination,
            PurePosixPath("redirected/runtime.a"),
        )


def _built_target(
    tmp_path: Path,
    *,
    identity: bundle.BundleIdentity = IDENTITY,
    build_identity: RuntimeBuildIdentity = RUNTIME_BUILD_IDENTITY,
) -> Path:
    target_root = tmp_path / "target"
    profile_root = target_root / bundle.PROFILE
    profile_root.mkdir(parents=True)
    runtime = profile_root / bundle._runtime_archive_name(identity)
    write_test_static_archive(runtime, b"runtime object bytes")
    write_test_native_link_manifest(runtime, build_identity=build_identity)
    backend = profile_root / bundle._backend_executable_name(identity)
    backend.write_bytes(b"\x7fELF\x02\x01molt backend")
    backend.chmod(0o755)
    return target_root


def _pack(tmp_path: Path) -> tuple[Path, Path, dict[str, object]]:
    archive = tmp_path / "nightly-runtime.tar"
    manifest = tmp_path / "nightly-runtime.json"
    payload = bundle.pack_bundle(
        runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        target_root=_built_target(tmp_path),
        output=archive,
        manifest_output=manifest,
        identity=IDENTITY,
    )
    return archive, manifest, payload


def _pack_custody(tmp_path: Path) -> tuple[Path, dict[str, object]]:
    target_root = _built_target(tmp_path)
    runtime = target_root / bundle.PROFILE / bundle._runtime_archive_name(IDENTITY)
    producer = tmp_path / "producer"
    out_dir = producer / "out"
    library_dir = producer / "lib"
    out_dir.mkdir(parents=True)
    library_dir.mkdir()
    (library_dir / "libportable.a").write_bytes(b"portable dependency")
    cargo_stdout = json.dumps(
        {
            "reason": "build-script-executed",
            "package_id": "registry+https://example.invalid#portable-sys@1.0.0",
            "linked_libs": ["static=portable"],
            "linked_paths": [f"native={library_dir}"],
            "cfgs": [],
            "env": [],
            "out_dir": str(out_dir),
        }
    )
    write_native_link_dependency_manifest(
        cargo_stdout,
        cargo_stderr="note: native-static-libs: -lportable\n",
        runtime_lib=runtime,
        cargo_profile=bundle.PROFILE,
        target_triple=None,
        runtime_build_identity=RUNTIME_BUILD_IDENTITY,
    )
    shutil.rmtree(producer)

    archive = tmp_path / "nightly-runtime.tar"
    bundle_manifest = bundle.pack_bundle(
        runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        target_root=target_root,
        output=archive,
        manifest_output=tmp_path / "nightly-runtime.json",
        identity=IDENTITY,
    )
    return archive, bundle_manifest


def test_bundle_carries_portable_native_dependency_custody(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive, bundle_manifest = _pack_custody(tmp_path)
    assert [record["role"] for record in bundle_manifest["files"]] == [
        bundle.RUNTIME_ROLE,
        bundle.LINK_ROLE,
        bundle.CUSTODY_ROLE,
        bundle.BACKEND_ROLE,
    ]

    destination = tmp_path / "hydrated"
    publish = bundle.publish_validated_outputs
    published: list[str] = []

    def observe(pairs: list[tuple[Path, Path]]) -> tuple[Path, ...]:
        published.extend(final.name for _stage, final in pairs)
        return publish(pairs)

    monkeypatch.setattr(bundle, "publish_validated_outputs", observe)
    bundle.verify_extract_bundle(
        archive=archive,
        destination=destination,
        expected_identity=IDENTITY,
        expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
    )
    assert published[0].startswith("molt-native-link-custody-")
    assert published[-2].endswith(".native-link-deps.json")
    assert published[-1] == bundle.MANIFEST_NAME
    hydrated_runtime = (
        destination / bundle.PROFILE / bundle._runtime_archive_name(IDENTITY)
    )
    manifest = read_native_link_dependency_manifest(
        hydrated_runtime,
        target_triple=None,
        cargo_profile=bundle.PROFILE,
        runtime_build_identity=RUNTIME_BUILD_IDENTITY,
    )
    flags = native_link_flags_from_manifest(
        manifest,
        object_format="elf",
        runtime_lib=hydrated_runtime,
    )
    assert flags[-1] == "-lportable"
    custody_directory = Path(flags[-2][2:])
    assert (custody_directory / "libportable.a").read_bytes() == (
        b"portable dependency"
    )


@pytest.mark.parametrize(
    "mutation", ["custody_name", "custody_schema", "native_schema_float"]
)
def test_bundle_rejects_resealed_custody_protocol_before_publication(
    tmp_path: Path, mutation: str
) -> None:
    archive, manifest = _pack_custody(tmp_path)
    records = manifest["files"]
    payloads = _tar_payloads(archive)
    by_name = {info.name: (info, data) for info, data in payloads}
    if mutation == "custody_name":
        record = next(
            record for record in records if record["role"] == bundle.CUSTODY_ROLE
        )
        info, data = by_name.pop(record["path"])
        record["path"] = f"dev-fast/molt-native-link-custody-{'0' * 64}.tar"
        info.name = record["path"]
        by_name[info.name] = (info, data)
    else:
        record = next(
            record for record in records if record["role"] == bundle.LINK_ROLE
        )
        info, data = by_name[record["path"]]
        native = json.loads(data)
        if mutation == "custody_schema":
            native["custody"]["schema"] = "molt.native-link-custody.v1"
        else:
            native["schema_version"] = 5.0
        data = encode_exact(native)
        record["size_bytes"] = len(data)
        record["sha256"] = hashlib.sha256(data).hexdigest()
        by_name[info.name] = (info, data)
    info, _data = by_name[bundle.MANIFEST_NAME]
    by_name[info.name] = (info, encode_exact(manifest))
    malformed = tmp_path / "resealed.tar"
    _write_tar(malformed, list(by_name.values()))
    destination = tmp_path / "hydrated"
    with pytest.raises(
        bundle.NightlyRuntimeBundleError, match="native link metadata is invalid"
    ):
        bundle.verify_extract_bundle(
            archive=malformed,
            destination=destination,
            expected_identity=IDENTITY,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )
    assert not (destination / bundle.PROFILE).exists()


@pytest.mark.parametrize("value", [1.0, True])
def test_bundle_rejects_resealed_runtime_receipt_numeric_coercion(
    tmp_path: Path, value: object
) -> None:
    archive, _manifest_path, manifest = _pack(tmp_path)
    manifest["files"][0]["artifact_identity"]["member_count"] = value
    payloads = [
        (info, encode_exact(manifest) if info.name == bundle.MANIFEST_NAME else data)
        for info, data in _tar_payloads(archive)
    ]
    malformed = tmp_path / "resealed-runtime.tar"
    _write_tar(malformed, payloads)
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="nonnegative integer"):
        bundle.verify_extract_bundle(
            archive=malformed,
            destination=tmp_path / "hydrated",
            expected_identity=IDENTITY,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )
    assert not (tmp_path / "hydrated").exists()


def test_bundle_member_count_rejects_before_unbounded_retention(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    members = [
        _regular_info(
            f"{bundle.PROFILE}/molt-native-link-custody-{index:064x}.tar", b"x"
        )
        for index in range(8)
    ]
    archive = tmp_path / "too-many.tar"
    _write_tar(archive, [(member, b"x") for member in members])
    visits: list[str] = []
    validate_name = bundle._validated_member_name

    def visit(name: str) -> str:
        visits.append(name)
        return validate_name(name)

    monkeypatch.setattr(bundle, "_validated_member_name", visit)
    with pytest.raises(
        bundle.NightlyRuntimeBundleError, match="member closure mismatch"
    ):
        bundle.verify_extract_bundle(
            archive=archive,
            destination=tmp_path / "hydrated",
            expected_identity=IDENTITY,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )
    assert len(visits) == len(bundle._ROLES_WITH_CUSTODY) + 2


@pytest.mark.parametrize(
    "role",
    [
        bundle.RUNTIME_ROLE,
        bundle.LINK_ROLE,
        bundle.BACKEND_ROLE,
        bundle.CUSTODY_ROLE,
        "manifest",
    ],
)
def test_bundle_rejects_staged_byte_mutation_after_semantic_validation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, role: str
) -> None:
    archive, manifest = _pack_custody(tmp_path)
    destination = tmp_path / "hydrated"
    original_outputs: dict[Path, bytes] = {}
    for record in manifest["files"]:
        final = destination / record["path"]
        final.parent.mkdir(parents=True, exist_ok=True)
        original_outputs[final] = b"previous artifact"
        final.write_bytes(original_outputs[final])
    final_manifest = destination / bundle.MANIFEST_NAME
    original_outputs[final_manifest] = b"previous manifest"
    final_manifest.write_bytes(original_outputs[final_manifest])
    selected_final = (
        final_manifest
        if role == "manifest"
        else next(
            destination / record["path"]
            for record in manifest["files"]
            if record["role"] == role
        )
    )
    stages: dict[Path, Path] = {}
    stage_path = bundle.staged_output_path

    def record_stage(final: Path, **kwargs) -> Path:
        result = stage_path(final, **kwargs)
        stages[final] = result
        return result

    def mutate() -> None:
        path = stages[selected_final]
        metadata = path.stat()
        data = path.read_bytes()
        path.write_bytes(bytes([data[0] ^ 1]) + data[1:])
        os.utime(path, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))

    monkeypatch.setattr(bundle, "staged_output_path", record_stage)
    if role == "manifest":
        check_destination = bundle._ensure_destination_has_no_link
        mutated = False

        def mutate_after_manifest_capture(root: Path, relative: PurePosixPath) -> None:
            nonlocal mutated
            check_destination(root, relative)
            if relative == PurePosixPath(bundle.MANIFEST_NAME) and not mutated:
                mutate()
                mutated = True

        monkeypatch.setattr(
            bundle, "_ensure_destination_has_no_link", mutate_after_manifest_capture
        )
    else:
        validate_metadata = bundle._validate_staged_link_metadata

        def mutate_after_validation(*args, **kwargs) -> None:
            validate_metadata(*args, **kwargs)
            mutate()

        monkeypatch.setattr(
            bundle, "_validate_staged_link_metadata", mutate_after_validation
        )
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="changed"):
        bundle.verify_extract_bundle(
            archive=archive,
            destination=destination,
            expected_identity=IDENTITY,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )
    for final, previous in original_outputs.items():
        assert final.read_bytes() == previous


@pytest.mark.parametrize("role", ["staged bundle archive", "staged bundle manifest"])
def test_pack_retains_staged_generations_until_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, role: str
) -> None:
    target_root = _built_target(tmp_path)
    output = tmp_path / "runtime.tar"
    manifest_output = tmp_path / "runtime.json"
    output.write_bytes(b"previous archive")
    manifest_output.write_bytes(b"previous manifest")
    captured = None
    capture = bundle._capture_file
    verify = bundle._require_unchanged

    def record_capture(path: Path, **kwargs):
        nonlocal captured
        identity = capture(path, **kwargs)
        if kwargs["role"] == role:
            captured = identity
        return identity

    mutated = False

    def mutate_before_final_verify(identity) -> None:
        nonlocal mutated
        if captured is not None and identity.path != captured.path and not mutated:
            metadata = captured.path.stat()
            data = captured.path.read_bytes()
            captured.path.write_bytes(bytes([data[0] ^ 1]) + data[1:])
            os.utime(captured.path, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
            mutated = True
        verify(identity)

    monkeypatch.setattr(bundle, "_capture_file", record_capture)
    monkeypatch.setattr(bundle, "_require_unchanged", mutate_before_final_verify)
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="changed"):
        bundle.pack_bundle(
            target_root=target_root,
            output=output,
            manifest_output=manifest_output,
            identity=IDENTITY,
            runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )
    assert mutated
    assert output.read_bytes() == b"previous archive"
    assert manifest_output.read_bytes() == b"previous manifest"


def _tar_payloads(path: Path) -> list[tuple[tarfile.TarInfo, bytes]]:
    payloads: list[tuple[tarfile.TarInfo, bytes]] = []
    with tarfile.open(path, "r:") as archive:
        for member in archive:
            stream = archive.extractfile(member)
            assert stream is not None
            payloads.append((member, stream.read()))
    return payloads


def _write_tar(path: Path, members: list[tuple[tarfile.TarInfo, bytes]]) -> None:
    with tarfile.open(path, "w", format=tarfile.USTAR_FORMAT) as archive:
        for info, payload in members:
            info.size = len(payload)
            archive.addfile(info, io.BytesIO(payload))


def _regular_info(name: str, payload: bytes, *, mode: int = 0o644) -> tarfile.TarInfo:
    info = tarfile.TarInfo(name)
    info.size = len(payload)
    info.mode = mode
    info.uid = 0
    info.gid = 0
    info.mtime = 0
    return info


def test_pack_selects_exact_payload_and_is_byte_deterministic(tmp_path: Path) -> None:
    target_root = _built_target(tmp_path)
    first = tmp_path / "first.tar"
    first_manifest = tmp_path / "first.json"
    second = tmp_path / "second.tar"
    second_manifest = tmp_path / "second.json"

    payload = bundle.pack_bundle(
        runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        target_root=target_root,
        output=first,
        manifest_output=first_manifest,
        identity=IDENTITY,
    )
    bundle.pack_bundle(
        runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        target_root=target_root,
        output=second,
        manifest_output=second_manifest,
        identity=IDENTITY,
    )

    assert first.read_bytes() == second.read_bytes()
    assert first_manifest.read_bytes() == second_manifest.read_bytes()
    with tarfile.open(first, "r:") as archive:
        members = archive.getmembers()
    assert [member.name for member in members] == [
        bundle.MANIFEST_NAME,
        "dev-fast/libmolt_runtime.stdlib_full.a",
        "dev-fast/libmolt_runtime.stdlib_full.a.native-link-deps.json",
        "dev-fast/molt-backend",
    ]
    assert [member.mode for member in members] == [0o644, 0o644, 0o644, 0o755]
    assert all(member.mtime == 0 for member in members)
    assert all(member.uid == member.gid == 0 for member in members)
    assert [record["role"] for record in payload["files"]] == [
        bundle.RUNTIME_ROLE,
        bundle.LINK_ROLE,
        bundle.BACKEND_ROLE,
    ]
    runtime_record = payload["files"][0]
    assert runtime_record["artifact_identity"]["schema"] == (
        "molt.static-archive-semantic.v1"
    )


def test_verify_extract_publishes_hash_checked_files_and_modes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive, _manifest_path, expected = _pack(tmp_path)
    destination = tmp_path / "hydrated-target"
    original_publish = bundle.publish_validated_outputs
    observed_pairs: list[tuple[Path, Path]] = []

    def publish_same_directory(pairs: list[tuple[Path, Path]]) -> tuple[Path, ...]:
        observed_pairs.extend(pairs)
        assert all(staged.parent == final.parent for staged, final in pairs)
        return original_publish(pairs)

    monkeypatch.setattr(bundle, "publish_validated_outputs", publish_same_directory)

    actual = bundle.verify_extract_bundle(
        archive=archive,
        destination=destination,
        expected_identity=IDENTITY,
        expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
    )

    assert actual == expected
    assert len(observed_pairs) == 4
    assert (
        json.loads((destination / bundle.MANIFEST_NAME).read_text(encoding="utf-8"))
        == expected
    )
    runtime = destination / "dev-fast" / "libmolt_runtime.stdlib_full.a"
    link_manifest = runtime.with_name(f"{runtime.name}.native-link-deps.json")
    backend = destination / "dev-fast" / "molt-backend"
    assert runtime.read_bytes().startswith(b"!<arch>\n")
    assert link_manifest.is_file()
    assert backend.read_bytes().startswith(b"\x7fELF")
    if os.name == "posix":
        assert stat.S_IMODE(runtime.stat().st_mode) == 0o644
        assert stat.S_IMODE(link_manifest.stat().st_mode) == 0o644
        assert stat.S_IMODE(backend.stat().st_mode) == 0o755


def test_pack_rejects_non_executable_backend(tmp_path: Path) -> None:
    if os.name != "posix":
        pytest.skip("Windows does not expose portable POSIX executable mode bits")
    target_root = _built_target(tmp_path)
    (target_root / bundle.PROFILE / "molt-backend").chmod(0o644)

    with pytest.raises(bundle.NightlyRuntimeBundleError, match="no executable bit"):
        bundle.pack_bundle(
            runtime_build_identity=RUNTIME_BUILD_IDENTITY,
            target_root=target_root,
            output=tmp_path / "bundle.tar",
            manifest_output=tmp_path / "manifest.json",
            identity=IDENTITY,
        )


def test_failed_hydration_rolls_back_payload_but_retains_parent_lock_custody(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive, _manifest, _expected = _pack(tmp_path)
    destination = tmp_path / "hydrated"
    replace = artifact_publication._durable_replace

    def fail_manifest(stage: Path, final: Path) -> None:
        if final == destination / bundle.MANIFEST_NAME:
            raise OSError("manifest publication interrupted")
        replace(stage, final)

    monkeypatch.setattr(artifact_publication, "_durable_replace", fail_manifest)
    with pytest.raises(
        bundle.NightlyRuntimeBundleError, match="manifest publication interrupted"
    ):
        bundle.verify_extract_bundle(
            archive=archive,
            destination=destination,
            expected_identity=IDENTITY,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )
    retained = [path for path in destination.rglob("*") if path.is_file()]
    assert {path.parent for path in retained} == {
        destination,
        destination / bundle.PROFILE,
    }
    assert all(artifact_publication.is_publication_lock_file(path) for path in retained)


def test_pack_rejects_link_manifest_not_bound_to_runtime(tmp_path: Path) -> None:
    target_root = _built_target(tmp_path)
    runtime = target_root / bundle.PROFILE / "libmolt_runtime.stdlib_full.a"
    runtime.write_bytes(runtime.read_bytes() + b"changed")

    with pytest.raises(
        bundle.NightlyRuntimeBundleError,
        match="does not attest the selected runtime archive",
    ):
        bundle.pack_bundle(
            runtime_build_identity=RUNTIME_BUILD_IDENTITY,
            target_root=target_root,
            output=tmp_path / "bundle.tar",
            manifest_output=tmp_path / "manifest.json",
            identity=IDENTITY,
        )


def test_pack_rejects_runtime_built_from_other_build_identity(
    tmp_path: Path,
) -> None:
    target_root = _built_target(tmp_path)
    other_build_identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast",
        target_triple=None,
        family_seed="other-native-link-test-family",
    )

    with pytest.raises(
        bundle.NightlyRuntimeBundleError,
        match="does not attest the selected runtime archive",
    ):
        bundle.pack_bundle(
            runtime_build_identity=other_build_identity,
            target_root=target_root,
            output=tmp_path / "bundle.tar",
            manifest_output=tmp_path / "manifest.json",
            identity=IDENTITY,
        )


def test_verify_extract_rejects_file_hash_mismatch_without_publication(
    tmp_path: Path,
) -> None:
    archive, _manifest_path, _payload = _pack(tmp_path)
    changed: list[tuple[tarfile.TarInfo, bytes]] = []
    for info, payload in _tar_payloads(archive):
        if info.name == "dev-fast/molt-backend":
            payload = bytes([payload[0] ^ 0xFF]) + payload[1:]
        changed.append((info, payload))
    tampered = tmp_path / "tampered.tar"
    _write_tar(tampered, changed)
    destination = tmp_path / "hydrated"

    with pytest.raises(bundle.NightlyRuntimeBundleError, match="hash does not match"):
        bundle.verify_extract_bundle(
            archive=tampered,
            destination=destination,
            expected_identity=IDENTITY,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )

    assert not (destination / "dev-fast").exists()
    assert not (destination / bundle.MANIFEST_NAME).exists()


def test_verify_extract_rejects_tar_mode_not_attested_by_manifest(
    tmp_path: Path,
) -> None:
    archive, _manifest_path, _payload = _pack(tmp_path)
    members = _tar_payloads(archive)
    for info, _payload_bytes in members:
        if info.name == "dev-fast/molt-backend":
            info.mode = 0o777
    malformed = tmp_path / "mode.tar"
    _write_tar(malformed, members)

    with pytest.raises(bundle.NightlyRuntimeBundleError, match="mode does not match"):
        bundle.verify_extract_bundle(
            archive=malformed,
            destination=tmp_path / "hydrated",
            expected_identity=IDENTITY,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )


def test_verify_extract_rejects_source_or_target_mismatch(
    tmp_path: Path,
) -> None:
    archive, _manifest_path, _payload = _pack(tmp_path)
    other = bundle.BundleIdentity(
        source_commit="2" * 40,
        target_triple="x86_64-unknown-linux-gnu",
    )

    with pytest.raises(bundle.NightlyRuntimeBundleError, match="does not match"):
        bundle.verify_extract_bundle(
            archive=archive,
            destination=tmp_path / "hydrated",
            expected_identity=other,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )


def test_verify_extract_rejects_runtime_build_identity_mismatch(
    tmp_path: Path,
) -> None:
    archive, _manifest_path, _payload = _pack(tmp_path)
    other_build_identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast",
        target_triple=None,
        family_seed="other-native-link-test-family",
    )

    with pytest.raises(bundle.NightlyRuntimeBundleError, match="build identity"):
        bundle.verify_extract_bundle(
            archive=archive,
            destination=tmp_path / "hydrated",
            expected_identity=IDENTITY,
            expected_runtime_build_identity=other_build_identity,
        )


@pytest.mark.parametrize(
    ("name", "type_code", "message"),
    [
        ("../escape", tarfile.REGTYPE, "unsafe archive member path"),
        ("/absolute", tarfile.REGTYPE, "unsafe archive member path"),
        ("dev-fast\\escape", tarfile.REGTYPE, "unsafe archive member path"),
        ("dev-fast/symlink", tarfile.SYMTYPE, "not a regular file"),
        ("dev-fast/hardlink", tarfile.LNKTYPE, "not a regular file"),
        ("dev-fast/device", tarfile.CHRTYPE, "not a regular file"),
        ("dev-fast/fifo", tarfile.FIFOTYPE, "not a regular file"),
    ],
)
def test_verify_extract_rejects_unsafe_member_classes(
    tmp_path: Path,
    name: str,
    type_code: bytes,
    message: str,
) -> None:
    payload = b"bad"
    member = _regular_info(name, payload)
    member.type = type_code
    if type_code in {tarfile.SYMTYPE, tarfile.LNKTYPE}:
        member.linkname = "../outside"
        member.size = 0
        payload = b""
    malformed = tmp_path / "malformed.tar"
    _write_tar(malformed, [(member, payload)])

    with pytest.raises(bundle.NightlyRuntimeBundleError, match=message):
        bundle.verify_extract_bundle(
            archive=malformed,
            destination=tmp_path / "hydrated",
            expected_identity=IDENTITY,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )


@pytest.mark.parametrize(
    "limit_name, message",
    [
        ("_MAX_MANIFEST_BYTES", "bundle manifest exceeds safety limit"),
        ("_MAX_BUNDLE_PAYLOAD_BYTES", "bundle payload exceeds safety limit"),
    ],
)
def test_pack_rejects_oversized_payload_before_publication(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    limit_name: str,
    message: str,
) -> None:
    target_root = _built_target(tmp_path)
    output = tmp_path / "runtime.tar"
    manifest_output = tmp_path / "runtime.json"
    output.write_bytes(b"existing archive")
    manifest_output.write_bytes(b"existing manifest")
    monkeypatch.setattr(bundle, limit_name, 1)
    with pytest.raises(bundle.NightlyRuntimeBundleError, match=message):
        bundle.pack_bundle(
            runtime_build_identity=RUNTIME_BUILD_IDENTITY,
            target_root=target_root,
            output=output,
            manifest_output=manifest_output,
            identity=IDENTITY,
        )
    assert output.read_bytes() == b"existing archive"
    assert manifest_output.read_bytes() == b"existing manifest"


def test_verify_extract_rejects_oversized_archive_before_hashing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive = tmp_path / "oversized.tar"
    archive.write_bytes(b"ninebytes")
    monkeypatch.setattr(bundle, "_MAX_BUNDLE_ARCHIVE_BYTES", 8)

    def unexpected_hash(*_args, **_kwargs):
        pytest.fail("oversized archive must not reach content hashing")

    monkeypatch.setattr(bundle, "stable_regular_file_identity", unexpected_hash)
    destination = tmp_path / "hydrated"
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="exceeds safety limit"):
        bundle.verify_extract_bundle(
            archive=archive,
            destination=destination,
            expected_identity=IDENTITY,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )
    assert not destination.exists()


def test_capture_bundle_input_enforces_exact_pre_hash_bound(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "input"
    source.write_bytes(b"boundary")
    captured = bundle._capture_file(source, role="test input", max_bytes=8)
    assert captured.size == 8

    def unexpected_hash(*_args, **_kwargs):
        pytest.fail("oversized input must not reach content hashing")

    monkeypatch.setattr(bundle, "stable_regular_file_identity", unexpected_hash)
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="exceeds safety limit"):
        bundle._capture_file(source, role="test input", max_bytes=7)


def test_verify_extract_wraps_late_tar_failure_and_removes_staging(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive, _manifest, _payload = _pack(tmp_path)
    extract = tarfile.TarFile.extractfile

    def fail_payload(source: tarfile.TarFile, member):
        if member.name != bundle.MANIFEST_NAME:
            raise tarfile.ReadError("late payload truncation")
        return extract(source, member)

    monkeypatch.setattr(tarfile.TarFile, "extractfile", fail_payload)
    destination = tmp_path / "hydrated"
    with pytest.raises(
        bundle.NightlyRuntimeBundleError, match="late payload truncation"
    ):
        bundle.verify_extract_bundle(
            archive=archive,
            destination=destination,
            expected_identity=IDENTITY,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )
    assert not (destination / bundle.PROFILE).exists()


def test_verify_extract_rejects_duplicate_archive_member(tmp_path: Path) -> None:
    manifest = b"{}\n"
    first = _regular_info(bundle.MANIFEST_NAME, manifest)
    second = _regular_info(bundle.MANIFEST_NAME, manifest)
    malformed = tmp_path / "duplicate.tar"
    _write_tar(malformed, [(first, manifest), (second, manifest)])

    with pytest.raises(bundle.NightlyRuntimeBundleError, match="duplicate archive"):
        bundle.verify_extract_bundle(
            archive=malformed,
            destination=tmp_path / "hydrated",
            expected_identity=IDENTITY,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )


def test_verify_extract_rejects_duplicate_manifest_key(tmp_path: Path) -> None:
    raw = b'{"schema_version":1,"schema_version":1}\n'
    malformed = tmp_path / "duplicate-key.tar"
    _write_tar(
        malformed,
        [(_regular_info(bundle.MANIFEST_NAME, raw), raw)],
    )

    with pytest.raises(bundle.NightlyRuntimeBundleError, match="duplicate JSON key"):
        bundle.verify_extract_bundle(
            archive=malformed,
            destination=tmp_path / "hydrated",
            expected_identity=IDENTITY,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )


def test_verify_extract_rejects_unknown_extra_member(tmp_path: Path) -> None:
    archive, _manifest_path, _payload = _pack(tmp_path)
    members = _tar_payloads(archive)
    extra = b"unexpected"
    members.append((_regular_info("dev-fast/extra", extra), extra))
    malformed = tmp_path / "extra.tar"
    _write_tar(malformed, members)

    with pytest.raises(bundle.NightlyRuntimeBundleError, match="closure mismatch"):
        bundle.verify_extract_bundle(
            archive=malformed,
            destination=tmp_path / "hydrated",
            expected_identity=IDENTITY,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )


def test_verify_extract_preserves_existing_outputs_on_late_semantic_failure(
    tmp_path: Path,
) -> None:
    archive, _manifest_path, payload = _pack(tmp_path)
    payload["files"][0]["artifact_identity"] = {
        "schema": "molt.static-archive-semantic.v1",
        "semantic_sha256": "f" * 64,
        "member_count": 1,
        "content_size_bytes": 1,
    }
    encoded = encode_exact(payload)
    changed = []
    for info, member_payload in _tar_payloads(archive):
        if info.name == bundle.MANIFEST_NAME:
            member_payload = encoded
        changed.append((info, member_payload))
    malformed = tmp_path / "semantic-mismatch.tar"
    _write_tar(malformed, changed)
    destination = tmp_path / "hydrated"
    existing = destination / "dev-fast" / "molt-backend"
    existing.parent.mkdir(parents=True)
    existing.write_bytes(b"existing")

    with pytest.raises(bundle.NightlyRuntimeBundleError, match="semantic identity"):
        bundle.verify_extract_bundle(
            archive=malformed,
            destination=destination,
            expected_identity=IDENTITY,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )

    assert existing.read_bytes() == b"existing"
    assert not (destination / bundle.MANIFEST_NAME).exists()


def test_verify_extract_rejects_tampered_native_link_protocol_before_publication(
    tmp_path: Path,
) -> None:
    archive, _manifest_path, bundle_manifest = _pack(tmp_path)
    link_path = "dev-fast/libmolt_runtime.stdlib_full.a.native-link-deps.json"
    changed: list[tuple[tarfile.TarInfo, bytes]] = []
    tampered_link_payload: bytes | None = None
    source_members = _tar_payloads(archive)
    for info, member_payload in source_members:
        if info.name == link_path:
            link_manifest = json.loads(member_payload)
            link_manifest["link_plan"]["items"] = [
                {
                    "kind": "system-library",
                    "argument": "-lwrong",
                    "unexpected": True,
                }
            ]
            tampered_link_payload = (
                json.dumps(link_manifest, indent=2, sort_keys=True).encode("utf-8")
                + b"\n"
            )
            member_payload = tampered_link_payload
        changed.append((info, member_payload))
    assert tampered_link_payload is not None
    link_record = bundle_manifest["files"][1]
    link_record["size_bytes"] = len(tampered_link_payload)
    link_record["sha256"] = hashlib.sha256(tampered_link_payload).hexdigest()
    encoded_manifest = encode_exact(bundle_manifest)
    changed = [
        (info, encoded_manifest if info.name == bundle.MANIFEST_NAME else payload)
        for info, payload in changed
    ]
    malformed = tmp_path / "tampered-link-protocol.tar"
    _write_tar(malformed, changed)
    destination = tmp_path / "hydrated"

    with pytest.raises(
        bundle.NightlyRuntimeBundleError,
        match="invalid system-library item",
    ):
        bundle.verify_extract_bundle(
            archive=malformed,
            destination=destination,
            expected_identity=IDENTITY,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )

    assert not (destination / "dev-fast").exists()
    assert not (destination / bundle.MANIFEST_NAME).exists()


@pytest.mark.parametrize(
    ("system", "machine", "target_triple", "runtime_name", "backend_name"),
    [
        (
            "linux",
            "x86_64",
            "x86_64-unknown-linux-gnu",
            "libmolt_runtime.stdlib_full.a",
            "molt-backend",
        ),
        (
            "macos",
            "aarch64",
            "aarch64-apple-darwin",
            "libmolt_runtime.stdlib_full.a",
            "molt-backend",
        ),
        (
            "windows",
            "x86_64",
            "x86_64-pc-windows-msvc",
            "molt_runtime.stdlib_full.lib",
            "molt-backend.exe",
        ),
    ],
)
def test_bundle_identity_gates_portable_native_target_cells(
    system: str,
    machine: str,
    target_triple: str,
    runtime_name: str,
    backend_name: str,
) -> None:
    identity = bundle.BundleIdentity(
        source_commit="1" * 40,
        target_triple=target_triple,
    )
    assert identity.target_triple == target_triple
    assert identity.platform_system == system
    assert identity.platform_machine == machine
    assert bundle._runtime_archive_name(identity) == runtime_name
    assert bundle._backend_executable_name(identity) == backend_name


@pytest.mark.parametrize(
    "target_triple",
    [
        "x86_64-unknown-freebsd",
        "riscv64gc-unknown-linux-gnu",
        "x86_64-unknown-linux-musl",
    ],
)
def test_bundle_identity_rejects_unsupported_target_cells(
    target_triple: str,
) -> None:
    with pytest.raises(ValueError, match="unsupported.*target"):
        bundle.BundleIdentity(
            source_commit="1" * 40,
            target_triple=target_triple,
        )


@pytest.mark.parametrize("target", list(bundle._NATIVE_TARGET_CELLS))
def test_bundle_platform_is_derived_from_captured_runtime_target_without_host_probe(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, target: str
) -> None:
    build_identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast", host_target=target
    )
    monkeypatch.setattr(
        bundle,
        "_run_identity_command",
        lambda command, **_kwargs: "1" * 40 if "rev-parse" in command else "",
    )
    identity = bundle.collect_bundle_identity(
        tmp_path, runtime_build_identity=build_identity
    )
    assert identity.target_triple == target
    assert (
        identity.platform_system,
        identity.platform_machine,
    ) == bundle._NATIVE_TARGET_CELLS[target]
    assert bundle._validated_identity(identity.as_dict()) == identity
    archive = tmp_path / "runtime.tar"
    manifest = bundle.pack_bundle(
        target_root=_built_target(
            tmp_path, identity=identity, build_identity=build_identity
        ),
        output=archive,
        manifest_output=tmp_path / "runtime.json",
        identity=identity,
        runtime_build_identity=build_identity,
    )
    assert (
        bundle.verify_extract_bundle(
            archive=archive,
            destination=tmp_path / "hydrated",
            expected_identity=identity,
            expected_runtime_build_identity=build_identity,
        )
        == manifest
    )


def test_bundle_rejects_internally_consistent_but_foreign_target_projection(
    tmp_path: Path,
) -> None:
    _archive, _path, manifest = _pack(tmp_path)
    foreign = bundle.BundleIdentity("1" * 40, "aarch64-apple-darwin")
    manifest["identity"] = foreign.as_dict()
    with pytest.raises(
        bundle.NightlyRuntimeBundleError, match="captured runtime effective target"
    ):
        bundle.validate_manifest(
            manifest,
            expected_identity=foreign,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )


def test_bundle_never_labels_musl_runtime_as_gnu() -> None:
    build_identity = native_runtime_staticlib_identity(
        cargo_profile="dev-fast", host_target="x86_64-unknown-linux-musl"
    )
    with pytest.raises(ValueError, match="unsupported.*target"):
        bundle.BundleIdentity.from_runtime("1" * 40, build_identity)


@pytest.mark.parametrize(
    "constant", ["NaN", "Infinity", "-Infinity", "1e9999", "-1e9999"]
)
def test_manifest_uses_exact_json_for_nonfinite_values(constant: str) -> None:
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="non-finite JSON"):
        bundle._read_manifest_bytes(('{"extra": ' + constant + "}").encode())


@pytest.mark.parametrize("field", ["system", "machine"])
@pytest.mark.parametrize("value", [None, False, 1, [], {}])
def test_bundle_identity_never_coerces_nonstring_platform_values(
    field: str, value: object
) -> None:
    identity = IDENTITY.as_dict()
    identity["platform"][field] = value
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="non-empty strings"):
        bundle._validated_identity(identity)


def test_file_record_required_fields_follow_typed_schema() -> None:
    assert bundle.BundleFileRecord.__required_keys__ == {
        "role",
        "path",
        "size_bytes",
        "sha256",
        "mode",
    }
    assert bundle.BundleFileRecord.__optional_keys__ == {"artifact_identity"}


@pytest.mark.parametrize("version", [True, 3.0])
def test_bundle_rejects_noninteger_schema_version(
    tmp_path: Path, version: object
) -> None:
    _archive, _manifest, payload = _pack(tmp_path)
    payload["schema_version"] = version
    with pytest.raises(bundle.NightlyRuntimeBundleError, match="schema is unsupported"):
        bundle.validate_manifest(
            payload,
            expected_identity=IDENTITY,
            expected_runtime_build_identity=RUNTIME_BUILD_IDENTITY,
        )
