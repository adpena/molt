from __future__ import annotations

import io
import tarfile

import pytest

from molt.ustar import RegularUstarTarInfo


@pytest.mark.parametrize(
    "kind",
    [
        tarfile.XHDTYPE,
        tarfile.XGLTYPE,
        tarfile.SOLARIS_XHDTYPE,
        tarfile.GNUTYPE_LONGNAME,
        tarfile.GNUTYPE_LONGLINK,
        tarfile.GNUTYPE_SPARSE,
    ],
)
@pytest.mark.parametrize("after_regular_member", [False, True])
def test_regular_ustar_rejects_extensions_before_payload_reads(
    kind: bytes, after_regular_member: bool
) -> None:
    header = tarfile.TarInfo("extension")
    header.type = kind
    header.size = 2 * 1024 * 1024 * 1024

    class HeaderOnlyStream(io.BytesIO):
        def read(self, size: int = -1) -> bytes:
            assert 0 <= size <= tarfile.BLOCKSIZE, "extension payload must not be read"
            return super().read(size)

    prefix = (
        tarfile.TarInfo("regular").tobuf(format=tarfile.USTAR_FORMAT)
        if after_regular_member
        else b""
    )
    source = HeaderOnlyStream(prefix + header.tobuf(format=tarfile.USTAR_FORMAT))
    with pytest.raises(tarfile.ReadError, match="not a regular file"):
        with tarfile.open(
            fileobj=source, mode="r:", tarinfo=RegularUstarTarInfo
        ) as archive:
            list(archive)


@pytest.mark.parametrize("after_regular_member", [False, True])
def test_regular_ustar_rejects_noncanonical_headers_at_every_offset(
    after_regular_member: bool,
) -> None:
    prefix = (
        tarfile.TarInfo("regular").tobuf(format=tarfile.USTAR_FORMAT)
        if after_regular_member
        else b""
    )
    source = io.BytesIO(
        prefix + tarfile.TarInfo("gnu").tobuf(format=tarfile.GNU_FORMAT)
    )
    with pytest.raises(tarfile.ReadError, match="not canonical USTAR"):
        with tarfile.open(
            fileobj=source, mode="r:", tarinfo=RegularUstarTarInfo
        ) as archive:
            list(archive)


def test_regular_ustar_owns_archive_header_admission(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source = io.BytesIO()
    payloads = {"first": b"first payload", "second": b"second payload"}
    with tarfile.open(fileobj=source, mode="w", format=tarfile.USTAR_FORMAT) as writer:
        for name, payload in payloads.items():
            header = tarfile.TarInfo(name)
            header.size = len(payload)
            writer.addfile(header, io.BytesIO(payload))
    source.seek(0)

    def bypass_admission(
        cls: type[tarfile.TarInfo], archive: tarfile.TarFile
    ) -> tarfile.TarInfo:
        raise AssertionError("base archive decoder must not bypass header admission")

    monkeypatch.setattr(tarfile.TarInfo, "fromtarfile", classmethod(bypass_admission))
    with tarfile.open(
        fileobj=source, mode="r:", tarinfo=RegularUstarTarInfo
    ) as archive:
        members = list(archive)
        assert [member.name for member in members] == list(payloads)
        for member in members:
            assert isinstance(member, RegularUstarTarInfo)
            stream = archive.extractfile(member)
            assert stream is not None
            assert stream.read() == payloads[member.name]


def test_regular_ustar_frombuf_accepts_inherited_bytearray_contract() -> None:
    raw = bytearray(tarfile.TarInfo("regular").tobuf(format=tarfile.USTAR_FORMAT))
    member = RegularUstarTarInfo.frombuf(raw, "utf-8", "strict")
    assert isinstance(member, RegularUstarTarInfo)
    assert member.name == "regular"
