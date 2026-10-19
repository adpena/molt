from __future__ import annotations

import ast
import hashlib
import json
import os
from typing import Any, Iterable, Mapping, TextIO

from molt.frontend import (
    BoundMethodHint,
    BuiltinClass,
    ClassId,
    DeclaredClass,
    MoltValue,
    ReceiverKind,
)
from molt.opcode_literal_payloads import integer_literal_bytes, literal_ast_dump

from molt.cli import function_records
from molt.cli.function_records import FunctionDigestSeed

from molt.cli.cache_fingerprints import _cache_fingerprint, _cache_tooling_fingerprint
from molt.cli.rust_cpu_policy import RustCpuPolicy


_CACHE_KEY_SCHEMA_VERSION = "v6"
_FUNCTION_CACHE_KEY_SCHEMA_VERSION = "func-v4"
# Backend object cache keys hash the serialized backend IR payload. Frontend
# tooling changes are upstream of this boundary and matter only when they change
# that payload.
_BACKEND_IR_PAYLOAD_TOOLING_FINGERPRINT = "backend-ir-payload-authority-v1"
_CACHE_JSON_TAGS = frozenset(
    {
        "__integer_bytes__",
        "__dict__",
        "__ellipsis__",
        "__bytes__",
        "__complex__",
        "__tuple__",
        "__ast__",
        "__set__",
        "__frozenset__",
        "__molt_value__",
        "__class_id__",
        "__builtin_class__",
        "__declared_class__",
        "__bound_method__",
    }
)

type _ClassIdentity = ClassId | BuiltinClass | DeclaredClass | BoundMethodHint


# ── Class identity codec (HF-203) ──────────────────────────────────────────
# The one place that turns a class identity into cache JSON and back. A
# ``ClassId`` is ``{"__class_id__": [module, qualname, [line, column] | null]}``;
# a map keyed by ``ClassId`` is written in ``ClassId.sort_key`` order, so the
# lowering-context digest never depends on integration order.


def _encode_class_identity(value: _ClassIdentity) -> dict[str, Any]:
    if isinstance(value, ClassId):
        occurrence = None if value.occurrence is None else list(value.occurrence)
        return {"__class_id__": [value.module, value.qualname, occurrence]}
    if isinstance(value, BuiltinClass):
        return {"__builtin_class__": value.name}
    if isinstance(value, DeclaredClass):
        return {
            "__declared_class__": [
                _encode_class_identity(value.class_id),
                value.kind.value,
            ]
        }
    return {"__bound_method__": [_encode_class_identity(value.owner), value.method]}


def _decode_class_id(payload: Any) -> ClassId:
    if not (
        isinstance(payload, list)
        and len(payload) == 3
        and isinstance(payload[0], str)
        and isinstance(payload[1], str)
    ):
        raise ValueError("invalid cached class identity")
    occurrence = payload[2]
    if occurrence is None:
        return ClassId(payload[0], payload[1])
    if not (
        isinstance(occurrence, list)
        and len(occurrence) == 2
        and all(type(item) is int for item in occurrence)
    ):
        raise ValueError("invalid cached class occurrence")
    return ClassId(payload[0], payload[1], (occurrence[0], occurrence[1]))


def _decode_class_ref(value: Any) -> ClassId | BuiltinClass:
    if isinstance(value, dict) and len(value) == 1:
        if "__class_id__" in value:
            return _decode_class_id(value["__class_id__"])
        if "__builtin_class__" in value and isinstance(value["__builtin_class__"], str):
            return BuiltinClass(value["__builtin_class__"])
    raise ValueError("invalid cached class reference")


