"""Release ZIP authority regressions, including preserved donor contracts."""

from __future__ import annotations

from dataclasses import replace
import os
from pathlib import Path
import stat
import struct
import zipfile

import pytest

from molt.toolchain_identity import open_stable_regular_file
from tools.release import archive as release_archive
from tools.release import verify_consumer

EPOCH = 1_700_000_001


def _source(tmp_path: Path) -> Path:
    root = tmp_path / "source"
    (root / "bin").mkdir(parents=True)
    (root / "bin" / "tool").write_bytes(b"tool")
    (root / "data").write_bytes(b"data")
    return root


def _zip(path: Path, names: tuple[str, ...] = ("file",)) -> Path:
    with zipfile.ZipFile(path, "w") as archive:
        for name in names:
            member = zipfile.ZipInfo(name)
            # Windows ZipInfo construction normalizes backslashes. Preserve the
            # deliberately malformed wire spelling, not a corrected fixture.
            member.filename = name
            archive.writestr(member, b"" if name.endswith("/") else b"payload")
    return path


def test_canonical_zip_is_deterministic_and_round_trips(tmp_path: Path) -> None:
    root = _source(tmp_path)
    first, second = tmp_path / "first.zip", tmp_path / "second.zip"
    for output in (first, second):
        release_archive.write_reproducible_zip(root, output, source_date_epoch=EPOCH)
    assert first.read_bytes() == second.read_bytes()
    assert release_archive.same_regular_file_bytes(first, second)
    with zipfile.ZipFile(first) as archive:
        assert archive.namelist() == ["bin/tool", "data"]
        assert {item.compress_type for item in archive.infolist()} == {
            zipfile.ZIP_STORED
        }
        assert {item.date_time[-1] % 2 for item in archive.infolist()} == {0}
        assert [item.external_attr >> 16 for item in archive.infolist()] == [
            stat.S_IFREG | 0o755,
            stat.S_IFREG | 0o644,
        ]
    output = tmp_path / "extracted"
    release_archive.extract_zip_strict(first, output)
    assert (output / "bin" / "tool").read_bytes() == b"tool"
    assert (output / "data").read_bytes() == b"data"


@pytest.mark.parametrize("epoch,year", [(1, 1980), (10**20, 2107)])
def test_timestamp_is_clamped(tmp_path: Path, epoch: int, year: int) -> None:
    output = tmp_path / "archive.zip"
    release_archive.write_reproducible_zip(
        _source(tmp_path), output, source_date_epoch=epoch
    )
    with zipfile.ZipFile(output) as archive:
        assert {member.date_time[0] for member in archive.infolist()} == {year}


def test_prefix_and_mode_resolver_have_one_authority(tmp_path: Path) -> None:
    output = tmp_path / "archive.zip"
    release_archive.write_reproducible_zip(
        _source(tmp_path),
        output,
        source_date_epoch=EPOCH,
        prefix="bundle",
        mode_resolver=lambda _relative: 0o644,
    )
    with zipfile.ZipFile(output) as archive:
        assert archive.namelist() == ["bundle/bin/tool", "bundle/data"]
        assert {member.external_attr >> 16 for member in archive.infolist()} == {
            stat.S_IFREG | 0o644
        }


@pytest.mark.parametrize(
    "names",
    [
        ("A.txt", "a.txt"),
        (
            "caf\N{LATIN SMALL LETTER E WITH ACUTE}.txt",
            "cafe\N{COMBINING ACUTE ACCENT}.txt",
        ),
        ("parent", "parent/child"),
        ("parent/child", "parent"),
        ("A/one", "a/two"),
        ("A/one", "a/"),
        ("A/", "a/one"),
        ("same", "same"),
    ],
)
def test_extraction_rejects_collisions_including_implicit_parents(
    tmp_path: Path, names: tuple[str, ...]
) -> None:
    archive = _zip(tmp_path / "archive.zip", names)
    with pytest.raises(ValueError, match="collide|parent directory"):
        release_archive.extract_zip_strict(archive, tmp_path / "out")
    assert not (tmp_path / "out").exists()


