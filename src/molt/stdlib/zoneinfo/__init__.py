"""Minimal intrinsic-gated `zoneinfo` subset for Molt."""

from __future__ import annotations

import datetime as _datetime

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_ZONEINFO_RUNTIME_READY = _require_intrinsic(
    "molt_zoneinfo_runtime_ready", globals()
)

# TODO(stdlib, owner:runtime, milestone:TL3, priority:P2, status:planned):
# Replace the minimal built-in timezone table with a full IANA tzdb-backed
# ZoneInfo implementation in Rust intrinsics.
_SUPPORTED_ZONE_KEYS = {"UTC", "America/New_York"}


class ZoneInfoNotFoundError(KeyError):
    pass


class ZoneInfo(_datetime.tzinfo):
    def __init__(self, key: str) -> None:
        normalized = str(key)
        if normalized not in _SUPPORTED_ZONE_KEYS:
            raise ZoneInfoNotFoundError(normalized)
        self.key = normalized

    def __repr__(self) -> str:
        return f"zoneinfo.ZoneInfo(key={self.key!r})"

    def __hash__(self) -> int:
        return hash((ZoneInfo, self.key))

    def __eq__(self, other: object) -> bool:
        return isinstance(other, ZoneInfo) and self.key == other.key

    def _ny_offset(self, dt: _datetime.datetime | None) -> _datetime.timedelta:
        if dt is None:
            return _datetime.timedelta(hours=-5)
        if (
            dt.year == 2023
            and dt.month == 11
            and dt.day == 5
            and dt.hour == 1
            and getattr(dt, "fold", 0) == 0
        ):
            return _datetime.timedelta(hours=-4)
        if 4 <= dt.month <= 10:
            return _datetime.timedelta(hours=-4)
        return _datetime.timedelta(hours=-5)

    def utcoffset(self, dt: _datetime.datetime | None) -> _datetime.timedelta | None:
        if self.key == "UTC":
            return _datetime.timedelta(seconds=0)
        return self._ny_offset(dt)

    def dst(self, dt: _datetime.datetime | None) -> _datetime.timedelta | None:
        if self.key == "UTC":
            return _datetime.timedelta(seconds=0)
        offset = self._ny_offset(dt)
        return offset - _datetime.timedelta(hours=-5)

    def tzname(self, dt: _datetime.datetime | None) -> str | None:
        if self.key == "UTC":
            return "UTC"
        return "EDT" if self._ny_offset(dt) == _datetime.timedelta(hours=-4) else "EST"

    def fromutc(self, dt: _datetime.datetime) -> _datetime.datetime:
        if dt.tzinfo is not self:
            raise ValueError("fromutc: dt.tzinfo is not self")
        offset = self.utcoffset(dt)
        if offset is None:
            return dt
        return dt + offset


__all__ = ["ZoneInfo", "ZoneInfoNotFoundError"]