def _decode_class_identity(value: dict[str, Any]) -> _ClassIdentity | None:
    """Decode one tagged class identity; None when ``value`` carries no tag."""
    if len(value) != 1:
        return None
    if "__class_id__" in value or "__builtin_class__" in value:
        return _decode_class_ref(value)
    if "__declared_class__" in value:
        payload = value["__declared_class__"]
        if not (isinstance(payload, list) and len(payload) == 2):
            raise ValueError("invalid cached declared class")
        class_id = _decode_class_ref(payload[0])
        if not isinstance(class_id, ClassId):
            raise ValueError("a declared class names a class statement")
        return DeclaredClass(class_id, ReceiverKind(payload[1]))
    if "__bound_method__" in value:
        payload = value["__bound_method__"]
        if not (
            isinstance(payload, list)
            and len(payload) == 2
            and isinstance(payload[1], str)
        ):
            raise ValueError("invalid cached bound method")
        return BoundMethodHint(_decode_class_ref(payload[0]), payload[1])
    return None


def _cache_json_sort_key(value: Any) -> str:
    """Order already-encoded heterogeneous keys or set members losslessly."""
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), default=_json_ir_default
    )


def _cache_json_value(value: Any) -> Any:
    """Prepare compiler metadata without decimalizing arbitrary Python ints.

    Retain unchanged JSON containers. String-keyed maps are compiler metadata
    (names and schema fields), so their order is irrelevant to cache identity.
    Python dict literals remain ordered AST/IR, never raw metadata maps. Escape
    tag-shaped name maps and preserve insertion order for non-string key maps.
    Tags are decoded before semantic consumers see the cached metadata.
    """
    if type(value) is int and not -(1 << 63) <= value < (1 << 63):
        return {"__integer_bytes__": list(integer_literal_bytes(value))}
    if isinstance(value, (ClassId, BuiltinClass, DeclaredClass, BoundMethodHint)):
        return _encode_class_identity(value)
    if isinstance(value, dict):
        name_map = all(isinstance(key, str) for key in value)
        if not name_map or any(key in _CACHE_JSON_TAGS for key in value):
            if name_map:
                items = sorted(value.items())
            elif value and all(isinstance(key, ClassId) for key in value):
                items = sorted(value.items(), key=lambda item: item[0].sort_key())
            else:
                items = value.items()
            return {
                "__dict__": [
                    [_cache_json_value(key), _cache_json_value(child)]
                    for key, child in items
                ]
            }
        result = None
        for key, child in value.items():
            replacement = _cache_json_value(child)
            if replacement is not child:
                if result is None:
                    result = dict(value)
                result[key] = replacement
        return value if result is None else result
    if isinstance(value, tuple):
        return {"__tuple__": [_cache_json_value(item) for item in value]}
    if isinstance(value, list):
        result = None
        for index, child in enumerate(value):
            replacement = _cache_json_value(child)
            if replacement is not child:
                if result is None:
                    result = list(value)
                result[index] = replacement
        if result is not None:
            return result
    return value


def _json_ir_default(value: Any) -> Any:
    if isinstance(value, complex):
        return {"__complex__": [value.real, value.imag]}
    if value is Ellipsis:
        return {"__ellipsis__": True}
    if isinstance(value, bytes):
        return {"__bytes__": list(value)}
    if isinstance(value, tuple):
        return _cache_json_value(value)
    if isinstance(value, ast.AST):
        # Encode as the bare ``ast.dump`` string, NOT a ``{"__ast__": ...}`` wrapper.
        # The cache decoder (``_decode_cached_json_value``) collapses a stored
        # ``{"__ast__": s}`` back to the bare string ``s``, so a payload that has
        # been round-tripped through the analysis/lowering cache holds a bare
        # string where a freshly-analysed payload holds a live ``ast.AST``. If the
        # encoder wrapped the live node in a dict, those two provably-equivalent
        # forms would serialise to different bytes, making the frontend-lowering
        # context digest depend on cache-population state rather than only on the
        # source -- the shared per-module lowering cache then almost never hits
        # across fresh witness sessions (observed hit_rate ~0.19). Emitting the
        # same bare string the decoder produces makes the round-trip idempotent:
        # encode(node) == encode(decode(encode(node))). Proven representation-only
        # (does not change lowered IR) by the two-direction gate in
        # test_cli_lowering_context_ast_idempotent.py.
        return literal_ast_dump(value)
    if isinstance(value, (set, frozenset)):
        items = [_cache_json_value(item) for item in value]
        items.sort(key=_cache_json_sort_key)
        tag = "__frozenset__" if isinstance(value, frozenset) else "__set__"
        return {tag: items}
    if isinstance(value, (ClassId, BuiltinClass, DeclaredClass, BoundMethodHint)):
        return _encode_class_identity(value)
    if isinstance(value, MoltValue):
        type_hint = value.type_hint
        return {
            "__molt_value__": {
                "name": value.name,
                "type_hint": (
                    type_hint
                    if isinstance(type_hint, str)
                    else _encode_class_identity(type_hint)
                ),
            }
        }
    raise TypeError(f"Object of type {type(value).__name__} is not JSON serializable")


