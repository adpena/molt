from __future__ import annotations

from copy import deepcopy
import hashlib
import io
import json
import os
from pathlib import Path
import tarfile
from typing import cast

import pytest

from molt.cli.native_link_custody import (
    NativeLinkCustodyError,
    ensure_native_link_custody,
    native_link_custody_archive_path,
    publish_native_link_custody,
    validate_native_link_custody,
    validate_native_link_custody_archive,
)
from molt.cli import native_link_custody as custody_authority
from molt.exact_json import canonical_json_sha256


def _published_custody(
    root: Path,
) -> tuple[Path, dict[str, object], Path, dict[str, object]]:
    runtime = root / "runtime" / "libmolt_runtime.a"
    dependency = root / "producer" / "libdependency.a"
    runtime.parent.mkdir(parents=True)
    dependency.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    dependency.write_bytes(b"dependency")
    custody, _source_ids = publish_native_link_custody(runtime, (dependency,))
    archive = native_link_custody_archive_path(runtime, custody)
    assert archive is not None
    entries = cast(list[object], custody["entries"])
    entry = cast(dict[str, object], entries[0])
    return runtime, custody, archive, entry


def test_custody_rejects_retired_schema(tmp_path: Path) -> None:
    _runtime, custody, _archive, _entry = _published_custody(tmp_path)
    custody["schema"] = "molt.native-link-custody.v1"
    with pytest.raises(NativeLinkCustodyError, match="unsupported.*schema"):
        validate_native_link_custody(custody, context="retired schema")


@pytest.mark.parametrize(
    "filename",
    [
        "C:payload.a",
        "thing.a:stream",
        "NUL.a",
        "CON .txt",
        "CONIN$",
        "tail.a.",
        "tail.a ",
        "a/b.a",
        "a\\b.a",
        "..",
        "../lib.a",
    ],
)
def test_custody_rejects_nonportable_filenames_even_when_resealed(
    tmp_path: Path, filename: str
) -> None:
    _runtime, custody, _archive, entry = _published_custody(tmp_path)
    entry["filename"] = filename
    entry["id"] = canonical_json_sha256(
        {
            "filename": filename,
            "sha256": entry["sha256"],
            "size_bytes": entry["size_bytes"],
        }
    )
    entry["archive_path"] = f"files/{entry['id']}/{filename}"
    with pytest.raises(NativeLinkCustodyError, match="unsafe"):
        validate_native_link_custody(custody, context="resealed path")


def test_custody_unicode_identifier_uses_shared_utf8_json(tmp_path: Path) -> None:
    runtime = tmp_path / "libmolt_runtime.a"
    dependency = tmp_path / "libcaf\u00e9.a"
    dependency.write_bytes(b"unicode dependency")
    custody, _ids = publish_native_link_custody(runtime, (dependency,))
    entry = cast(dict[str, object], cast(list[object], custody["entries"])[0])
    material = {key: entry[key] for key in ("filename", "sha256", "size_bytes")}
    assert entry["id"] == canonical_json_sha256(material)
    assert set(ensure_native_link_custody(runtime, custody)) == {entry["id"]}
    old_identifier = hashlib.sha256(
        json.dumps(
            material, sort_keys=True, separators=(",", ":"), ensure_ascii=True
        ).encode("utf-8")
    ).hexdigest()
    assert old_identifier != entry["id"]
    entry["id"] = old_identifier
    entry["archive_path"] = f"files/{old_identifier}/{entry['filename']}"
    with pytest.raises(NativeLinkCustodyError, match="non-canonical"):
        validate_native_link_custody(custody, context="retired JSON digest")


