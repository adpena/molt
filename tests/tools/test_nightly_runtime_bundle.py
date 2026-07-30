from __future__ import annotations

import io
import json
import os
from pathlib import Path
import stat
import tarfile

import pytest

from tests.cli.native_link_test_support import (
    SOURCE_FINGERPRINT,
    write_test_native_link_manifest,
    write_test_static_archive,
)
from tools import nightly_runtime_bundle as bundle


IDENTITY = bundle.BundleIdentity(
    source_commit="1" * 40,
    platform_system="linux",
    platform_machine="x86_64",
    rustc_verbose="rustc 1.99.0-nightly\nhost: x86_64-unknown-linux-gnu",
    cargo_version="cargo 1.99.0-nightly",
)


@pytest.fixture(autouse=True)
def _runtime_source_fingerprint_authority(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        bundle,
        "current_runtime_source_fingerprint",
        lambda *_args, **_kwargs: dict(SOURCE_FINGERPRINT),
    )


def _built_target(tmp_path: Path) -> Path:
    target_root = tmp_path / "target"
    profile_root = target_root / bundle.PROFILE
    profile_root.mkdir(parents=True)
    runtime = profile_root / "libmolt_runtime.stdlib_full.a"
    write_test_static_archive(runtime, b"runtime object bytes")
    write_test_native_link_manifest(runtime, source_root=tmp_path)
    backend = profile_root / "molt-backend"
    backend.write_bytes(b"\x7fELF\x02\x01molt backend")
    backend.chmod(0o755)
    return target_root


def _pack(tmp_path: Path) -> tuple[Path, Path, dict[str, object]]:
    archive = tmp_path / "nightly-runtime.tar"
    manifest = tmp_path / "nightly-runtime.json"
    payload = bundle.pack_bundle(
        project_root=tmp_path,
        target_root=_built_target(tmp_path),
        output=archive,
        manifest_output=manifest,
        identity=IDENTITY,
    )
    return archive, manifest, payload


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
        project_root=tmp_path,
        target_root=target_root,
        output=first,
        manifest_output=first_manifest,
        identity=IDENTITY,
    )
    bundle.pack_bundle(
        project_root=tmp_path,
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


def test_verify_extract_publishes_hash_checked_files_and_modes(tmp_path: Path) -> None:
    archive, _manifest_path, expected = _pack(tmp_path)
    destination = tmp_path / "hydrated-target"

    actual = bundle.verify_extract_bundle(
        archive=archive,
        destination=destination,
        expected_identity=IDENTITY,
        expected_runtime_source_fingerprint=SOURCE_FINGERPRINT,
    )

    assert actual == expected
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
            project_root=tmp_path,
            target_root=target_root,
            output=tmp_path / "bundle.tar",
            manifest_output=tmp_path / "manifest.json",
            identity=IDENTITY,
        )


def test_pack_rejects_link_manifest_not_bound_to_runtime(tmp_path: Path) -> None:
    target_root = _built_target(tmp_path)
    runtime = target_root / bundle.PROFILE / "libmolt_runtime.stdlib_full.a"
    runtime.write_bytes(runtime.read_bytes() + b"changed")

    with pytest.raises(
        bundle.NightlyRuntimeBundleError,
        match="does not attest the selected runtime archive",
    ):
        bundle.pack_bundle(
            project_root=tmp_path,
            target_root=target_root,
            output=tmp_path / "bundle.tar",
            manifest_output=tmp_path / "manifest.json",
            identity=IDENTITY,
        )


def test_pack_rejects_runtime_built_from_other_source_fingerprint(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target_root = _built_target(tmp_path)
    monkeypatch.setattr(
        bundle,
        "current_runtime_source_fingerprint",
        lambda *_args, **_kwargs: {**SOURCE_FINGERPRINT, "hash": "f" * 64},
    )

    with pytest.raises(
        bundle.NightlyRuntimeBundleError,
        match="does not attest the selected runtime archive",
    ):
        bundle.pack_bundle(
            project_root=tmp_path,
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
            expected_runtime_source_fingerprint=SOURCE_FINGERPRINT,
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
            expected_runtime_source_fingerprint=SOURCE_FINGERPRINT,
        )


def test_verify_extract_rejects_source_toolchain_or_platform_mismatch(
    tmp_path: Path,
) -> None:
    archive, _manifest_path, _payload = _pack(tmp_path)
    other = bundle.BundleIdentity(
        source_commit="2" * 40,
        platform_system="linux",
        platform_machine="x86_64",
        rustc_verbose=IDENTITY.rustc_verbose,
        cargo_version=IDENTITY.cargo_version,
    )

    with pytest.raises(bundle.NightlyRuntimeBundleError, match="does not match"):
        bundle.verify_extract_bundle(
            archive=archive,
            destination=tmp_path / "hydrated",
            expected_identity=other,
            expected_runtime_source_fingerprint=SOURCE_FINGERPRINT,
        )


def test_verify_extract_rejects_runtime_source_fingerprint_mismatch(
    tmp_path: Path,
) -> None:
    archive, _manifest_path, _payload = _pack(tmp_path)
    other_fingerprint = {**SOURCE_FINGERPRINT, "hash": "f" * 64}

    with pytest.raises(bundle.NightlyRuntimeBundleError, match="fingerprint"):
        bundle.verify_extract_bundle(
            archive=archive,
            destination=tmp_path / "hydrated",
            expected_identity=IDENTITY,
            expected_runtime_source_fingerprint=other_fingerprint,
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
            expected_runtime_source_fingerprint=SOURCE_FINGERPRINT,
        )


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
            expected_runtime_source_fingerprint=SOURCE_FINGERPRINT,
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
            expected_runtime_source_fingerprint=SOURCE_FINGERPRINT,
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
            expected_runtime_source_fingerprint=SOURCE_FINGERPRINT,
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
    encoded = bundle._manifest_bytes(payload)
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
            expected_runtime_source_fingerprint=SOURCE_FINGERPRINT,
        )

    assert existing.read_bytes() == b"existing"
    assert not (destination / bundle.MANIFEST_NAME).exists()


def test_linux_bundle_identity_rejects_other_platforms() -> None:
    with pytest.raises(ValueError, match="require Linux"):
        bundle.BundleIdentity(
            source_commit="1" * 40,
            platform_system="windows",
            platform_machine="x86_64",
            rustc_verbose="rustc test",
            cargo_version="cargo test",
        )