def _cache_ir_payload_ir(ir: Mapping[str, Any]) -> dict[str, Any]:
    funcs = ir.get("functions")
    normalized: dict[str, Any] = dict(ir)
    if isinstance(funcs, list):
        normalized["functions"] = _sorted_ir_functions(funcs)
    return normalized


def _sorted_ir_functions(functions: list[Any]) -> list[Any]:
    def _func_sort_key(entry: Any) -> str:
        if isinstance(entry, dict):
            name = entry.get("name")
            if isinstance(name, str):
                return name
        return ""

    return sorted(functions, key=_func_sort_key)


def _cache_backend_payload_ir(ir: Mapping[str, Any]) -> dict[str, Any]:
    functions = ir.get("functions")
    sorted_funcs: list[Any] = []
    if isinstance(functions, list):
        sorted_funcs = _sorted_ir_functions(functions)

    return {
        "functions": sorted_funcs,
        "profile": ir.get("profile"),
        "top_level_extras_digest": _ir_top_level_extras_digest(ir),
    }


def _cache_json_payload_bytes(value: object) -> bytes:
    """The canonical JSON of any IR value: a payload, a header or a function."""
    encoder = json.JSONEncoder(
        sort_keys=True,
        separators=(",", ":"),
        default=_json_ir_default,
    )
    return encoder.encode(_cache_json_value(value)).encode("utf-8")


# Domain tag of the Merkle payload digest; framing is fixed so no two payloads
# share an encoding: tag, header digest, function count, function digests.
_PAYLOAD_DIGEST_TAG = b"molt.cache-payload.merkle.msgpack-functions.v2\0"


class FunctionDigests:
    """One binary-body digest per strongly held executable function identity.

    Seeds come from the admitted record producer/decoder. Replacements created
    by copy-on-write assembly have different identities and use the same codec
    once here. This owner starts after rewrites; functions must stay unchanged
    for its lifetime. Optional verification re-encodes seeds once at admission.
    """

    __slots__ = ("_by_identity",)

    def __init__(self, seeds: Iterable[FunctionDigestSeed] = ()) -> None:
        admitted: dict[int, tuple[dict[str, Any], bytes]] = {}
        for seed in seeds:
            if (
                not isinstance(seed, FunctionDigestSeed)
                or not isinstance(seed.function, dict)
                or type(seed.digest) is not bytes
                or len(seed.digest) != 32
            ):
                raise ValueError("invalid function digest seed")
            identity = id(seed.function)
            previous = admitted.get(identity)
            if previous is not None and previous[1] != seed.digest:
                raise ValueError("conflicting function digest seeds")
            admitted[identity] = (seed.function, seed.digest)
        if os.environ.get("MOLT_VERIFY_FUNCTION_DIGESTS") == "1":
            for function, digest in admitted.values():
                actual = function_records.function_body_digest(
                    function_records.encode_function_body(function)
                )
                if actual != digest:
                    raise ValueError("function digest seed verification failed")
        # Publish only after every seed and optional verification has succeeded.
        self._by_identity = admitted

    def digest(self, function: object) -> bytes:
        held = self._by_identity.get(id(function))
        if held is not None and held[0] is function:
            return held[1]
        if not isinstance(function, dict):
            raise TypeError("function digest requires a function dict")
        digest = function_records.function_body_digest(
            function_records.encode_function_body(function)
        )
        self._by_identity[id(function)] = (function, digest)
        return digest