@pytest.mark.parametrize(
    "name",
    [
        "CON",
        "nul.txt",
        "dir/COM1.log",
        "CONIN$",
        "conout$.txt",
        "COM¹.log",
        "bad?.txt",
        "bad*.txt",
        'bad".txt',
        "bad<.txt",
        "bad>.txt",
        "bad|.txt",
        "bad\\path",
        "trailing.",
        "trailing ",
        "./file",
        "a//file",
        "a/../file",
        "../escape",
        "/absolute",
        "C:/drive",
    ],
)
def test_extraction_rejects_nonportable_paths(tmp_path: Path, name: str) -> None:
    archive = _zip(tmp_path / "archive.zip", (name,))
    with pytest.raises(ValueError, match="portable|escapes"):
        release_archive.extract_zip_strict(archive, tmp_path / "out")


def test_explicit_directory_after_implicit_directory_is_valid(tmp_path: Path) -> None:
    archive = tmp_path / "archive.zip"
    with zipfile.ZipFile(archive, "w") as stream:
        stream.writestr("parent/child", b"data")
        stream.writestr("parent/", b"")
        stream.writestr("empty/", b"")
    release_archive.extract_zip_strict(archive, tmp_path / "out")
    assert (tmp_path / "out/parent/child").read_bytes() == b"data"
    assert (tmp_path / "out/empty").is_dir()


@pytest.mark.parametrize(
    "file_type,dos",
    [
        (stat.S_IFLNK, 0),
        (stat.S_IFIFO, 0),
        (stat.S_IFREG, 0x400),
    ],
)
def test_extraction_rejects_special_members(
    tmp_path: Path, file_type: int, dos: int
) -> None:
    path = tmp_path / "archive.zip"
    member = zipfile.ZipInfo("node")
    member.create_system = 3
    member.external_attr = ((file_type | 0o644) << 16) | dos
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr(member, b"target")
    with pytest.raises(ValueError, match="symbolic link or special"):
        release_archive.extract_zip_strict(path, tmp_path / "out")


@pytest.mark.parametrize(
    "policy,message",
    [
        (release_archive.ArchivePolicy(max_members=1), "member-count"),
        (release_archive.ArchivePolicy(max_archive_bytes=1), "compressed-size"),
        (release_archive.ArchivePolicy(max_file_bytes=64), "member exceeds size"),
        (release_archive.ArchivePolicy(max_total_bytes=100), "total uncompressed"),
        (release_archive.ArchivePolicy(max_compression_ratio=2), "compression-ratio"),
    ],
)
def test_resource_policy_bounds_extraction(
    tmp_path: Path, policy: release_archive.ArchivePolicy, message: str
) -> None:
    path = tmp_path / "archive.zip"
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("first", b"0" * 128)
        archive.writestr("second", b"1")
    with pytest.raises(ValueError, match=message):
        release_archive.extract_zip_strict(path, tmp_path / "out", policy=policy)
    assert not (tmp_path / "out").exists()


@pytest.mark.parametrize("ratio", [True, float("nan"), float("inf"), 0.5, "two"])
def test_policy_rejects_invalid_ratio(ratio: object) -> None:
    with pytest.raises(ValueError, match="ratio limit"):
        release_archive.ArchivePolicy(max_compression_ratio=ratio)  # type: ignore[arg-type]


