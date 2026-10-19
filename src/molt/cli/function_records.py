"""Pure function bytes and their persisted digest, independent of rich metadata.

The frontend owns the recursive executable value domain. This codec neither
normalizes executable values nor visits them through the rich metadata codec.
A module header is opaque bytes so its source/context can be admitted before
any function body is materialized. Raw views belong to that short-lived reader;
digest seeds retain only the actual decoded function and its digest.
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
import hashlib
from typing import Any, Generic, TypeVar

import msgspec


_FUNCTION_DIGEST_DOMAIN = b"molt.function-body.msgpack.v1\0"
_ENCODER = msgspec.msgpack.Encoder(order="sorted")


@dataclass(frozen=True, slots=True)
class FunctionDigestSeed:
    function: dict[str, Any]
    digest: bytes


class _FunctionRecord(
    msgspec.Struct, frozen=True, array_like=True, forbid_unknown_fields=True
):
    body: bytes
    digest: bytes


_Record = TypeVar("_Record")


class LoweringRecord(
    msgspec.Struct,
    Generic[_Record],
    frozen=True,
    array_like=True,
    forbid_unknown_fields=True,
    tag="molt.lowering-record.v1",
):
    metadata: bytes
    functions: tuple[_Record, ...]


_VIEW_DECODER = msgspec.msgpack.Decoder(LoweringRecord[msgspec.Raw])
_RECORD_DECODER = msgspec.msgpack.Decoder(_FunctionRecord)
_BODY_DECODER = msgspec.msgpack.Decoder(dict[str, Any])


def encode_function_body(function: dict[str, Any]) -> bytes:
    """Encode an already-admitted pure function deterministically once."""
    if not isinstance(function, dict) or any(
        not isinstance(key, str) for key in function
    ):
        raise TypeError("function record requires a string-keyed function dict")
    return _ENCODER.encode(function)


def function_body_digest(encoded: bytes | msgspec.Raw) -> bytes:
    """Hash the exact encoded body in its own explicit codec domain."""
    digest = hashlib.sha256(_FUNCTION_DIGEST_DOMAIN)
    digest.update(encoded)
    return digest.digest()


def encode_lowering_record(
    metadata: bytes, functions: Sequence[dict[str, Any]]
) -> tuple[bytes, tuple[FunctionDigestSeed, ...]]:
    if not isinstance(metadata, bytes):
        raise TypeError("lowering metadata must be encoded bytes")
    records: list[_FunctionRecord] = []
    seeds: list[FunctionDigestSeed] = []
    for function in functions:
        body = encode_function_body(function)
        digest = function_body_digest(body)
        records.append(_FunctionRecord(body, digest))
        seeds.append(FunctionDigestSeed(function, digest))
    return _ENCODER.encode(LoweringRecord(metadata, tuple(records))), tuple(seeds)


def decode_lowering_record(encoded: bytes) -> LoweringRecord[msgspec.Raw]:
    """Decode only the frame/header; the caller admits metadata before bodies."""
    try:
        return _VIEW_DECODER.decode(encoded)
    except msgspec.DecodeError as error:
        raise ValueError(f"invalid lowering record: {error}") from error


def decode_function_records(
    view: LoweringRecord[msgspec.Raw],
) -> tuple[list[dict[str, Any]], tuple[FunctionDigestSeed, ...]]:
    """Verify exact body bytes, then materialize each function once.

    No raw view or body buffer is retained by the returned functions/seeds.
    The caller releases the module view after leaving its admission boundary.
    """
    functions: list[dict[str, Any]] = []
    seeds: list[FunctionDigestSeed] = []
    try:
        for raw in view.functions:
            record = _RECORD_DECODER.decode(raw)
            if (
                len(record.digest) != 32
                or function_body_digest(record.body) != record.digest
            ):
                raise ValueError("function record digest mismatch")
            function = _BODY_DECODER.decode(record.body)
            functions.append(function)
            seeds.append(FunctionDigestSeed(function, record.digest))
    except msgspec.DecodeError as error:
        raise ValueError(f"invalid function record: {error}") from error
    return functions, tuple(seeds)
