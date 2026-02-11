"""Minimal `wsgiref.headers` subset for Molt."""

from __future__ import annotations

from collections.abc import Iterable, Iterator

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_WSGIREF_RUNTIME_READY = _require_intrinsic(
    "molt_wsgiref_runtime_ready", globals()
)


class Headers:
    def __init__(self, headers: Iterable[tuple[str, str]] | None = None) -> None:
        self._headers: list[tuple[str, str]] = []
        if headers is not None:
            for key, value in headers:
                self._headers.append((str(key), str(value)))

    def add_header(self, _name: str, _value: str, **_params: str) -> None:
        value = str(_value)
        if _params:
            suffix = "; ".join(f"{key}={val}" for key, val in _params.items())
            if suffix:
                value = f"{value}; {suffix}"
        self._headers.append((str(_name), value))

    def __iter__(self) -> Iterator[tuple[str, str]]:
        return iter(self._headers)

    def __len__(self) -> int:
        return len(self._headers)

    def __getitem__(self, key: str) -> str:
        needle = str(key).lower()
        for header_name, header_value in reversed(self._headers):
            if header_name.lower() == needle:
                return header_value
        raise KeyError(key)

    def __setitem__(self, key: str, value: str) -> None:
        needle = str(key).lower()
        self._headers = [
            (header_name, header_value)
            for header_name, header_value in self._headers
            if header_name.lower() != needle
        ]
        self._headers.append((str(key), str(value)))

    def __delitem__(self, key: str) -> None:
        needle = str(key).lower()
        new_headers = [
            (header_name, header_value)
            for header_name, header_value in self._headers
            if header_name.lower() != needle
        ]
        if len(new_headers) == len(self._headers):
            raise KeyError(key)
        self._headers = new_headers

    def __str__(self) -> str:
        lines = [
            f"{header_name}: {header_value}\r\n"
            for header_name, header_value in self._headers
        ]
        return "".join(lines) + "\r\n"


__all__ = ["Headers"]
