"""Exact JSON codec for durable identities, manifests, and evidence."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from molt.file_hashing import _sha256_bytes
from molt.file_publication import atomic_write_bytes


class ExactJsonError(ValueError):
    """Raised when JSON contains values outside the exact interchange format."""


def _object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    payload: dict[str, Any] = {}
    for key, value in pairs:
        if key in payload:
            raise ExactJsonError(f"duplicate JSON key {key!r}")
        payload[key] = value
    return payload


def _constant(value: str) -> None:
    raise ExactJsonError(f"non-finite JSON number {value!r}")


def loads_exact(value: str) -> Any:
    """Decode standard JSON without lossy duplicate keys or non-finite numbers."""

    return json.loads(
        value,
        object_pairs_hook=_object,
        parse_constant=_constant,
    )


def canonical_json_bytes(value: object, *, default: Any | None = None) -> bytes:
    """Encode the unique compact UTF-8 form used by durable content identities."""

    return json.dumps(
        value,
        allow_nan=False,
        default=default,
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")


def canonical_json_sha256(value: object, *, default: Any | None = None) -> str:
    """Hash the unique compact UTF-8 form used by durable content identities."""

    return _sha256_bytes(canonical_json_bytes(value, default=default))


def dumps_exact(
    value: object,
    *,
    indent: int | None = 2,
    sort_keys: bool = True,
    default: Any | None = None,
) -> str:
    """Encode deterministic, finite JSON text terminated by exactly one LF."""

    return (
        json.dumps(
            value,
            allow_nan=False,
            default=default,
            ensure_ascii=False,
            indent=indent,
            separators=(",", ":") if indent is None else None,
            sort_keys=sort_keys,
        )
        + "\n"
    )


def encode_exact(
    value: object,
    *,
    indent: int | None = 2,
    sort_keys: bool = True,
    default: Any | None = None,
) -> bytes:
    """Encode canonical, finite JSON as deterministic UTF-8 with one LF."""

    return dumps_exact(
        value,
        indent=indent,
        sort_keys=sort_keys,
        default=default,
    ).encode("utf-8")


def write_exact(path: Path, value: object, *, exclusive: bool = False) -> None:
    """Crash-consistently publish exact JSON as one complete filesystem leaf."""

    atomic_write_bytes(path, encode_exact(value), exclusive=exclusive)