def _cache_payload_digest(
    payload_ir: Mapping[str, Any], function_digests: FunctionDigests | None = None
) -> str:
    """Identify a payload by its header and the digest of each function.

    Headers retain canonical rich JSON; functions use their domain-tagged
    sorted MessagePack bytes. Function order and header/body boundaries matter.
    """
    functions = payload_ir.get("functions")
    digest = hashlib.sha256(_PAYLOAD_DIGEST_TAG)
    if not isinstance(functions, list):
        digest.update(b"\0")
        digest.update(hashlib.sha256(_cache_json_payload_bytes(payload_ir)).digest())
        return digest.hexdigest()
    owner = function_digests if function_digests is not None else FunctionDigests()
    header = {key: value for key, value in payload_ir.items() if key != "functions"}
    digest.update(b"\1")
    digest.update(hashlib.sha256(_cache_json_payload_bytes(header)).digest())
    digest.update(len(functions).to_bytes(8, "big"))
    for function in functions:
        digest.update(owner.digest(function))
    return digest.hexdigest()


def _cache_key_for_payload_digest(
    payload_digest: str,
    *,
    target: str,
    target_triple: str | None,
    variant: str,
    schema_version: str,
    compiler_fingerprint: str | None = None,
    tooling_fingerprint: str | None = None,
) -> str:
    suffix = target_triple or target
    if variant:
        suffix = f"{suffix}:{variant}"
    if compiler_fingerprint is None:
        # No admitted compiler means no build request: the baseline compiler.
        compiler_fingerprint = _cache_fingerprint(rust_cpu=RustCpuPolicy.BASELINE)
    if tooling_fingerprint is None:
        tooling_fingerprint = _cache_tooling_fingerprint()
    digest = hashlib.sha256()
    digest.update(b"payload-digest-v1")
    digest.update(b"|")
    digest.update(payload_digest.encode("utf-8"))
    digest.update(b"|")
    digest.update(suffix.encode("utf-8"))
    digest.update(b"|")
    digest.update(compiler_fingerprint.encode("utf-8"))
    digest.update(b"|")
    digest.update(tooling_fingerprint.encode("utf-8"))
    digest.update(b"|")
    digest.update(schema_version.encode("utf-8"))
    return digest.hexdigest()


def _cache_key_for_json_payload_bytes(
    payload_chunks: Iterable[bytes],
    *,
    target: str,
    target_triple: str | None,
    variant: str,
    schema_version: str,
    compiler_fingerprint: str | None = None,
    tooling_fingerprint: str | None = None,
) -> str:
    suffix = target_triple or target
    if variant:
        suffix = f"{suffix}:{variant}"
    if compiler_fingerprint is None:
        # No admitted compiler means no build request: the baseline compiler.
        compiler_fingerprint = _cache_fingerprint(rust_cpu=RustCpuPolicy.BASELINE)
    if tooling_fingerprint is None:
        tooling_fingerprint = _cache_tooling_fingerprint()
    payload_hasher = hashlib.sha256()
    for chunk in payload_chunks:
        payload_hasher.update(chunk)
    return _cache_key_for_payload_digest(
        payload_hasher.hexdigest(),
        target=target,
        target_triple=target_triple,
        variant=variant,
        schema_version=schema_version,
        compiler_fingerprint=compiler_fingerprint,
        tooling_fingerprint=tooling_fingerprint,
    )