def test_custody_rejects_same_size_source_mutation_during_streaming(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = tmp_path / "runtime" / "libmolt_runtime.a"
    source = tmp_path / "libinput.a"
    source.write_bytes(b"before")
    captured_stat = source.stat()
    original = custody_authority._tar_info

    def mutate(entry: custody_authority.NativeLinkCustodyEntry) -> tarfile.TarInfo:
        source.write_bytes(b"after!")
        os.utime(source, ns=(captured_stat.st_atime_ns, captured_stat.st_mtime_ns))
        return original(entry)

    monkeypatch.setattr(custody_authority, "_tar_info", mutate)
    with pytest.raises(NativeLinkCustodyError, match="changed"):
        publish_native_link_custody(runtime, (source,))
    assert not list(runtime.parent.glob("molt-native-link-custody-*.tar"))


def _tar_member(
    name: str,
    payload: bytes,
    *,
    mode: int = 0o644,
    mtime: int = 0,
) -> tuple[tarfile.TarInfo, bytes]:
    info = tarfile.TarInfo(name)
    info.size = len(payload)
    info.mode = mode
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.mtime = mtime
    info.type = tarfile.REGTYPE
    return info, payload


def _replace_archive(
    runtime: Path,
    custody: dict[str, object],
    members: list[tuple[tarfile.TarInfo, bytes]],
) -> dict[str, object]:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        for info, payload in members:
            archive.addfile(info, io.BytesIO(payload))
    encoded = buffer.getvalue()
    digest = hashlib.sha256(encoded).hexdigest()
    updated = deepcopy(custody)
    updated["archive"] = {
        "name": f"molt-native-link-custody-{digest}.tar",
        "sha256": digest,
        "size_bytes": len(encoded),
    }
    archive = native_link_custody_archive_path(runtime, updated)
    assert archive is not None
    archive.write_bytes(encoded)
    return updated


def test_custody_is_content_addressed_and_relocation_deterministic(
    tmp_path: Path,
) -> None:
    left_runtime, left, left_archive, _entry = _published_custody(tmp_path / "left")
    right_runtime, right, right_archive, _entry = _published_custody(tmp_path / "right")

    assert left == right
    assert left_archive.name == right_archive.name
    assert left_archive.read_bytes() == right_archive.read_bytes()
    assert json.dumps(left, sort_keys=True) == json.dumps(right, sort_keys=True)

    left_paths = ensure_native_link_custody(left_runtime, left)
    right_paths = ensure_native_link_custody(right_runtime, right)
    assert set(left_paths) == set(right_paths)
    assert {path.read_bytes() for path in left_paths.values()} == {b"dependency"}
    assert {path.read_bytes() for path in right_paths.values()} == {b"dependency"}
    assert all(tmp_path / "left" in path.parents for path in left_paths.values())
    assert all(tmp_path / "right" in path.parents for path in right_paths.values())


def test_custody_rejects_missing_and_tampered_archives(tmp_path: Path) -> None:
    runtime, custody, archive, _entry = _published_custody(tmp_path)
    original = archive.read_bytes()
    archive.unlink()
    with pytest.raises(NativeLinkCustodyError, match="unavailable"):
        ensure_native_link_custody(runtime, custody)

    archive.write_bytes(original[:-1] + bytes((original[-1] ^ 1,)))
    with pytest.raises(NativeLinkCustodyError, match="identity mismatch"):
        ensure_native_link_custody(runtime, custody)


def test_custody_rejects_empty_and_unrepresentable_inputs(tmp_path: Path) -> None:
    runtime = tmp_path / "runtime" / "libmolt_runtime.a"
    runtime.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    empty = tmp_path / "empty.a"
    empty.write_bytes(b"")
    with pytest.raises(NativeLinkCustodyError, match="must not be empty"):
        publish_native_link_custody(runtime, (empty,))

    long_name = tmp_path / ("x" * 110 + ".a")
    long_name.write_bytes(b"archive")
    with pytest.raises(NativeLinkCustodyError, match="deterministic.*archive"):
        publish_native_link_custody(runtime, (long_name,))


def test_explicit_archive_validator_accepts_staged_filename_and_exact_bytes(
    tmp_path: Path,
) -> None:
    _runtime, custody, archive, _entry = _published_custody(tmp_path)
    staged = tmp_path / "bundle-publication.stage"
    staged.write_bytes(archive.read_bytes())

    validate_native_link_custody_archive(staged, custody, context="nightly staging")
    staged.write_bytes(staged.read_bytes()[:-1])
    with pytest.raises(NativeLinkCustodyError, match="identity mismatch"):
        validate_native_link_custody_archive(
            staged,
            custody,
            context="nightly staging",
        )


@pytest.mark.parametrize("mutation", ["missing", "duplicate", "extra"])
def test_custody_rejects_non_exact_archive_member_closure(
    tmp_path: Path,
    mutation: str,
) -> None:
    runtime, custody, _archive, entry = _published_custody(tmp_path)
    canonical = _tar_member(str(entry["archive_path"]), b"dependency")
    if mutation == "missing":
        members: list[tuple[tarfile.TarInfo, bytes]] = []
    elif mutation == "duplicate":
        members = [canonical, _tar_member(str(entry["archive_path"]), b"dependency")]
    else:
        members = [canonical, _tar_member("files/extra/extra.a", b"extra")]
    updated = _replace_archive(runtime, custody, members)

    expected = "duplicate" if mutation == "duplicate" else "closure|invalid"
    with pytest.raises(NativeLinkCustodyError, match=expected):
        ensure_native_link_custody(runtime, updated)


def test_custody_rejects_noncanonical_archive_metadata(tmp_path: Path) -> None:
    runtime, custody, _archive, entry = _published_custody(tmp_path)
    updated = _replace_archive(
        runtime,
        custody,
        [_tar_member(str(entry["archive_path"]), b"dependency", mtime=1)],
    )

    with pytest.raises(NativeLinkCustodyError, match="invalid.*member"):
        ensure_native_link_custody(runtime, updated)


def test_custody_rejects_noncanonical_archive_member_order(tmp_path: Path) -> None:
    runtime = tmp_path / "runtime" / "libmolt_runtime.a"
    first = tmp_path / "producer" / "libfirst.a"
    second = tmp_path / "producer" / "libsecond.a"
    runtime.parent.mkdir(parents=True)
    first.parent.mkdir(parents=True)
    runtime.write_bytes(b"runtime")
    first.write_bytes(b"first")
    second.write_bytes(b"second")
    custody, _source_ids = publish_native_link_custody(runtime, (first, second))
    entries = cast(list[dict[str, object]], custody["entries"])
    assert len(entries) == 2
    payloads = {"libfirst.a": b"first", "libsecond.a": b"second"}
    reversed_members = [
        _tar_member(str(entry["archive_path"]), payloads[str(entry["filename"])])
        for entry in reversed(entries)
    ]
    updated = _replace_archive(runtime, custody, reversed_members)

    with pytest.raises(NativeLinkCustodyError, match="closure mismatch"):
        ensure_native_link_custody(runtime, updated)


def test_custody_revalidates_existing_extraction(tmp_path: Path) -> None:
    runtime, custody, _archive, _entry = _published_custody(tmp_path)
    paths = ensure_native_link_custody(runtime, custody)
    extracted = next(iter(paths.values()))
    extracted.write_bytes(b"tampered!!")

    with pytest.raises(NativeLinkCustodyError, match="identity mismatch"):
        ensure_native_link_custody(runtime, custody)
