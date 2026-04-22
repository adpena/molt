"""Rust-intrinsic-backed zoneinfo for Molt."""

from __future__ import annotations

import datetime as _datetime

from _intrinsics import require_intrinsic as _require_intrinsic

_molt_zoneinfo_runtime_ready = _require_intrinsic("molt_zoneinfo_runtime_ready")
_molt_zoneinfo_new = _require_intrinsic("molt_zoneinfo_new")
_molt_zoneinfo_drop = _require_intrinsic("molt_zoneinfo_drop")
_molt_zoneinfo_key = _require_intrinsic("molt_zoneinfo_key")
_molt_zoneinfo_utcoffset = _require_intrinsic("molt_zoneinfo_utcoffset")
_molt_zoneinfo_dst = _require_intrinsic("molt_zoneinfo_dst")
_molt_zoneinfo_tzname = _require_intrinsic("molt_zoneinfo_tzname")
_molt_zoneinfo_available_timezones = _require_intrinsic(
    "molt_zoneinfo_available_timezones"
)


class ZoneInfoNotFoundError(KeyError):
    pass


def _dt_to_components(dt: _datetime.datetime | None) -> tuple[int, ...] | None:
    if dt is None:
        return None
    return (dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second, dt.fold)


class ZoneInfo(_datetime.tzinfo):
    def __init__(self, key: str) -> None:
        self._handle = _molt_zoneinfo_new(str(key))

    @property
    def key(self) -> str:
        return str(_molt_zoneinfo_key(self._handle))

    def __repr__(self) -> str:
        return f"zoneinfo.ZoneInfo(key={self.key!r})"

    def __hash__(self) -> int:
        return hash((ZoneInfo, self.key))

    def __eq__(self, other: object) -> bool:
        return isinstance(other, ZoneInfo) and self.key == other.key

    def utcoffset(self, dt: _datetime.datetime | None) -> _datetime.timedelta | None:
        secs = int(_molt_zoneinfo_utcoffset(self._handle, _dt_to_components(dt)))
        return _datetime.timedelta(seconds=secs)

    def dst(self, dt: _datetime.datetime | None) -> _datetime.timedelta | None:
        secs = int(_molt_zoneinfo_dst(self._handle, _dt_to_components(dt)))
        return _datetime.timedelta(seconds=secs)

    def tzname(self, dt: _datetime.datetime | None) -> str | None:
        return str(_molt_zoneinfo_tzname(self._handle, _dt_to_components(dt)))

    def fromutc(self, dt: _datetime.datetime) -> _datetime.datetime:
        if dt.tzinfo is not self:
            raise ValueError("fromutc: dt.tzinfo is not self")
        offset = self.utcoffset(dt)
        if offset is None:
            return dt
        return dt + offset

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _molt_zoneinfo_drop(handle)
            except Exception:
                pass


def available_timezones() -> set[str]:
    return _molt_zoneinfo_available_timezones()


__all__ = ["ZoneInfo", "ZoneInfoNotFoundError", "available_timezones"]

globals().pop("_require_intrinsic", None)
