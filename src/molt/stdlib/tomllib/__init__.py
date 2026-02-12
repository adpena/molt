"""Minimal `tomllib` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_TOMLLIB_RUNTIME_READY = _require_intrinsic(
    "molt_tomllib_runtime_ready", globals()
)


class TOMLDecodeError(ValueError):
    def __init__(
        self,
        message: str,
        doc: str | None = None,
        pos: int | None = None,
    ) -> None:
        super().__init__(message)
        self.msg = message
        self.doc = doc
        self.pos = pos
        self.lineno = None
        self.colno = None


def _parse_value(value: str):
    text = value.strip()
    if not text:
        raise TOMLDecodeError("Invalid TOML value")
    if text.startswith('"') and text.endswith('"') and len(text) >= 2:
        return text[1:-1]
    signed = text[1:] if text[:1] in {"+", "-"} else text
    if signed.isdigit():
        return int(text)
    raise TOMLDecodeError("Invalid TOML value")


def loads(payload: str) -> dict[str, object]:
    _MOLT_TOMLLIB_RUNTIME_READY()
    if not isinstance(payload, str):
        raise TypeError("tomllib.loads() argument must be str")
    out: dict[str, object] = {}
    normalized = payload.replace("\r\n", "\n").replace("\r", "\n")
    for raw_line in normalized.split("\n"):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            raise TOMLDecodeError("Expected '=' in TOML key/value pair")
        key, value = line.split("=", 1)
        key = key.strip()
        if not key:
            raise TOMLDecodeError("Invalid TOML key")
        out[key] = _parse_value(value)
    return out


__all__ = ["TOMLDecodeError", "loads"]
