"""zipfile implementation: STORED, DEFLATED, BZIP2, LZMA.

The deflate / inflate path uses the molt_deflate_raw / molt_inflate_raw
runtime intrinsics. BZIP2 and LZMA delegate to the public `bz2` and
`lzma` modules, which themselves wrap molt_bz2_* and molt_lzma_*
runtime intrinsics. No fallback Python implementations are introduced.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


__all__ = [
    "BadZipFile",
    "Path",
    "ZipFile",
    "ZipFileError",
    "ZipInfo",
    "ZIP_DEFLATED",
    "ZIP_STORED",
    "ZIP_BZIP2",
    "ZIP_LZMA",
]


class ZipFileError(Exception):
    pass


class BadZipFile(ZipFileError):
    pass


ZIP_STORED = 0
ZIP_DEFLATED = 8
ZIP_BZIP2 = 12
ZIP_LZMA = 14
ZIP64_LIMIT = 0xFFFFFFFF

_SUPPORTED_COMPRESSION = frozenset({ZIP_STORED, ZIP_DEFLATED, ZIP_BZIP2, ZIP_LZMA})
ZIP64_COUNT_LIMIT = 0xFFFF
ZIP64_EXTRA_ID = 0x0001

_LOCAL_SIG = b"PK\x03\x04"
_CENTRAL_SIG = b"PK\x01\x02"
_EOCD_SIG = b"PK\x05\x06"
_ZIP64_EOCD_SIG = b"PK\x06\x06"
_ZIP64_LOCATOR_SIG = b"PK\x06\x07"

_MOLT_CAPABILITIES_TRUSTED = _require_intrinsic("molt_capabilities_trusted")
_MOLT_CAPABILITIES_REQUIRE = _require_intrinsic("molt_capabilities_require")
_MOLT_ZIPFILE_CRC32 = _require_intrinsic("molt_zipfile_crc32")
_MOLT_ZIPFILE_PARSE_CENTRAL_DIRECTORY = _require_intrinsic(
    "molt_zipfile_parse_central_directory"
)
_MOLT_ZIPFILE_BUILD_ZIP64_EXTRA = _require_intrinsic("molt_zipfile_build_zip64_extra")


class ZipInfo:
    def __init__(self, filename: str) -> None:
        self.filename = filename
        self.date_time = (1980, 1, 1, 0, 0, 0)
        self.compress_type = ZIP_STORED


_Entry = tuple[bytes, int, int, int, int, int]
_IndexEntry = tuple[int, int, int, int, int]


def _require_capability(name: str) -> None:
    if _MOLT_CAPABILITIES_TRUSTED():
        return
    _MOLT_CAPABILITIES_REQUIRE(name)


def _u16(value: int) -> bytes:
    return int(value).to_bytes(2, "little", signed=False)


def _u32(value: int) -> bytes:
    return int(value).to_bytes(4, "little", signed=False)


def _u64(value: int) -> bytes:
    return int(value).to_bytes(8, "little", signed=False)


def _read_u16(blob: bytes, offset: int) -> int:
    return int.from_bytes(blob[offset : offset + 2], "little", signed=False)


def _read_u32(blob: bytes, offset: int) -> int:
    return int.from_bytes(blob[offset : offset + 4], "little", signed=False)


def _read_u64(blob: bytes, offset: int) -> int:
    return int.from_bytes(blob[offset : offset + 8], "little", signed=False)


def _crc32(data: bytes) -> int:
    return int(_MOLT_ZIPFILE_CRC32(data))


class ZipFile:
    def __init__(
        self,
        file: str,
        mode: str = "r",
        compression: int = ZIP_STORED,
        compresslevel: int | None = None,
    ) -> None:
        self.filename = file
        self.mode = mode
        self._fp = None
        self._entries: list[_Entry] = []
        self._index: dict[str, _IndexEntry] = {}
        self._data: bytes | None = None
        self._compression = compression
        self._compresslevel = compresslevel

        if mode not in {"r", "w"}:
            raise ValueError("zipfile mode must be 'r' or 'w'")
        if compression not in _SUPPORTED_COMPRESSION:
            raise NotImplementedError("unsupported compression method")
        if mode == "w":
            _require_capability("fs.write")
            self._fp = open(file, "wb")
        else:
            _require_capability("fs.read")
            handle = open(file, "rb")
            data = handle.read()
            handle.close()
            self._data = data
            self._index = _parse_central_directory(data)

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        self.close()
        return False

    def writestr(
        self,
        name: str | ZipInfo,
        data,
        compress_type: int | None = None,
        compresslevel: int | None = None,
    ) -> None:
        if self.mode != "w" or self._fp is None:
            raise ValueError("writestr requires mode='w'")
        info = None
        if isinstance(name, ZipInfo):
            info = name
            name = info.filename
        if isinstance(data, str):
            data = data.encode("utf-8")
        if not isinstance(data, (bytes, bytearray)):
            raise TypeError("data must be bytes or str")
        raw_data = bytes(data)
        method = compress_type
        if method is None and info is not None:
            method = info.compress_type
        if method is None:
            method = self._compression
        if method not in _SUPPORTED_COMPRESSION:
            raise NotImplementedError("unsupported compression method")
        name_bytes = name.encode("utf-8")
        crc = _crc32(raw_data)
        if method == ZIP_DEFLATED:
            level = self._compresslevel if compresslevel is None else compresslevel
            data = _deflate_raw(raw_data, level)
        elif method == ZIP_BZIP2:
            import bz2 as _bz2

            level = self._compresslevel if compresslevel is None else compresslevel
            data = _bz2.compress(raw_data, level if level is not None else 9)
        elif method == ZIP_LZMA:
            import lzma as _lzma

            data = _lzma.compress(raw_data)
        else:
            data = raw_data
        comp_size = len(data)
        offset = self._fp.tell()
        extra = b""
        local_size = len(raw_data)
        local_comp = comp_size
        if local_size > ZIP64_LIMIT or local_comp > ZIP64_LIMIT:
            extra = _zip64_extra(local_size, local_comp, None)
            local_size = ZIP64_LIMIT
            local_comp = ZIP64_LIMIT
        header = (
            _LOCAL_SIG
            + _u16(20)
            + _u16(0)
            + _u16(method)
            + _u16(0)
            + _u16(0)
            + _u32(crc)
            + _u32(local_comp)
            + _u32(local_size)
            + _u16(len(name_bytes))
            + _u16(len(extra))
        )
        self._fp.write(header)
        self._fp.write(name_bytes)
        if extra:
            self._fp.write(extra)
        self._fp.write(data)
        self._entries.append(
            (name_bytes, crc, comp_size, len(raw_data), method, offset)
        )

    def namelist(self) -> list[str]:
        if self.mode == "w":
            entries = getattr(self, "_entries", None)
            if not isinstance(entries, list):
                raise BadZipFile("zip writer state unavailable")
            return [entry[0].decode("utf-8") for entry in entries]
        index = getattr(self, "_index", None)
        if not isinstance(index, dict):
            data = getattr(self, "_data", None)
            if not isinstance(data, (bytes, bytearray)):
                raise BadZipFile("zip index unavailable")
            index = _parse_central_directory(bytes(data))
            self._index = index
        return list(index.keys())

    def read(self, name: str) -> bytes:
        if self.mode != "r":
            raise ValueError("read requires mode='r'")
        data = getattr(self, "_data", None)
        if not isinstance(data, (bytes, bytearray)):
            raise BadZipFile("missing zip data")
        index = getattr(self, "_index", None)
        if not isinstance(index, dict):
            index = _parse_central_directory(bytes(data))
            self._index = index
        entry = index.get(name)
        if entry is None:
            raise KeyError(name)
        offset, comp_size, comp_method, name_len, _uncomp_size = entry
        header_offset = offset
        data_bytes = bytes(data)
        if data_bytes[header_offset : header_offset + 4] != _LOCAL_SIG:
            raise BadZipFile("invalid local header signature")
        extra_len = _read_u16(data_bytes, header_offset + 28)
        data_start = header_offset + 30 + name_len + extra_len
        payload = data_bytes[data_start : data_start + comp_size]
        if comp_method == ZIP_STORED:
            return payload
        if comp_method == ZIP_DEFLATED:
            try:
                return _inflate_raw(payload)
            except ValueError as exc:
                raise BadZipFile(str(exc)) from exc
        if comp_method == ZIP_BZIP2:
            import bz2 as _bz2

            try:
                return _bz2.decompress(payload)
            except (OSError, ValueError) as exc:
                raise BadZipFile(str(exc)) from exc
        if comp_method == ZIP_LZMA:
            import lzma as _lzma

            try:
                return _lzma.decompress(payload)
            except (OSError, ValueError) as exc:
                raise BadZipFile(str(exc)) from exc
        raise NotImplementedError("unsupported compression method")

    def close(self) -> None:
        if self.mode == "w" and self._fp is not None:
            cd_start = self._fp.tell()
            cd_data = bytearray()
            for name_bytes, crc, comp_size, size, method, offset in self._entries:
                extra = b""
                cd_comp = comp_size
                cd_size = size
                cd_offset = offset
                needs_zip64 = (
                    comp_size > ZIP64_LIMIT
                    or size > ZIP64_LIMIT
                    or offset > ZIP64_LIMIT
                )
                if needs_zip64:
                    extra = _zip64_extra(size, comp_size, offset)
                    cd_comp = ZIP64_LIMIT
                    cd_size = ZIP64_LIMIT
                    cd_offset = ZIP64_LIMIT
                cd_data.extend(
                    _CENTRAL_SIG
                    + _u16(20)
                    + _u16(20)
                    + _u16(0)
                    + _u16(method)
                    + _u16(0)
                    + _u16(0)
                    + _u32(crc)
                    + _u32(cd_comp)
                    + _u32(cd_size)
                    + _u16(len(name_bytes))
                    + _u16(len(extra))
                    + _u16(0)
                    + _u16(0)
                    + _u16(0)
                    + _u32(0)
                    + _u32(cd_offset)
                )
                cd_data.extend(name_bytes)
                if extra:
                    cd_data.extend(extra)
            self._fp.write(cd_data)
            cd_size = len(cd_data)
            needs_zip64 = (
                len(self._entries) > ZIP64_COUNT_LIMIT
                or cd_size > ZIP64_LIMIT
                or cd_start > ZIP64_LIMIT
                or any(
                    entry[2] > ZIP64_LIMIT
                    or entry[3] > ZIP64_LIMIT
                    or entry[5] > ZIP64_LIMIT
                    for entry in self._entries
                )
            )
            if needs_zip64:
                zip64_eocd_offset = self._fp.tell()
                entry_count = len(self._entries)
                zip64_eocd = (
                    _ZIP64_EOCD_SIG
                    + _u64(44)
                    + _u16(45)
                    + _u16(45)
                    + _u32(0)
                    + _u32(0)
                    + _u64(entry_count)
                    + _u64(entry_count)
                    + _u64(cd_size)
                    + _u64(cd_start)
                )
                self._fp.write(zip64_eocd)
                locator = (
                    _ZIP64_LOCATOR_SIG + _u32(0) + _u64(zip64_eocd_offset) + _u32(1)
                )
                self._fp.write(locator)
                eocd = (
                    _EOCD_SIG
                    + _u16(0)
                    + _u16(0)
                    + _u16(ZIP64_COUNT_LIMIT)
                    + _u16(ZIP64_COUNT_LIMIT)
                    + _u32(ZIP64_LIMIT)
                    + _u32(ZIP64_LIMIT)
                    + _u16(0)
                )
            else:
                eocd = (
                    _EOCD_SIG
                    + _u16(0)
                    + _u16(0)
                    + _u16(len(self._entries))
                    + _u16(len(self._entries))
                    + _u32(cd_size)
                    + _u32(cd_start)
                    + _u16(0)
                )
            self._fp.write(eocd)
            self._fp.close()
            self._fp = None


def _parse_central_directory(data: bytes) -> dict[str, _IndexEntry]:
    try:
        index = _MOLT_ZIPFILE_PARSE_CENTRAL_DIRECTORY(data)
    except ValueError as exc:
        raise BadZipFile(str(exc)) from exc
    if not isinstance(index, dict):
        raise BadZipFile("zip index unavailable")
    return index


def _zip64_extra(size: int, comp_size: int, offset: int | None) -> bytes:
    return _MOLT_ZIPFILE_BUILD_ZIP64_EXTRA(size, comp_size, offset)


_MOLT_DEFLATE_RAW = _require_intrinsic("molt_deflate_raw")
_MOLT_INFLATE_RAW = _require_intrinsic("molt_inflate_raw")
_MOLT_ZIPFILE_NORMALIZE_MEMBER_PATH = _require_intrinsic(
    "molt_zipfile_normalize_member_path"
)


def _deflate_raw(data: bytes, level: int | None) -> bytes:
    return _MOLT_DEFLATE_RAW(data, level)


def _inflate_raw(data: bytes) -> bytes:
    return _MOLT_INFLATE_RAW(data)


def main(args=None):
    import argparse
    import os
    import sys

    description = "A simple command-line interface for zipfile module."
    parser = argparse.ArgumentParser(description=description)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument(
        "-l", "--list", metavar="<zipfile>", help="Show listing of a zipfile"
    )
    group.add_argument(
        "-e",
        "--extract",
        nargs=2,
        metavar=("<zipfile>", "<output_dir>"),
        help="Extract zipfile into target dir",
    )
    group.add_argument(
        "-c",
        "--create",
        nargs="+",
        metavar=("<name>", "<file>"),
        help="Create zipfile from sources",
    )
    group.add_argument(
        "-t", "--test", metavar="<zipfile>", help="Test if a zipfile is valid"
    )
    parser.add_argument(
        "--metadata-encoding",
        metavar="<encoding>",
        help="Specify encoding of member names for -l, -e and -t",
    )
    parsed = parser.parse_args(args)
    encoding = parsed.metadata_encoding

    if parsed.test is not None:
        src = parsed.test
        badfile = None
        with ZipFile(src, "r") as zf:
            for name in zf.namelist():
                try:
                    zf.read(name)
                except Exception:
                    badfile = name
                    break
        if badfile is not None:
            print(f"The following enclosed file is corrupted: {badfile!r}")
        print("Done testing")
        return

    if parsed.list is not None:
        src = parsed.list
        with ZipFile(src, "r") as zf:
            for name in zf.namelist():
                print(name)
        return

    if parsed.extract is not None:
        src, output_dir = parsed.extract
        with ZipFile(src, "r") as zf:
            for member in zf.namelist():
                normalized = _MOLT_ZIPFILE_NORMALIZE_MEMBER_PATH(member)
                if normalized is None:
                    continue
                target = os.path.join(output_dir, normalized)
                if member.endswith("/"):
                    os.makedirs(target, exist_ok=True)
                    continue
                parent = os.path.dirname(target)
                if parent:
                    os.makedirs(parent, exist_ok=True)
                with open(target, "wb") as handle:
                    handle.write(zf.read(member))
        return

    if parsed.create is None:
        return
    if encoding:
        print("Non-conforming encodings not supported with -c.", file=sys.stderr)
        sys.exit(1)

    zip_name = parsed.create.pop(0)
    files = parsed.create

    def add_to_zip(zf: ZipFile, path: str, zippath: str) -> None:
        if os.path.isfile(path):
            arcname = zippath.replace(os.sep, "/")
            with open(path, "rb") as handle:
                zf.writestr(arcname, handle.read(), ZIP_DEFLATED)
            return
        if not os.path.isdir(path):
            return
        if zippath:
            zf.writestr(zippath.replace(os.sep, "/").rstrip("/") + "/", b"")
        for name in sorted(os.listdir(path)):
            add_to_zip(zf, os.path.join(path, name), os.path.join(zippath, name))

    with ZipFile(zip_name, "w") as zf:
        for path in files:
            zippath = os.path.basename(path)
            if not zippath:
                zippath = os.path.basename(os.path.dirname(path))
            if zippath in ("", os.curdir, os.pardir):
                zippath = ""
            add_to_zip(zf, path, zippath)


from ._path import (  # noqa: E402
    Path,
    CompleteDirs,  # noqa: F401
)

globals().pop("_require_intrinsic", None)
