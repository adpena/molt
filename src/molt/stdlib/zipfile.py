"""Minimal zipfile implementation (store + deflate)."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): parity.

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


__all__ = [
    "BadZipFile",
    "ZipFile",
    "ZipFileError",
    "ZipInfo",
    "ZIP_DEFLATED",
    "ZIP_STORED",
]


class ZipFileError(Exception):
    pass


class BadZipFile(ZipFileError):
    pass


ZIP_STORED = 0
ZIP_DEFLATED = 8
ZIP64_LIMIT = 0xFFFFFFFF
ZIP64_COUNT_LIMIT = 0xFFFF
ZIP64_EXTRA_ID = 0x0001

_LOCAL_SIG = b"PK\x03\x04"
_CENTRAL_SIG = b"PK\x01\x02"
_EOCD_SIG = b"PK\x05\x06"
_ZIP64_EOCD_SIG = b"PK\x06\x06"
_ZIP64_LOCATOR_SIG = b"PK\x06\x07"


class ZipInfo:
    def __init__(self, filename: str) -> None:
        self.filename = filename
        self.date_time = (1980, 1, 1, 0, 0, 0)
        self.compress_type = ZIP_STORED


_Entry = tuple[bytes, int, int, int, int, int]
_IndexEntry = tuple[int, int, int, int, int]


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
    table = _CRC_TABLE[0]
    if table is None:
        table = _build_crc_table()
        _CRC_TABLE[0] = table
    crc = 0xFFFFFFFF
    for value in data:
        crc = (crc >> 8) ^ table[(crc ^ value) & 0xFF]
    return crc ^ 0xFFFFFFFF


def _build_crc_table():
    table = [0] * 256
    for idx in range(256):
        crc = idx
        for _ in range(8):
            if crc & 1:
                crc = (crc >> 1) ^ 0xEDB88320
            else:
                crc >>= 1
        table[idx] = crc
    return table


_CRC_TABLE = [None]


class ZipFile:
    def __init__(
        self,
        file: str,
        mode: str = "r",
        compression: int = ZIP_STORED,
        compresslevel: int | None = None,
    ) -> None:
        from molt import capabilities

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
        if compression not in {ZIP_STORED, ZIP_DEFLATED}:
            raise NotImplementedError("unsupported compression method")
        if mode == "w":
            if not capabilities.trusted():
                capabilities.require("fs.write")
            self._fp = open(file, "wb")
        else:
            if not capabilities.trusted():
                capabilities.require("fs.read")
            with open(file, "rb") as handle:
                data = handle.read()
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
        if method not in {ZIP_STORED, ZIP_DEFLATED}:
            raise NotImplementedError("unsupported compression method")
        name_bytes = name.encode("utf-8")
        crc = _crc32(raw_data)
        if method == ZIP_DEFLATED:
            level = self._compresslevel if compresslevel is None else compresslevel
            data = _deflate_raw(raw_data, level)
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
            return [entry[0].decode("utf-8") for entry in self._entries]
        return list(self._index.keys())

    def read(self, name: str) -> bytes:
        if self.mode != "r":
            raise ValueError("read requires mode='r'")
        if self._data is None:
            raise BadZipFile("missing zip data")
        entry = self._index.get(name)
        if entry is None:
            raise KeyError(name)
        offset, comp_size, comp_method, name_len, _uncomp_size = entry
        header_offset = offset
        if self._data[header_offset : header_offset + 4] != _LOCAL_SIG:
            raise BadZipFile("invalid local header signature")
        extra_len = _read_u16(self._data, header_offset + 28)
        data_start = header_offset + 30 + name_len + extra_len
        payload = self._data[data_start : data_start + comp_size]
        if comp_method == ZIP_STORED:
            return payload
        if comp_method == ZIP_DEFLATED:
            try:
                return _inflate_raw(payload)
            except ValueError as exc:
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
    if len(data) < 22:
        raise BadZipFile("file is not a zip file")
    eocd_offset = _find_eocd(data)
    if eocd_offset < 0:
        raise BadZipFile("end of central directory not found")
    cd_size = _read_u32(data, eocd_offset + 12)
    cd_offset = _read_u32(data, eocd_offset + 16)
    total_entries = _read_u16(data, eocd_offset + 10)
    if (
        total_entries == ZIP64_COUNT_LIMIT
        or cd_size == ZIP64_LIMIT
        or cd_offset == ZIP64_LIMIT
    ):
        cd_offset, cd_size = _read_zip64_eocd(data, eocd_offset)
    pos = cd_offset
    end = cd_offset + cd_size
    index: dict[str, _IndexEntry] = {}
    while pos + 46 <= end:
        if data[pos : pos + 4] != _CENTRAL_SIG:
            break
        comp_method = _read_u16(data, pos + 10)
        comp_size = _read_u32(data, pos + 20)
        uncomp_size = _read_u32(data, pos + 24)
        name_len = _read_u16(data, pos + 28)
        extra_len = _read_u16(data, pos + 30)
        comment_len = _read_u16(data, pos + 32)
        local_offset = _read_u32(data, pos + 42)
        name_start = pos + 46
        name_bytes = data[name_start : name_start + name_len]
        try:
            name = name_bytes.decode("utf-8")
        except Exception:
            name = name_bytes.decode("utf-8", errors="replace")
        extra_start = name_start + name_len
        extra = data[extra_start : extra_start + extra_len]
        if (
            comp_size == ZIP64_LIMIT
            or uncomp_size == ZIP64_LIMIT
            or local_offset == ZIP64_LIMIT
        ):
            comp_size, uncomp_size, local_offset = _parse_zip64_extra(
                extra,
                comp_size,
                uncomp_size,
                local_offset,
            )
        index[name] = (local_offset, comp_size, comp_method, name_len, uncomp_size)
        pos = name_start + name_len + extra_len + comment_len
    return index


def _find_eocd(data: bytes) -> int:
    max_comment = 65535
    start = max(0, len(data) - (22 + max_comment))
    return data.rfind(_EOCD_SIG, start)


def _read_zip64_eocd(data: bytes, eocd_offset: int) -> tuple[int, int]:
    locator_offset = eocd_offset - 20
    if locator_offset < 0:
        raise BadZipFile("zip64 locator missing")
    if data[locator_offset : locator_offset + 4] != _ZIP64_LOCATOR_SIG:
        raise BadZipFile("zip64 locator missing")
    zip64_eocd_offset = _read_u64(data, locator_offset + 8)
    if data[zip64_eocd_offset : zip64_eocd_offset + 4] != _ZIP64_EOCD_SIG:
        raise BadZipFile("zip64 eocd missing")
    cd_size = _read_u64(data, zip64_eocd_offset + 40)
    cd_offset = _read_u64(data, zip64_eocd_offset + 48)
    return cd_offset, cd_size


def _parse_zip64_extra(
    extra: bytes,
    comp_size: int,
    uncomp_size: int,
    local_offset: int,
) -> tuple[int, int, int]:
    pos = 0
    while pos + 4 <= len(extra):
        header_id = _read_u16(extra, pos)
        data_size = _read_u16(extra, pos + 2)
        pos += 4
        if pos + data_size > len(extra):
            break
        if header_id == ZIP64_EXTRA_ID:
            cursor = pos
            if uncomp_size == ZIP64_LIMIT:
                if cursor + 8 > pos + data_size:
                    raise BadZipFile("zip64 extra missing size")
                uncomp_size = _read_u64(extra, cursor)
                cursor += 8
            if comp_size == ZIP64_LIMIT:
                if cursor + 8 > pos + data_size:
                    raise BadZipFile("zip64 extra missing comp size")
                comp_size = _read_u64(extra, cursor)
                cursor += 8
            if local_offset == ZIP64_LIMIT:
                if cursor + 8 > pos + data_size:
                    raise BadZipFile("zip64 extra missing offset")
                local_offset = _read_u64(extra, cursor)
            return comp_size, uncomp_size, local_offset
        pos += data_size
    raise BadZipFile("zip64 extra missing")


def _zip64_extra(size: int, comp_size: int, offset: int | None) -> bytes:
    data = bytearray()
    data.extend(_u64(size))
    data.extend(_u64(comp_size))
    if offset is not None:
        data.extend(_u64(offset))
    return _u16(ZIP64_EXTRA_ID) + _u16(len(data)) + data


_MOLT_DEFLATE_RAW = _require_intrinsic("molt_deflate_raw", globals())
_MOLT_INFLATE_RAW = _require_intrinsic("molt_inflate_raw", globals())


def _deflate_raw(data: bytes, level: int | None) -> bytes:
    return _MOLT_DEFLATE_RAW(data, level)


def _inflate_raw(data: bytes) -> bytes:
    return _MOLT_INFLATE_RAW(data)