def test_zip_parser_allocation_is_bounded_before_open(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = _zip(tmp_path / "archive.zip")
    data = bytearray(path.read_bytes())
    struct.pack_into(
        "<L", data, len(data) - 10, release_archive._MAX_METADATA_BYTES + 1
    )
    path.write_bytes(data)

    def unexpected_open(*_args, **_kwargs):
        pytest.fail("unbounded central directory reached ZipFile")

    monkeypatch.setattr(release_archive.zipfile, "ZipFile", unexpected_open)
    with pytest.raises(ValueError, match="metadata-size"):
        release_archive.extract_zip_strict(path, tmp_path / "out")


def test_creation_is_bounded_before_publication(tmp_path: Path) -> None:
    output = tmp_path / "archive.zip"
    with pytest.raises(ValueError, match="compressed-size"):
        release_archive.write_reproducible_zip(
            _source(tmp_path),
            output,
            source_date_epoch=EPOCH,
            policy=release_archive.ArchivePolicy(max_archive_bytes=25),
        )
    assert not output.exists()


def test_creation_rejects_output_inside_source(tmp_path: Path) -> None:
    root = _source(tmp_path)
    with pytest.raises(ValueError, match="outside the source tree"):
        release_archive.write_reproducible_zip(
            root, root / "out.zip", source_date_epoch=EPOCH
        )


@pytest.mark.parametrize("operation", ["create", "extract"])
def test_publication_race_preserves_foreign_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, operation: str
) -> None:
    output = tmp_path / "output"
    if operation == "create":
        original = release_archive.durable_publish_exclusive

        def race(stage: Path, destination: Path) -> None:
            destination.write_bytes(b"foreign")
            original(stage, destination)

        monkeypatch.setattr(release_archive, "durable_publish_exclusive", race)
        with pytest.raises(FileExistsError):
            release_archive.write_reproducible_zip(
                _source(tmp_path), output, source_date_epoch=EPOCH
            )
        assert output.read_bytes() == b"foreign"
    else:
        original = release_archive.durable_publish_directory_exclusive

        def race(stage: Path, destination: Path) -> None:
            destination.mkdir()
            (destination / "foreign").write_bytes(b"foreign")
            original(stage, destination)

        monkeypatch.setattr(
            release_archive, "durable_publish_directory_exclusive", race
        )
        with pytest.raises(FileExistsError):
            release_archive.extract_zip_strict(_zip(tmp_path / "archive.zip"), output)
        assert (output / "foreign").read_bytes() == b"foreign"
    assert not list(tmp_path.glob(".molt-*-*"))