def _cache_key_for_payload_ir(
    payload_ir: Mapping[str, Any],
    *,
    target: str,
    target_triple: str | None,
    variant: str,
    schema_version: str,
    compiler_fingerprint: str | None = None,
    tooling_fingerprint: str | None = None,
    payload_digest: str | None = None,
    function_digests: FunctionDigests | None = None,
) -> str:
    if payload_digest is None:
        payload_digest = _cache_payload_digest(payload_ir, function_digests)
    return _cache_key_for_payload_digest(
        payload_digest,
        target=target,
        target_triple=target_triple,
        variant=variant,
        schema_version=schema_version,
        compiler_fingerprint=compiler_fingerprint,
        tooling_fingerprint=tooling_fingerprint,
    )


def _cache_key(
    ir: Mapping[str, Any],
    target: str,
    target_triple: str | None,
    variant: str = "",
    payload_ir: Mapping[str, Any] | None = None,
    payload_digest: str | None = None,
    compiler_fingerprint: str | None = None,
    tooling_fingerprint: str | None = None,
    function_digests: FunctionDigests | None = None,
) -> str:
    if payload_ir is None:
        payload_ir = _cache_ir_payload_ir(ir)
    return _cache_key_for_payload_ir(
        payload_ir,
        target=target,
        target_triple=target_triple,
        variant=variant,
        schema_version=_CACHE_KEY_SCHEMA_VERSION,
        compiler_fingerprint=compiler_fingerprint,
        tooling_fingerprint=tooling_fingerprint,
        payload_digest=payload_digest,
        function_digests=function_digests,
    )


def _ir_top_level_extras_digest(ir: Mapping[str, Any]) -> str:
    extras = {
        key: value for key, value in ir.items() if key not in {"functions", "profile"}
    }
    encoded = json.dumps(
        _cache_json_value(extras),
        sort_keys=True,
        separators=(",", ":"),
        default=_json_ir_default,
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


# The compact backend IR JSON projection. In-memory text and streamed lease
# writes share this encoder, so both produce identical bytes.
_BACKEND_IR_JSON_ENCODER = json.JSONEncoder(
    separators=(",", ":"), default=_json_ir_default
)


def _backend_ir_text(ir: Mapping[str, Any]) -> str:
    return _BACKEND_IR_JSON_ENCODER.encode(ir)


def _write_backend_ir_text(handle: TextIO, ir: Mapping[str, Any]) -> None:
    """Write ``_backend_ir_text(ir)`` without holding the whole document.

    ``json.dump`` always takes CPython's pure-Python encoder and writes once per
    token. Here the C encoder encodes each top-level member, and each element of
    a top-level array, so only one element's text is resident at a time.
    """
    if not isinstance(ir, dict):
        raise TypeError(f"Object of type {type(ir).__name__} is not JSON serializable")
    encode = _BACKEND_IR_JSON_ENCODER.encode
    handle.write("{")
    for position, (key, value) in enumerate(ir.items()):
        if not isinstance(key, str):
            raise TypeError(f"backend IR keys must be str, not {type(key).__name__}")
        if position:
            handle.write(",")
        handle.write(encode(key))
        handle.write(":")
        if isinstance(value, list):
            handle.write("[")
            for index, item in enumerate(value):
                if index:
                    handle.write(",")
                handle.write(encode(item))
            handle.write("]")
        else:
            handle.write(encode(value))
    handle.write("}")


def _backend_ir_bytes(ir: Mapping[str, Any]) -> bytes:
    return _backend_ir_text(ir).encode("utf-8")


def _function_cache_key(
    ir: Mapping[str, Any],
    target: str,
    target_triple: str | None,
    variant: str = "",
    payload_ir: Mapping[str, Any] | None = None,
    payload_digest: str | None = None,
    compiler_fingerprint: str | None = None,
    tooling_fingerprint: str | None = None,
) -> str:
    if payload_ir is None:
        payload_ir = _cache_backend_payload_ir(ir)
    return _cache_key_for_payload_ir(
        payload_ir,
        target=target,
        target_triple=target_triple,
        variant=variant,
        schema_version=_FUNCTION_CACHE_KEY_SCHEMA_VERSION,
        compiler_fingerprint=compiler_fingerprint,
        tooling_fingerprint=tooling_fingerprint,
        payload_digest=payload_digest,
    )