def test_source_mutation_with_restored_mtime_fails_closed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _source(tmp_path)
    original = release_archive._copy_exact

    def mutate(source, sink, *, expected_size):
        digest = original(source, sink, expected_size=expected_size)
        path = root / "bin/tool"
        metadata = path.stat()
        path.write_bytes(b"evil")
        os.utime(path, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
        return digest

    monkeypatch.setattr(release_archive, "_copy_exact", mutate)
    with pytest.raises(ValueError, match="changed"):
        release_archive.write_reproducible_zip(
            root, tmp_path / "out.zip", source_date_epoch=EPOCH
        )
    assert not (tmp_path / "out.zip").exists()


def test_extracted_destination_is_rehashed_before_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    original = release_archive._inventory_regular_files

    def mutate(root, **kwargs):
        (root / "file").write_bytes(b"changed")
        return original(root, **kwargs)

    monkeypatch.setattr(release_archive, "_inventory_regular_files", mutate)
    with pytest.raises(ValueError, match="tree changed"):
        release_archive.extract_zip_strict(
            _zip(tmp_path / "archive.zip"), tmp_path / "out"
        )
    assert not (tmp_path / "out").exists()


def test_archive_mutation_during_extraction_fails_closed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = _zip(tmp_path / "archive.zip")
    original = release_archive._copy_exact

    def mutate(source, sink, *, expected_size):
        digest = original(source, sink, expected_size=expected_size)
        metadata = path.stat()
        with path.open("r+b") as stream:
            stream.seek(-1, os.SEEK_END)
            stream.write(b"!")
        os.utime(path, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
        return digest

    monkeypatch.setattr(release_archive, "_copy_exact", mutate)
    with pytest.raises(ValueError, match="changed"):
        release_archive.extract_zip_strict(path, tmp_path / "out")
    assert not (tmp_path / "out").exists()


def test_replaced_stage_is_not_removed(tmp_path: Path) -> None:
    stage = tmp_path / "stage"
    stage.mkdir()
    identity = release_archive._directory_identity(stage)
    stage.rename(tmp_path / "original")
    stage.mkdir()
    (stage / "foreign").write_bytes(b"foreign")
    release_archive._discard_stage(stage, identity)
    assert (stage / "foreign").read_bytes() == b"foreign"


def test_byte_comparison_rejects_in_read_mutation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    first, second = tmp_path / "first", tmp_path / "second"
    first.write_bytes(b"same")
    second.write_bytes(b"same")
    original = release_archive._copy_exact

    def mutate(source, sink, *, expected_size):
        digest = original(source, sink, expected_size=expected_size)
        metadata = first.stat()
        first.write_bytes(b"evil")
        os.utime(first, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
        return digest

    monkeypatch.setattr(release_archive, "_copy_exact", mutate)
    with pytest.raises(ValueError, match="changed"):
        release_archive.same_regular_file_bytes(first, second)


def test_prefixed_creation_counts_implicit_prefix(tmp_path: Path) -> None:
    source = tmp_path / "source"
    source.mkdir()
    (source / "file").write_bytes(b"data")
    with pytest.raises(ValueError, match="member-count"):
        release_archive.write_reproducible_zip(
            source,
            tmp_path / "out.zip",
            source_date_epoch=EPOCH,
            prefix="root",
            policy=release_archive.ArchivePolicy(max_members=1),
        )


@pytest.mark.parametrize("target", ["source", "archive", "output-parent"])
def test_indirect_paths_are_rejected(tmp_path: Path, target: str) -> None:
    root = _source(tmp_path)
    archive = _zip(tmp_path / "archive.zip")
    link = tmp_path / "link"
    linked = root if target != "archive" else archive
    try:
        link.symlink_to(linked, target_is_directory=target != "archive")
    except OSError:
        pytest.skip("host does not permit symbolic links")
    with pytest.raises(ValueError, match="link|junction"):
        if target == "source":
            release_archive.write_reproducible_zip(
                link, tmp_path / "out.zip", source_date_epoch=EPOCH
            )
        elif target == "archive":
            release_archive.extract_zip_strict(link, tmp_path / "out")
        else:
            release_archive.extract_zip_strict(archive, link / "out")


def test_clean_consumer_uses_strict_zip_authority(tmp_path: Path) -> None:
    archive = _zip(tmp_path / "archive.zip", ("A/first", "a/second"))
    with pytest.raises(ValueError, match="collide"):
        verify_consumer._extract(archive, tmp_path / "out")


def test_inventory_accepts_linux_style_read_atime_advance(tmp_path: Path) -> None:
    sources, _ = release_archive._inventory_regular_files(
        _source(tmp_path), policy=release_archive.DEFAULT_ARCHIVE_POLICY
    )
    source = sources[0]
    with open_stable_regular_file(source.path, label="test source") as opened:
        fields = list(opened.stat)
        fields[7] += 60  # st_atime: simulate Linux relatime independently of host.
        accessed = os.stat_result(
            fields,
            {
                "st_atime_ns": opened.stat.st_atime_ns + 60_000_000_000,
                "st_mtime_ns": opened.stat.st_mtime_ns,
                "st_ctime_ns": opened.stat.st_ctime_ns,
            },
        )
        assert accessed.st_atime != source.metadata.st_atime
        source.check(replace(opened, stat=accessed))
        with pytest.raises(ValueError, match="changed since inventory"):
            source.check(
                replace(
                    opened,
                    stat=accessed,
                    content_change_time_ns=source.content_change_time_ns + 1,
                )
            )
